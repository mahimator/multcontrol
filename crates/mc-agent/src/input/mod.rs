//! Platform input backends.
//!
//! The agent talks to exactly two platform-specific things:
//!
//! * **capture** — observe and optionally swallow the local keyboard/mouse, and
//! * **injection** — synthesise keyboard/mouse events coming from the peer.
//!
//! Everything else (protocol, switching, clipboard) is platform independent.

use anyhow::Result;
use mc_core::hotkey::Hotkey;
use mc_core::key::{mods, Key};
use mc_core::proto::{Edge, KeyAction, MouseButton};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{accessibility_trusted, prompt_for_accessibility};
#[cfg(windows)]
mod windows;

/// An event produced by the local input capture.
#[derive(Clone, Copy, Debug)]
pub enum CaptureEvent {
    /// A key changed state.
    Key {
        /// Down / up / repeat.
        action: KeyAction,
        /// The key.
        key: Key,
        /// Modifier state at the time of the event.
        mods: u8,
    },
    /// Relative pointer movement, in pixels.
    MouseMove {
        /// Horizontal delta.
        dx: i16,
        /// Vertical delta.
        dy: i16,
    },
    /// A mouse button changed state.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// True for press, false for release.
        pressed: bool,
    },
    /// Scroll wheel movement.
    Wheel {
        /// Horizontal scroll.
        dx: i16,
        /// Vertical scroll.
        dy: i16,
    },
    /// The configured hotkey was pressed.
    Hotkey,
    /// The pointer was pushed past a screen edge.
    Edge(Edge),
}

/// Static capture configuration.
#[derive(Clone, Debug)]
pub struct CaptureOptions {
    /// The chord that toggles control.
    pub hotkey: Hotkey,
    /// Whether pushing the pointer past a screen edge should switch.
    pub edge_push: bool,
    /// How close to the edge counts as "at the edge", in pixels.
    pub edge_margin: i32,
}

/// A platform input backend.
pub trait Backend: Send + Sync {
    /// Install capture hooks. Must be called once.
    fn start(&self, tx: Sender<CaptureEvent>, opts: CaptureOptions) -> Result<()>;

    /// Swallow local input (true) or pass it through (false).
    fn set_suppress(&self, suppress: bool);

    /// Inject a key event.
    fn inject_key(&self, action: KeyAction, key: Key, mods: u8);

    /// Inject relative pointer movement.
    fn inject_move(&self, dx: i16, dy: i16);

    /// Inject a mouse button event.
    fn inject_button(&self, button: MouseButton, pressed: bool);

    /// Inject a scroll event.
    fn inject_wheel(&self, dx: i16, dy: i16);

    /// Move the pointer to a screen edge, used when handing control over.
    fn place_pointer_at_edge(&self, edge: Edge);

    /// Current pointer position in global screen coordinates.
    fn pointer_position(&self) -> (i32, i32);

    /// Bounding box of the virtual desktop: `(x, y, width, height)`.
    fn screen_bounds(&self) -> (i32, i32, i32, i32);

    /// Show or hide the local pointer.
    fn set_cursor_visible(&self, visible: bool);

    /// Post the platform paste shortcut (Ctrl+V / Cmd+V).
    fn inject_paste(&self) {
        #[cfg(target_os = "macos")]
        let paste_mod = mods::META;
        #[cfg(not(target_os = "macos"))]
        let paste_mod = mods::CTRL;

        let modifier = if paste_mod == mods::META {
            Key::LMeta
        } else {
            Key::LCtrl
        };
        self.inject_key(KeyAction::Down, modifier, 0);
        self.inject_key(KeyAction::Down, Key::V, paste_mod);
        self.inject_key(KeyAction::Up, Key::V, paste_mod);
        self.inject_key(KeyAction::Up, modifier, 0);
    }
}

/// The backend for the current platform.
pub fn platform_backend() -> Result<Arc<dyn Backend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(macos::MacBackend::new()?))
    }
    #[cfg(windows)]
    {
        Ok(Arc::new(windows::WindowsBackend::new()?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        anyhow::bail!("multcontrol only supports macOS and Windows hosts")
    }
}

/// Compute the modifier bitmask from a set of pressed keys.
#[allow(dead_code)]
pub fn mods_from_keys(pressed: &[Key], caps: bool) -> u8 {
    let mut m = 0u8;
    for k in pressed {
        m |= match k {
            Key::LShift | Key::RShift => mods::SHIFT,
            Key::LCtrl | Key::RCtrl => mods::CTRL,
            Key::LAlt | Key::RAlt => mods::ALT,
            Key::LMeta | Key::RMeta => mods::META,
            _ => 0,
        };
    }
    if caps {
        m |= mods::CAPS;
    }
    m
}

/// Tear down platform hooks (best effort).
pub fn shutdown() {
    #[cfg(windows)]
    windows::shutdown();
    #[cfg(target_os = "macos")]
    macos::shutdown();
}

/// The edge opposite to `edge`. Handing the pointer across a boundary lands it
/// on the far side of the other screen.
pub fn opposite(edge: Edge) -> Edge {
    match edge {
        Edge::Left => Edge::Right,
        Edge::Right => Edge::Left,
        Edge::Top => Edge::Bottom,
        Edge::Bottom => Edge::Top,
    }
}
