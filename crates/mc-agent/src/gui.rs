//! GUI status/control window for presentations.
//!
//! macOS: status bar item showing "MC PC", "MC Mac", "MC offline", etc.
//! Windows: system tray icon with a right-click context menu (Toggle / Local /
//! Remote / Quit) and left-click to toggle.

use crate::app::Status;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Commands the GUI can send to the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum GuiCommand {
    Toggle,
    Local,
    Remote,
    Quit,
}

/// Run the tray/menu-bar UI.
///
/// On macOS this runs on the main thread within NSApplication.
/// On Windows it spawns a hidden message window with a tray icon.
pub fn run(
    status_rx: Receiver<Status>,
    command_tx: Sender<GuiCommand>,
    running: Arc<AtomicBool>,
) {
    #[cfg(target_os = "macos")]
    macos::run(status_rx, command_tx, running);
    #[cfg(windows)]
    windows::run(status_rx, command_tx, running);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (status_rx, command_tx, running);
        while running.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::gui::GuiCommand;
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    use mc_core::config::ControlRole;
    use mc_core::proto::Target;
    use objc2::ClassType;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSImage, NSStatusBar,
    };
    use objc2_foundation::{MainThreadMarker, NSSize, NSString};
    use std::time::Duration;

    pub fn run(
        status_rx: Receiver<Status>,
        _command_tx: Sender<GuiCommand>,
        running: Arc<AtomicBool>,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            log::error!("status UI must run on the main thread");
            return;
        };
        unsafe {
            let app = NSApplication::sharedApplication(mtm);
            let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            let bar = NSStatusBar::systemStatusBar();
            let item = bar.statusItemWithLength(objc2_app_kit::NSVariableStatusItemLength);

            // Use the app icon as a template image so it follows the menu bar's
            // light/dark appearance, and keep the state in the tooltip.
            if let Some(image) = load_icon() {
                image.setTemplate(true);
                // `setImage` on the status item is deprecated in favour of the
                // button, but the button needs a main-thread marker we do not
                // have here; the item-level call still works.
                #[allow(deprecated)]
                item.setImage(Some(image.as_ref()));
            }

            let mut last = String::new();
            let mut trusted = true;
            let mut current: Option<Status> = None;
            let mut last_trust_check = std::time::Instant::now();
            #[allow(deprecated)]
            item.setToolTip(Some(&NSString::from_str("MultControl")));

            while running.load(Ordering::SeqCst) {
                let _ = CFRunLoop::run_in_mode(
                    kCFRunLoopDefaultMode,
                    Duration::from_millis(200),
                    true,
                );

                // Accessibility can be granted while the agent is running;
                // notice it without making the user restart.
                if last_trust_check.elapsed() >= Duration::from_secs(2) {
                    last_trust_check = std::time::Instant::now();
                    let now_trusted = crate::input::accessibility_trusted();
                    if now_trusted != trusted {
                        trusted = now_trusted;
                        log::info!(
                            "Accessibility permission is now {}",
                            if trusted { "granted" } else { "missing" }
                        );
                        last.clear();
                    }
                }

                while let Ok(status) = status_rx.try_recv() {
                    current = Some(status);
                }

                let text = tooltip(current.as_ref(), trusted);
                if text != last {
                    #[allow(deprecated)]
                    item.setToolTip(Some(&NSString::from_str(&text)));
                    last = text;
                }
            }
        }
    }

    /// Load the app icon, if it is available.
    ///
    /// Normally it lives in the app bundle's `Resources` directory. When the
    /// binary is run directly (during development, or from a checkout) the
    /// bundle is not there, so the neighbouring directories are tried too.
    unsafe fn load_icon() -> Option<objc2::rc::Retained<objc2_app_kit::NSImage>> {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();

        if let Some(bundle) = objc2_foundation::NSBundle::mainBundle().resourcePath() {
            candidates.push(std::path::PathBuf::from(bundle.to_string()));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.to_path_buf());
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join("Resources"));
                    candidates.push(parent.join("MacOS"));
                }
            }
        }

        for dir in candidates {
            let path = dir.join("AppIcon.icns");
            if !path.exists() {
                continue;
            }
            let name = NSString::from_str(&path.to_string_lossy());
            if let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &name) {
                image.setSize(NSSize::new(18.0, 18.0));
                return Some(image);
            }
        }
        None
    }

    /// The status line shown as the menu bar tooltip.
    fn tooltip(status: Option<&Status>, trusted: bool) -> String {
        if !trusted {
            return "MultControl - Accessibility permission missing".to_string();
        }
        let Some(status) = status else {
            return "MultControl - starting".to_string();
        };
        let peer = status.peer.as_deref().unwrap_or("peer");
        let state = if !status.connected {
            "offline".to_string()
        } else {
            match status.role {
                ControlRole::Controller => match status.target {
                    Target::Local => format!("this PC (peer: {peer})"),
                    Target::Remote => format!("the Mac (peer: {peer})"),
                },
                ControlRole::Target => {
                    if status.peer_controls_us {
                        format!("controlled by the PC ({peer})")
                    } else {
                        format!("idle (peer: {peer})")
                    }
                }
            }
        };
        format!("MultControl - {state}")
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::gui::GuiCommand;
    use crate::tray;
    use std::thread;

    pub fn run(
        status_rx: Receiver<Status>,
        command_tx: Sender<GuiCommand>,
        running: Arc<AtomicBool>,
    ) {
        // The tray icon lives on its own thread with its own message loop.
        let tray_running = running.clone();
        let tray_thread = thread::Builder::new()
            .name("mc-tray".into())
            .spawn(move || tray::run(command_tx, tray_running))
            .ok();

        // Keep the tooltip in sync with the connection state.
        while running.load(Ordering::SeqCst) {
            let mut latest: Option<Status> = None;
            while let Ok(status) = status_rx.try_recv() {
                latest = Some(status);
            }
            if let Some(status) = latest {
                tray::set_tooltip(&tooltip(&status));
            }
            thread::sleep(std::time::Duration::from_millis(200));
        }

        if let Some(handle) = tray_thread {
            let _ = handle.join();
        }
    }

    fn tooltip(status: &Status) -> String {
        if !status.connected {
            return "MultControl - offline".to_string();
        }
        let peer = status.peer.as_deref().unwrap_or("peer");
        match status.role {
            mc_core::config::ControlRole::Controller => match status.target {
                mc_core::proto::Target::Local => {
                    format!("MultControl - this PC (peer: {peer})")
                }
                mc_core::proto::Target::Remote => {
                    format!("MultControl - the Mac (peer: {peer})")
                }
            },
            mc_core::config::ControlRole::Target => {
                if status.peer_controls_us {
                    format!("MultControl - controlled by the PC ({peer})")
                } else {
                    format!("MultControl - idle (peer: {peer})")
                }
            }
        }
    }
}
