//! Windows system tray icon.
//!
//! Shows a tray icon with a context menu (Toggle / Local / Remote / Quit).
//! Left-clicking the icon toggles control immediately.

use crate::gui::GuiCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TranslateMessage, TrackPopupMenuEx, UnregisterClassW, HMENU, HWND_MESSAGE,
    IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_COMMAND,
    WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER, WNDCLASSW,
};

/// Custom message for tray icon callbacks.
const WM_TRAYICON: u32 = WM_USER + 1;

/// Tray icon id.
const ID_TRAY: u32 = 1;

/// Menu command ids.
const IDM_TOGGLE: usize = 1001;
const IDM_LOCAL: usize = 1002;
const IDM_REMOTE: usize = 1003;
const IDM_QUIT: usize = 1004;

/// The command sender, shared with the window procedure.
static CMD_TX: OnceLock<Mutex<Option<Sender<GuiCommand>>>> = OnceLock::new();

fn cmd_tx() -> &'static Mutex<Option<Sender<GuiCommand>>> {
    CMD_TX.get_or_init(|| Mutex::new(None))
}

/// Build a `PCWSTR` from a Rust string. The buffer must outlive the call, so
/// callers keep the `Vec` alive for the duration.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_TRAYICON => {
                match lparam.0 as u32 {
                    WM_RBUTTONUP => show_menu(hwnd),
                    WM_LBUTTONUP => send_cmd(GuiCommand::Toggle),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                match wparam.0 {
                    IDM_TOGGLE => send_cmd(GuiCommand::Toggle),
                    IDM_LOCAL => send_cmd(GuiCommand::Local),
                    IDM_REMOTE => send_cmd(GuiCommand::Remote),
                    IDM_QUIT => {
                        send_cmd(GuiCommand::Quit);
                        PostQuitMessage(0);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn send_cmd(cmd: GuiCommand) {
    if let Ok(guard) = cmd_tx().lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(cmd);
        }
    }
}

/// Show the context menu at the cursor.
unsafe fn show_menu(hwnd: HWND) {
    let Ok(hmenu) = CreatePopupMenu() else {
        return;
    };
    let toggle = wide("Toggle  (Ctrl+Alt+Space)");
    let local = wide("Use this machine");
    let remote = wide("Use the other machine");
    let quit = wide("Quit");
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_TOGGLE, PCWSTR::from_raw(toggle.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_LOCAL, PCWSTR::from_raw(local.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_REMOTE, PCWSTR::from_raw(remote.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::from_raw(std::ptr::null()));
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_QUIT, PCWSTR::from_raw(quit.as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Required so the menu closes when the user clicks elsewhere.
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenuEx(
        hmenu,
        TPM_LEFTALIGN.0 | TPM_RIGHTBUTTON.0,
        pt.x,
        pt.y,
        hwnd,
        None,
    );
    let _ = DestroyMenu(hmenu);
}

/// Run the tray icon on its own thread. Returns when the message loop ends.
pub fn run(command_tx: Sender<GuiCommand>, running: Arc<AtomicBool>) {
    unsafe {
        if let Ok(mut guard) = cmd_tx().lock() {
            *guard = Some(command_tx);
        }

        let hinstance = GetModuleHandleW(None).expect("module handle");
        let class_name = wide("MultControlTray");
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassW(&wnd_class) == 0 {
            log::error!("failed to register the tray window class");
            return;
        }

        let hwnd = match CreateWindowExW(
            Default::default(),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(class_name.as_ptr()),
            Default::default(),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            HMENU::default(),
            hinstance,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log::error!("failed to create the tray window: {e}");
                return;
            }
        };

        // Add the tray icon.
        let mut nid: NOTIFYICONDATAW = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: ID_TRAY,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            ..Default::default()
        };
        // Prefer the icon embedded in the executable (resource id 1); fall
        // back to the default system application icon if that is missing.
        // MAKEINTRESOURCE(1): the resource id is passed as the pointer value.
        let hinstance: windows::Win32::Foundation::HINSTANCE = hinstance.into();
        let embedded = LoadIconW(hinstance, PCWSTR::from_raw(1 as *const u16));
        match embedded {
            Ok(icon) => nid.hIcon = icon,
            Err(_) => match LoadIconW(windows::Win32::Foundation::HINSTANCE::default(), IDI_APPLICATION) {
                Ok(icon) => nid.hIcon = icon,
                Err(_) => log::warn!("could not load the tray icon image"),
            },
        }
        let tip = wide("MultControl");
        let n = tip.len().min(nid.szTip.len());
        nid.szTip[..n].copy_from_slice(&tip[..n]);

        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            log::error!("failed to add the tray icon");
        }

        // Pump messages until quit.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(PCWSTR::from_raw(class_name.as_ptr()), hinstance);
        running.store(false, Ordering::SeqCst);
    }
}

/// Update the tray tooltip, e.g. to show the connection state.
pub fn set_tooltip(text: &str) {
    unsafe {
        let tip = wide(text);
        let mut nid: NOTIFYICONDATAW = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            uID: ID_TRAY,
            uFlags: NIF_TIP,
            ..Default::default()
        };
        let n = tip.len().min(nid.szTip.len());
        nid.szTip[..n].copy_from_slice(&tip[..n]);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}
