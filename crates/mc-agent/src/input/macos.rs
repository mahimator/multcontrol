//! macOS input backend.
//!
//! Capture is a `CGEventTap` at the HID layer. An active tap can return `NULL`
//! to swallow an event, which is how we stop the Mac's own keyboard from
//! double-typing while the PC is driving it. Injection posts synthetic events
//! back at the HID layer.
//!
//! Both directions require the agent to be granted *Accessibility* permission
//! once, in System Settings. That is a per-user privacy grant, not an
//! administrator or firewall change.

use super::{CaptureEvent, CaptureOptions};
use anyhow::{Context, Result};
use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGMouseButton, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use mc_core::key::{mods, Key};
use mc_core::proto::{Edge, KeyAction, MouseButton};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Marks every event we post, so the tap can tell our own injections from
/// physical input and never feeds them back to the peer.
const MAGIC: i64 = 0x4D43_4C54;
const HOTKEY_DEBOUNCE_MS: u64 = 400;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWarpMouseCursorPosition(new_cursor_position: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGEventTapEnable(tap: *mut core::ffi::c_void, enable: bool);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Whether this process may capture and post input events.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn event_source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

fn flags_from_mods(m: u8) -> CGEventFlags {
    let mut flags = CGEventFlags::CGEventFlagNull;
    if m & mods::SHIFT != 0 {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    if m & mods::CTRL != 0 {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if m & mods::ALT != 0 {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if m & mods::META != 0 {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    if m & mods::CAPS != 0 {
        flags |= CGEventFlags::CGEventFlagAlphaShift;
    }
    flags
}

fn mods_from_flags(flags: CGEventFlags) -> u8 {
    let mut m = 0u8;
    if flags.contains(CGEventFlags::CGEventFlagShift) {
        m |= mods::SHIFT;
    }
    if flags.contains(CGEventFlags::CGEventFlagControl) {
        m |= mods::CTRL;
    }
    if flags.contains(CGEventFlags::CGEventFlagAlternate) {
        m |= mods::ALT;
    }
    if flags.contains(CGEventFlags::CGEventFlagCommand) {
        m |= mods::META;
    }
    if flags.contains(CGEventFlags::CGEventFlagAlphaShift) {
        m |= mods::CAPS;
    }
    m
}

fn screen_bounds() -> (i32, i32, i32, i32) {
    let ids = CGDisplay::active_displays().unwrap_or_else(|_| unsafe { vec![CGMainDisplayID()] });
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for id in ids {
        let CGRect { origin, size } = CGDisplay::new(id).bounds();
        min_x = min_x.min(origin.x as i32);
        min_y = min_y.min(origin.y as i32);
        max_x = max_x.max((origin.x + size.width) as i32);
        max_y = max_y.max((origin.y + size.height) as i32);
    }
    if min_x == i32::MAX {
        return (0, 0, 1440, 900);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

struct TapState {
    tx: Sender<CaptureEvent>,
    opts: CaptureOptions,
    suppress: AtomicBool,
    last_hotkey_ms: AtomicU64,
    hotkey_engaged: AtomicBool,
    edge_latched: AtomicBool,
    /// Raw mach port of the tap, so it can be re-enabled after a timeout.
    tap_port: AtomicPtr<core::ffi::c_void>,
    /// Fractional scroll notches carried between events. macOS reports pixel
    /// deltas; the peer expects wheel notches.
    scroll_remainder: Mutex<(f64, f64)>,
    /// Do not treat pointer position as an edge push before this timestamp.
    edge_guard_ms: AtomicU64,
}

impl TapState {
    fn on_event(&self, event_type: CGEventType, event: &CGEvent) -> Option<CGEvent> {
        // Never echo our own injections back to the controller.
        if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == MAGIC {
            return Some(event.clone());
        }
        let remote = self.suppress.load(Ordering::Relaxed);

        match event_type {
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                let port = self.tap_port.load(Ordering::SeqCst);
                if !port.is_null() {
                    unsafe { CGEventTapEnable(port, true) };
                }
                Some(event.clone())
            }
            CGEventType::KeyDown | CGEventType::KeyUp => {
                let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                let Some(key) = Key::from_mac_keycode(code) else {
                    return Some(event.clone());
                };
                let flags = event.get_flags();
                let m = mods_from_flags(flags);
                let down = matches!(event_type, CGEventType::KeyDown);
                let repeat =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;

                let mut hotkey_hit = false;
                if down
                    && !repeat
                    && key == self.opts.hotkey.key
                    && m & self.opts.hotkey.mods == self.opts.hotkey.mods
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
                if !down && key == self.opts.hotkey.key {
                    self.hotkey_engaged.store(false, Ordering::Relaxed);
                }

                if remote && !hotkey_hit {
                    let action = if down {
                        if repeat {
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
                        mods: m,
                    });
                }
                if remote || hotkey_hit || (was_engaged && !key.is_modifier()) {
                    None
                } else {
                    Some(event.clone())
                }
            }
            CGEventType::FlagsChanged => {
                // Modifier changes are never forwarded as key events; the mods
                // byte on every other event already carries the state. While
                // the peer drives this machine they are swallowed too, because
                // a physically held Cmd combined with an injected Q would quit
                // the frontmost app on this Mac.
                if remote {
                    None
                } else {
                    Some(event.clone())
                }
            }
            CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged => {
                let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X);
                let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y);
                let dx = dx.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                let dy = dy.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                if remote {
                    if dx != 0 || dy != 0 {
                        let _ = self.tx.send(CaptureEvent::MouseMove { dx, dy });
                    }
                    return None;
                }
                if (dx != 0 || dy != 0)
                    && self.opts.edge_push
                    && now_ms() >= self.edge_guard_ms.load(Ordering::Relaxed)
                {
                    let loc = event.location();
                    let (x, y, w, h) = screen_bounds();
                    let margin = self.opts.edge_margin;
                    let edge = if dx > 0 && loc.x as i32 >= x + w - margin {
                        Some(Edge::Right)
                    } else if dx < 0 && loc.x as i32 <= x + margin {
                        Some(Edge::Left)
                    } else if dy > 0 && loc.y as i32 >= y + h - margin {
                        Some(Edge::Bottom)
                    } else if dy < 0 && loc.y as i32 <= y + margin {
                        Some(Edge::Top)
                    } else {
                        None
                    };
                    match edge {
                        Some(e) if !self.edge_latched.swap(true, Ordering::Relaxed) => {
                            let _ = self.tx.send(CaptureEvent::Edge(e));
                        }
                        None => self.edge_latched.store(false, Ordering::Relaxed),
                        _ => {}
                    }
                }
                Some(event.clone())
            }
            CGEventType::LeftMouseDown
            | CGEventType::LeftMouseUp
            | CGEventType::RightMouseDown
            | CGEventType::RightMouseUp
            | CGEventType::OtherMouseDown
            | CGEventType::OtherMouseUp => {
                let button = match event_type {
                    CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => MouseButton::Left,
                    CGEventType::RightMouseDown | CGEventType::RightMouseUp => MouseButton::Right,
                    _ => match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER)
                    {
                        3 => MouseButton::Extra1,
                        4 => MouseButton::Extra2,
                        _ => MouseButton::Middle,
                    },
                };
                let pressed = matches!(
                    event_type,
                    CGEventType::LeftMouseDown
                        | CGEventType::RightMouseDown
                        | CGEventType::OtherMouseDown
                );
                if remote {
                    let _ = self.tx.send(CaptureEvent::MouseButton { button, pressed });
                }
                if remote {
                    None
                } else {
                    Some(event.clone())
                }
            }
            CGEventType::ScrollWheel => {
                let dy = event
                    .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1);
                let dx = event
                    .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2);
                if remote {
                    // One notch is about ten points of pixel delta. Carry the
                    // remainder so smooth trackpad scrolling is not lost.
                    let mut rem = self.scroll_remainder.lock().unwrap();
                    rem.0 += dx as f64 / 10.0;
                    rem.1 += dy as f64 / 10.0;
                    let nx = rem.0.trunc();
                    let ny = rem.1.trunc();
                    rem.0 -= nx;
                    rem.1 -= ny;
                    if nx != 0.0 || ny != 0.0 {
                        let _ = self.tx.send(CaptureEvent::Wheel {
                            dx: nx.clamp(-32768.0, 32767.0) as i16,
                            dy: ny.clamp(-32768.0, 32767.0) as i16,
                        });
                    }
                    None
                } else {
                    Some(event.clone())
                }
            }
            _ => Some(event.clone()),
        }
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// macOS implementation of the input backend.
pub struct MacBackend {
    /// Created in `start`, once the event channel exists. `None` before then.
    state: Mutex<Option<Arc<TapState>>>,
    /// Pointer position as we last left it, used for relative injection.
    last_pos: Mutex<(i32, i32)>,
    cursor_hidden: AtomicBool,
    started: AtomicBool,
    tap_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Held modifier keys, so injected events carry correct flags.
    held: Mutex<Vec<Key>>,
    /// Mouse buttons currently held, so movement is posted as a drag.
    held_buttons: Mutex<Vec<MouseButton>>,
}

impl MacBackend {
    /// Create the backend. Fails if Accessibility permission is missing.
    pub fn new() -> Result<MacBackend> {
        if !accessibility_trusted() {
            log::warn!(
                "Accessibility permission not granted yet: capture and injection will not work"
            );
        }
        let (x, y) = current_pointer();
        Ok(MacBackend {
            state: Mutex::new(None),
            last_pos: Mutex::new((x, y)),
            cursor_hidden: AtomicBool::new(false),
            started: AtomicBool::new(false),
            tap_thread: Mutex::new(None),
            held: Mutex::new(Vec::new()),
            held_buttons: Mutex::new(Vec::new()),
        })
    }

    fn tap_state(&self) -> Option<Arc<TapState>> {
        self.state.lock().unwrap().clone()
    }

    fn post_key(&self, action: KeyAction, key: Key, m: u8) {
        let Some(code) = key.mac_injection_keycode() else {
            return;
        };
        let Some(source) = event_source() else {
            return;
        };
        let down = action != KeyAction::Up;
        let Ok(event) = CGEvent::new_keyboard_event(source, code, down) else {
            return;
        };
        event.set_flags(flags_from_mods(m));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
        event.post(CGEventTapLocation::HID);
    }

    fn post_mouse(&self, event_type: CGEventType, point: CGPoint, button: CGMouseButton) {
        let Some(source) = event_source() else {
            return;
        };
        let Ok(event) = CGEvent::new_mouse_event(source, event_type, point, button) else {
            return;
        };
        // Clicks and drags must carry the modifiers the user is holding on the
        // controller, otherwise Shift-select on the Mac silently misbehaves.
        let m = super::mods_from_keys(&self.held.lock().unwrap(), false);
        event.set_flags(flags_from_mods(m));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
        event.post(CGEventTapLocation::HID);
    }
}

impl super::Backend for MacBackend {
    fn start(&self, tx: Sender<CaptureEvent>, opts: CaptureOptions) -> Result<()> {
        // Build the live tap state and publish it, so suppression decisions
        // from the control loop reach the tap callback.
        let state = Arc::new(TapState {
            tx,
            opts,
            suppress: AtomicBool::new(false),
            last_hotkey_ms: AtomicU64::new(0),
            hotkey_engaged: AtomicBool::new(false),
            edge_latched: AtomicBool::new(false),
            tap_port: AtomicPtr::new(std::ptr::null_mut()),
            scroll_remainder: Mutex::new((0.0, 0.0)),
            edge_guard_ms: AtomicU64::new(0),
        });
        *self.state.lock().unwrap() = Some(state.clone());
        let thread_state = state.clone();
        let handle = std::thread::Builder::new()
            .name("mc-input-mac".into())
            .spawn(move || {
                let events = vec![
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                    CGEventType::FlagsChanged,
                    CGEventType::MouseMoved,
                    CGEventType::LeftMouseDown,
                    CGEventType::LeftMouseUp,
                    CGEventType::RightMouseDown,
                    CGEventType::RightMouseUp,
                    CGEventType::OtherMouseDown,
                    CGEventType::OtherMouseUp,
                    CGEventType::LeftMouseDragged,
                    CGEventType::RightMouseDragged,
                    CGEventType::OtherMouseDragged,
                    CGEventType::ScrollWheel,
                ];
                // Accessibility can be granted after the agent has started
                // (that is the normal first-run order: launch, then approve).
                // Wait for it instead of requiring a restart.
                let mut warned = false;
                let tap = loop {
                    if accessibility_trusted() {
                        let cb_state = thread_state.clone();
                        match CGEventTap::new(
                            CGEventTapLocation::HID,
                            CGEventTapPlacement::HeadInsertEventTap,
                            CGEventTapOptions::Default,
                            events.clone(),
                            move |_proxy: CGEventTapProxy,
                                  event_type: CGEventType,
                                  event: &CGEvent| {
                                cb_state.on_event(event_type, event)
                            },
                        ) {
                            Ok(tap) => break tap,
                            Err(()) => {
                                if !warned {
                                    warned = true;
                                    log::warn!(
                                        "could not create the input tap yet; retrying quietly"
                                    );
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2));
                };
                tap.enable();
                thread_state.tap_port.store(
                    tap.mach_port.as_concrete_TypeRef() as *mut core::ffi::c_void,
                    Ordering::SeqCst,
                );
                let source = match tap.mach_port.create_runloop_source(0) {
                    Ok(s) => s,
                    Err(()) => {
                        log::error!("could not create run loop source for event tap");
                        return;
                    }
                };
                let run_loop = CFRunLoop::get_current();
                unsafe { run_loop.add_source(&source, kCFRunLoopCommonModes) };
                log::info!("event tap installed");
                CFRunLoop::run_current();
            })
            .context("spawning event tap thread")?;
        *self.tap_thread.lock().unwrap() = Some(handle);

        // Give the tap a moment to come up so callers can rely on it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn set_suppress(&self, suppress: bool) {
        if let Some(state) = self.tap_state() {
            state.suppress.store(suppress, Ordering::SeqCst);
            if !suppress {
                state.edge_latched.store(false, Ordering::SeqCst);
                // Parking the pointer on an edge right now must not look like
                // the user pushing through it.
                state.edge_guard_ms.store(now_ms() + 350, Ordering::SeqCst);
            }
        }
    }

    fn inject_key(&self, action: KeyAction, key: Key, m: u8) {
        {
            let mut held = self.held.lock().unwrap();
            match action {
                KeyAction::Down | KeyAction::Repeat => {
                    if !held.contains(&key) {
                        held.push(key);
                    }
                }
                KeyAction::Up => held.retain(|k| *k != key),
            }
        }
        self.post_key(action, key, m);
    }

    fn inject_move(&self, dx: i16, dy: i16) {
        let mut pos = self.last_pos.lock().unwrap();
        let (bx, by, bw, bh) = screen_bounds();
        let nx = (pos.0 + dx as i32).clamp(bx, bx + bw - 1);
        let ny = (pos.1 + dy as i32).clamp(by, by + bh - 1);
        if (nx, ny) != *pos {
            // With a button held the event must be a drag, otherwise the
            // receiving application sees a plain move and drops the drag.
            let held = self.held_buttons.lock().unwrap();
            let event_type = if held.contains(&MouseButton::Left) {
                CGEventType::LeftMouseDragged
            } else if held.contains(&MouseButton::Right) {
                CGEventType::RightMouseDragged
            } else if held.is_empty() {
                CGEventType::MouseMoved
            } else {
                CGEventType::OtherMouseDragged
            };
            drop(held);
            self.post_mouse(
                event_type,
                CGPoint::new(nx as f64, ny as f64),
                CGMouseButton::Left,
            );
        }
        *pos = (nx, ny);
    }

    fn inject_button(&self, button: MouseButton, pressed: bool) {
        let (event_type, cg_button) = match (button, pressed) {
            (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
            (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
            (MouseButton::Middle, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
            (MouseButton::Middle, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
            (MouseButton::Extra1, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
            (MouseButton::Extra1, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
            (MouseButton::Extra2, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
            (MouseButton::Extra2, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
        };
        {
            let mut held = self.held_buttons.lock().unwrap();
            if pressed {
                if !held.contains(&button) {
                    held.push(button);
                }
            } else {
                held.retain(|b| *b != button);
            }
        }
        let (x, y) = *self.last_pos.lock().unwrap();
        let Some(source) = event_source() else {
            return;
        };
        let Ok(event) = CGEvent::new_mouse_event(
            source,
            event_type,
            CGPoint::new(x as f64, y as f64),
            cg_button,
        ) else {
            return;
        };
        // "Other" buttons: 3 and 4 for the two side buttons.
        if matches!(button, MouseButton::Extra1 | MouseButton::Extra2) {
            let number = if button == MouseButton::Extra1 { 3 } else { 4 };
            event.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, number);
        }
        let m = super::mods_from_keys(&self.held.lock().unwrap(), false);
        event.set_flags(flags_from_mods(m));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
        event.post(CGEventTapLocation::HID);
    }

    fn inject_wheel(&self, dx: i16, dy: i16) {
        let Some(source) = event_source() else {
            return;
        };
        let Ok(event) = CGEvent::new_scroll_event(
            source,
            core_graphics::event::ScrollEventUnit::LINE,
            2,
            dy as i32,
            dx as i32,
            0,
        ) else {
            return;
        };
        let m = super::mods_from_keys(&self.held.lock().unwrap(), false);
        event.set_flags(flags_from_mods(m));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC);
        event.post(CGEventTapLocation::HID);
    }

    fn place_pointer_at_edge(&self, edge: Edge) {
        let (bx, by, bw, bh) = screen_bounds();
        let (x, y) = match edge {
            Edge::Left => (bx + 1, by + bh / 2),
            Edge::Right => (bx + bw - 2, by + bh / 2),
            Edge::Top => (bx + bw / 2, by + 1),
            Edge::Bottom => (bx + bw / 2, by + bh - 2),
        };
        unsafe {
            CGWarpMouseCursorPosition(CGPoint::new(x as f64, y as f64));
            CGAssociateMouseAndMouseCursorPosition(true);
        }
        *self.last_pos.lock().unwrap() = (x, y);
        if let Some(state) = self.tap_state() {
            // The warp itself arrives as a mouse move; ignore it for edge
            // purposes for a moment.
            state.edge_guard_ms.store(now_ms() + 350, Ordering::SeqCst);
            state.edge_latched.store(false, Ordering::SeqCst);
        }
    }

    fn pointer_position(&self) -> (i32, i32) {
        // The physical pointer, so edge detection is not fooled by a stale
        // cache after the user moved the mouse locally.
        current_pointer()
    }

    fn screen_bounds(&self) -> (i32, i32, i32, i32) {
        screen_bounds()
    }

    fn set_cursor_visible(&self, visible: bool) {
        let hidden = self.cursor_hidden.load(Ordering::SeqCst);
        if visible && hidden {
            unsafe {
                CGDisplayShowCursor(CGMainDisplayID());
            }
            self.cursor_hidden.store(false, Ordering::SeqCst);
        } else if !visible && !hidden {
            unsafe {
                CGDisplayHideCursor(CGMainDisplayID());
            }
            self.cursor_hidden.store(true, Ordering::SeqCst);
        }
    }
}

fn current_pointer() -> (i32, i32) {
    let Some(source) = event_source() else {
        return (0, 0);
    };
    match CGEvent::new(source) {
        Ok(event) => {
            let p = event.location();
            (p.x as i32, p.y as i32)
        }
        Err(()) => (0, 0),
    }
}

/// Nothing to tear down explicitly; the process exit removes the tap.
pub fn shutdown() {}
