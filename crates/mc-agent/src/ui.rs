//! Status UI.
//!
//! macOS gets a menu bar item, which is the natural place for a background
//! agent and needs no window server privileges. Windows runs headless and
//! reports through the log file and the `status` control command.

use crate::app::Status;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Run the status UI on the calling thread (must be the main thread on macOS).
pub fn run(status_rx: Receiver<Status>, running: Arc<AtomicBool>, trusted: bool) {
    #[cfg(target_os = "macos")]
    macos::run(status_rx, running, trusted);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (status_rx, trusted);
        while running.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
    use mc_core::config::ControlRole;
    use mc_core::proto::Target;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSStatusBar};
    use objc2_foundation::{MainThreadMarker, NSString};

    pub fn run(status_rx: Receiver<Status>, running: Arc<AtomicBool>, trusted: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            log::error!("status UI must run on the main thread");
            return;
        };
        unsafe {
            let app = NSApplication::sharedApplication(mtm);
            let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            let bar = NSStatusBar::systemStatusBar();
            let item = bar.statusItemWithLength(objc2_app_kit::NSVariableStatusItemLength);
            let mut last = String::new();
            let mut trusted = trusted;
            let mut current: Option<Status> = None;
            let mut last_trust_check = std::time::Instant::now();
            set_title(&item, &title(None, trusted));
            while running.load(Ordering::SeqCst) {
                let _ = CFRunLoop::run_in_mode(
                    kCFRunLoopDefaultMode,
                    std::time::Duration::from_millis(200),
                    true,
                );
                // Accessibility can be granted while the agent is running;
                // notice it without making the user restart.
                if last_trust_check.elapsed() >= std::time::Duration::from_secs(2) {
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
                let text = title(current.as_ref(), trusted);
                if text != last {
                    set_title(&item, &text);
                    last = text;
                }
            }
        }
    }

    #[allow(deprecated)]
    fn set_title(item: &objc2_app_kit::NSStatusItem, text: &str) {
        unsafe {
            item.setTitle(Some(&NSString::from_str(text)));
        }
    }

    fn title(status: Option<&Status>, trusted: bool) -> String {
        if !trusted {
            return "MC !".to_string();
        }
        let Some(status) = status else {
            return "MC".to_string();
        };
        if !status.connected {
            return "MC offline".to_string();
        }
        match status.role {
            ControlRole::Controller => match status.target {
                Target::Local => "MC PC".to_string(),
                Target::Remote => "MC Mac".to_string(),
            },
            ControlRole::Target => {
                if status.peer_controls_us {
                    "MC controlled".to_string()
                } else {
                    "MC idle".to_string()
                }
            }
        }
    }
}
