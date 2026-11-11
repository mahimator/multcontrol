//! Windows input backend.
//!
//! Capture uses low-level hooks (`WH_KEYBOARD_LL` / `WH_MOUSE_LL`), the only
//! mechanism that both sees input system-wide *and* can swallow it without
//! administrator rights. Injection uses `SendInput`/`SetCursorPos`, which also
//! need no elevation as long as we stay at the same integrity level as the
//! foreground applications.
//!
//! No sockets are opened here and no firewall rules are touched.

use super::{mods_from_keys, CaptureEvent, CaptureOptions};
use anyhow::{Context, Result};
use mc_core::key::{Key, WinScan};
use mc_core::proto::{Edge, KeyAction, MouseButton};
use std::collections::HashSet;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{BOOL, HANDLE, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, VIRTUAL_KEY, VK_CAPITAL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    PostThreadMessageW, SetCursorPos, SetWindowsHookExW, ShowCursor, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1, XBUTTON2,
};

/// Marker written into `dwExtraInfo` on everything we inject, so our own
/// synthetic events are never mistaken for physical ones.
const MAGIC: usize = 0x4D43_4C54_0000_0001;
/// Injected mouse events carry this flag.
const LLMHF_INJECTED: u32 = 0x0000_0001;
/// `WM_QUIT`, used to stop the hook thread.
const WM_QUIT: u32 = 0x0012;
/// Ignore a second hotkey press this soon after the first, in milliseconds.
const HOTKEY_DEBOUNCE_MS: u64 = 400;

struct HookState {
    tx: Sender<CaptureEvent>,
    opts: CaptureOptions,
    /// When set, physical input is forwarded instead of reaching local apps.
    suppress: AtomicBool,
    pressed: Mutex<HashSet<Key>>,
    last_hotkey_ms: AtomicU64,
    /// Last pointer position observed by the mouse hook.
    last_pos: Mutex<(i32, i32)>,
    /// Position we warped to, so the resulting synthetic move can be ignored.
    warp_guard: Mutex<Option<(i32, i32)>>,
    /// Whether we currently have the system cursor hidden.
    cursor_hidden: AtomicBool,
    /// True between pressing and releasing the hotkey chord.
    hotkey_engaged: AtomicBool,
    /// Set once an edge event fires, cleared when the pointer leaves the edge.
    edge_latched: AtomicBool,
    /// Do not treat pointer position as an edge push before this timestamp.
    /// Set when control comes back, because parking the pointer on an edge
    /// otherwise looks exactly like the user pushing through it.
    edge_guard_ms: AtomicU64,
    thread_id: AtomicU32,
    /// Whether the hooks were installed, so `start` can report failure instead
    /// of returning success and silently capturing nothing.
    start_result: Mutex<Option<Result<(), String>>>,
}

static STATE: OnceLock<Arc<HookState>> = OnceLock::new();

fn now_ms() -> u64 {
    unsafe { GetTickCount() as u64 }
}

fn caps_lock_on() -> bool {
    unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 }
}

fn virtual_screen_bounds() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = STATE.get() else {
        return CallNextHookEx(None, code, wparam, lparam);
    };
    let msg = wparam.0 as u32;
    let mut swallow = false;
    if code == HC_ACTION as i32 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if !kb.flags.contains(LLKHF_INJECTED) && kb.flags.0 & LLMHF_INJECTED == 0 {
            swallow = state.on_key(kb, msg);
        }
    }
    if swallow {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = STATE.get() else {
        return CallNextHookEx(None, code, wparam, lparam);
    };
    let msg = wparam.0 as u32;
    let mut swallow = false;
    if code == HC_ACTION as i32 {
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if ms.flags & LLMHF_INJECTED == 0 {
            swallow = state.on_mouse(ms, msg);
        }
    }
    if swallow {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wparam, lparam)
    }
}

impl HookState {
    fn on_key(&self, kb: &KBDLLHOOKSTRUCT, msg: u32) -> bool {
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if !down && !up {
            return false;
        }
        let scan_hi = ((kb.scanCode >> 8) & 0xFF) as u8;
        let extended = scan_hi == 0xE0 || scan_hi == 0xE1 || kb.flags.contains(LLKHF_EXTENDED);
        let scan = WinScan {
            code: (kb.scanCode & 0xFF) as u8,
            extended,
        };
        let Some(key) = Key::from_win_scan(scan, kb.vkCode as u16) else {
            // Unknown key: never swallow it, or the keyboard would feel broken.
            return false;
        };

        let mut pressed = self.pressed.lock().unwrap();
        let was_down = pressed.contains(&key);
        if down {
            pressed.insert(key);
        } else {
            pressed.remove(&key);
        }
        let current_mods =
            mods_from_keys(&pressed.iter().copied().collect::<Vec<_>>(), caps_lock_on());
        drop(pressed);

        // Hotkey detection: only on a fresh key-down, and only when every
        // required modifier is held.
        let mut hotkey_hit = false;
        if down
            && !was_down
            && key == self.opts.hotkey.key
            && current_mods & self.opts.hotkey.mods == self.opts.hotkey.mods
        {
            let now = now_ms();
            let last = self.last_hotkey_ms.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= HOTKEY_DEBOUNCE_MS {
                self.last_hotkey_ms.store(now, Ordering::Relaxed);
                hotkey_hit = true;
                self.hotkey_engaged.store(true, Ordering::Relaxed);
                let _ = self.tx.send(CaptureEvent::Hotkey);
            }
        }

        let was_engaged = self.hotkey_engaged.load(Ordering::Relaxed);
        if up && key == self.opts.hotkey.key {
            self.hotkey_engaged.store(false, Ordering::Relaxed);
        }

        let remote = self.suppress.load(Ordering::Relaxed);
        if remote && !hotkey_hit {
            let action = if down {
                if was_down {
                    KeyAction::Repeat
                } else {
                    KeyAction::Down
                }
            } else {
                KeyAction::Up
            };
            let _ = self.tx.send(CaptureEvent::Key {
                action,
                key,
                mods: current_mods,
            });
        }

        // Swallow everything in remote mode. Locally, swallow the hotkey chord
        // but let modifiers through so no application ends up with a stuck
        // modifier after the switch.
        remote || hotkey_hit || (was_engaged && !key.is_modifier())
    }

    fn on_mouse(&self, ms: &MSLLHOOKSTRUCT, msg: u32) -> bool {
        let remote = self.suppress.load(Ordering::Relaxed);
        let mut last = self.last_pos.lock().unwrap();
        let pos = (ms.pt.x, ms.pt.y);
        let dx = (pos.0 - last.0).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let dy = (pos.1 - last.1).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let (vx, vy, vw, vh) = virtual_screen_bounds();

        match msg {
            WM_MOUSEMOVE => {
                // The warp is applied asynchronously, so allow a couple of
                // pixels of slop if a real movement raced with it. Only
                // consume the guard when it matches: a real move may arrive
                // before the warp's own event, and consuming it early would
                // let the warp through as a huge jump.
                let guard = *self.warp_guard.lock().unwrap();
                if let Some(guard) = guard {
                    if (guard.0 - pos.0).abs() <= 2 && (guard.1 - pos.1).abs() <= 2 {
                        *self.warp_guard.lock().unwrap() = None;
                        *last = pos;
                        return false;
                    }
                }
                if remote {
                    // A jump this large is a warp or a pointer teleport, not
                    // something the user did with the mouse.
                    if (dx != 0 || dy != 0) && dx.abs() <= 400 && dy.abs() <= 400 {
                        let _ = self.tx.send(CaptureEvent::MouseMove { dx, dy });
                    }
                    // Park the pointer in the middle so it can never stick to a
                    // screen edge and starve us of movement deltas.
                    let center = (vx + vw / 2, vy + vh / 2);
                    *self.warp_guard.lock().unwrap() = Some(center);
                    unsafe {
                        let _ = SetCursorPos(center.0, center.1);
                    }
                    *last = center;
                    return false;
                }
                if self.opts.edge_push
                    && (dx != 0 || dy != 0)
                    && now_ms() >= self.edge_guard_ms.load(Ordering::Relaxed)
                {
                    if let Some(edge) =
                        edge_for(pos, (vx, vy, vw, vh), self.opts.edge_margin, dx, dy)
                    {
                        if !self.edge_latched.swap(true, Ordering::Relaxed) {
                            let _ = self.tx.send(CaptureEvent::Edge(edge));
                        }
                    } else {
                        self.edge_latched.store(false, Ordering::Relaxed);
                    }
                }
                *last = pos;
                false
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let button = match msg {
                    WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
                    WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
                    WM_MBUTTONDOWN | WM_MBUTTONUP => MouseButton::Middle,
                    _ => {
                        if (ms.mouseData >> 16) as u16 == XBUTTON1 {
                            MouseButton::Extra1
                        } else {
                            MouseButton::Extra2
                        }
                    }
                };
                let pressed = matches!(
                    msg,
                    WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
                );
                if remote {
                    let _ = self.tx.send(CaptureEvent::MouseButton { button, pressed });
                }
                *last = pos;
                remote
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = ((ms.mouseData >> 16) as i16) / 120;
                if remote {
                    let (dx, dy) = if msg == WM_MOUSEWHEEL {
                        (0, delta)
                    } else {
                        (delta, 0)
                    };
                    let _ = self.tx.send(CaptureEvent::Wheel { dx, dy });
                }
                *last = pos;
                remote
            }
            _ => false,
        }
    }
}

fn edge_for(
    pos: (i32, i32),
    bounds: (i32, i32, i32, i32),
    margin: i32,
    dx: i16,
    dy: i16,
) -> Option<Edge> {
    let (x, y, w, h) = bounds;
    if dx > 0 && pos.0 >= x + w - margin {
        Some(Edge::Right)
    } else if dx < 0 && pos.0 <= x + margin {
        Some(Edge::Left)
    } else if dy > 0 && pos.1 >= y + h - margin {
        Some(Edge::Bottom)
    } else if dy < 0 && pos.1 <= y + margin {
        Some(Edge::Top)
    } else {
        None
    }
}

fn send_input(input: &[INPUT]) {
    unsafe {
        SendInput(input, size_of::<INPUT>() as i32);
    }
}

/// Windows implementation of the input backend.
pub struct WindowsBackend {
    /// Kept alive for the lifetime of the process; the hook thread runs the
    /// message pump that makes the low-level hooks callable.
    #[allow(dead_code)]
    hook_thread: Mutex<Option<JoinHandle<()>>>,
    started: AtomicBool,
}

impl WindowsBackend {
    /// Create the backend and declare per-monitor DPI awareness so pointer
    /// coordinates are real pixels.
    pub fn new() -> Result<WindowsBackend> {
        unsafe {
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
        Ok(WindowsBackend {
            hook_thread: Mutex::new(None),
            started: AtomicBool::new(false),
        })
    }

    fn state(&self) -> Option<&'static Arc<HookState>> {
        STATE.get()
    }

    fn send_key(&self, action: KeyAction, key: Key, _mods: u8) {
        let Some((scan, vk)) = key.to_win_scan() else {
            return;
        };
        let mut flags = KEYEVENTF_SCANCODE;
        // With KEYEVENTF_SCANCODE, wScan is the raw scan code; the E0 prefix
        // is expressed by KEYEVENTF_EXTENDEDKEY, not by adding 0xE000.
        let scan_code = scan.code as u16;
        if scan.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if action == KeyAction::Up {
            flags |= KEYEVENTF_KEYUP;
        }
        let mut wvk = VIRTUAL_KEY(0);
        let mut wscan = scan_code;
        if scan.code == 0 {
            // Keys that cannot be expressed as a scan code fall back to VK.
            flags = KEYBD_EVENT_FLAGS(0);
            if action == KeyAction::Up {
                flags |= KEYEVENTF_KEYUP;
            }
            wvk = VIRTUAL_KEY(vk);
            wscan = 0;
        }
        send_input(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: wvk,
                    wScan: wscan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: MAGIC,
                },
            },
        }]);
    }
}

impl super::Backend for WindowsBackend {
    fn start(&self, tx: Sender<CaptureEvent>, opts: CaptureOptions) -> Result<()> {
        let (x, y) = unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            (p.x, p.y)
        };
        let state = Arc::new(HookState {
            tx,
            opts,
            suppress: AtomicBool::new(false),
            pressed: Mutex::new(HashSet::new()),
            last_hotkey_ms: AtomicU64::new(0),
            last_pos: Mutex::new((x, y)),
            warp_guard: Mutex::new(None),
            cursor_hidden: AtomicBool::new(false),
            hotkey_engaged: AtomicBool::new(false),
            edge_latched: AtomicBool::new(false),
            edge_guard_ms: AtomicU64::new(0),
            thread_id: AtomicU32::new(0),
            start_result: Mutex::new(None),
        });
        STATE
            .set(state.clone())
            .map_err(|_| anyhow::anyhow!("input backend already started"))?;

        let thread_state = state.clone();
        std::thread::Builder::new()
            .name("mc-input-win".into())
            .spawn(move || unsafe {
                let tid = GetCurrentThreadId();
                thread_state.thread_id.store(tid, Ordering::SeqCst);
                let hmod = GetModuleHandleW(None).expect("module handle");
                let kb_hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0)
                {
                    Ok(h) => h,
                    Err(e) => {
                        log::error!("keyboard hook failed: {e}");
                        *thread_state.start_result.lock().unwrap() =
                            Some(Err(format!("keyboard hook failed: {e}")));
                        return;
                    }
                };
                let ms_hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) {
                    Ok(h) => h,
                    Err(e) => {
                        log::error!("mouse hook failed: {e}");
                        let _ = UnhookWindowsHookEx(kb_hook);
                        *thread_state.start_result.lock().unwrap() =
                            Some(Err(format!("mouse hook failed: {e}")));
                        return;
                    }
                };
                *thread_state.start_result.lock().unwrap() = Some(Ok(()));
                log::info!("low-level input hooks installed (no administrator rights needed)");
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let _ = UnhookWindowsHookEx(kb_hook);
                let _ = UnhookWindowsHookEx(ms_hook);
                log::info!("input hooks removed");
            })
            .context("spawning input hook thread")?;

        // Wait for the hooks themselves, not just the thread: a policy or a
        // locked-down session can make SetWindowsHookEx fail, and pretending
        // to be started would leave the user with a silent, dead agent.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let result = loop {
            if let Some(result) = state.start_result.lock().unwrap().take() {
                break result;
            }
            if std::time::Instant::now() >= deadline {
                break Err("timed out installing the input hooks".to_string());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        result.map_err(|e| anyhow::anyhow!(e))?;
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn set_suppress(&self, suppress: bool) {
        if let Some(state) = self.state() {
            state.suppress.store(suppress, Ordering::SeqCst);
            if !suppress {
                state.edge_latched.store(false, Ordering::SeqCst);
                // Give the user a moment before an edge push counts again.
                state.edge_guard_ms.store(now_ms() + 350, Ordering::SeqCst);
            }
        }
    }

    fn inject_key(&self, action: KeyAction, key: Key, mods: u8) {
        self.send_key(action, key, mods);
    }

    fn inject_move(&self, dx: i16, dy: i16) {
        let (mut x, mut y) = self.pointer_position();
        let (vx, vy, vw, vh) = virtual_screen_bounds();
        x = (x + dx as i32).clamp(vx, vx + vw - 1);
        y = (y + dy as i32).clamp(vy, vy + vh - 1);
        let _ = unsafe { SetCursorPos(x, y) };
    }

    fn inject_button(&self, button: MouseButton, pressed: bool) {
        let flag = match (button, pressed) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            (MouseButton::Extra1, true) => MOUSEEVENTF_XDOWN,
            (MouseButton::Extra1, false) => MOUSEEVENTF_XUP,
            (MouseButton::Extra2, true) => MOUSEEVENTF_XDOWN,
            (MouseButton::Extra2, false) => MOUSEEVENTF_XUP,
        };
        // For XBUTTON events the identifier goes in the high word.
        let data = match button {
            MouseButton::Extra1 => (XBUTTON1 as u32) << 16,
            MouseButton::Extra2 => (XBUTTON2 as u32) << 16,
            _ => 0,
        };
        send_input(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: data,
                    dwFlags: flag,
                    time: 0,
                    dwExtraInfo: MAGIC,
                },
            },
        }]);
    }

    fn inject_wheel(&self, dx: i16, dy: i16) {
        for (flag, delta) in [(MOUSEEVENTF_WHEEL, dy), (MOUSEEVENTF_HWHEEL, dx)] {
            if delta == 0 {
                continue;
            }
            send_input(&[INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        // Wheel delta is a signed value in the high word.
                        mouseData: ((delta as i32 * 120) as u32) << 16,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: MAGIC,
                    },
                },
            }]);
        }
    }

    fn place_pointer_at_edge(&self, edge: Edge) {
        let (x, y, w, h) = virtual_screen_bounds();
        let (px, py) = match edge {
            Edge::Left => (x + 2, y + h / 2),
            Edge::Right => (x + w - 3, y + h / 2),
            Edge::Top => (x + w / 2, y + 2),
            Edge::Bottom => (x + w / 2, y + h - 3),
        };
        let _ = unsafe { SetCursorPos(px, py) };
        if let Some(state) = self.state() {
            *state.last_pos.lock().unwrap() = (px, py);
            // Skip the synthetic move this warp produces, and do not treat the
            // parked position as an edge push.
            *state.warp_guard.lock().unwrap() = Some((px, py));
            state.edge_guard_ms.store(now_ms() + 350, Ordering::SeqCst);
        }
    }

    fn pointer_position(&self) -> (i32, i32) {
        unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            (p.x, p.y)
        }
    }

    fn screen_bounds(&self) -> (i32, i32, i32, i32) {
        virtual_screen_bounds()
    }

    fn set_cursor_visible(&self, visible: bool) {
        let Some(state) = self.state() else {
            return;
        };
        let hidden = state.cursor_hidden.load(Ordering::SeqCst);
        if visible && hidden {
            unsafe {
                ShowCursor(BOOL(1));
            }
            state.cursor_hidden.store(false, Ordering::SeqCst);
        } else if !visible && !hidden {
            unsafe {
                ShowCursor(BOOL(0));
            }
            state.cursor_hidden.store(true, Ordering::SeqCst);
        }
    }
}

/// Stop the hook thread (used on shutdown).
pub fn shutdown() {
    if let Some(state) = STATE.get() {
        let tid = state.thread_id.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Keeps the `HANDLE` import meaningful across cfg combinations.
#[allow(dead_code)]
type _Unused = HANDLE;
