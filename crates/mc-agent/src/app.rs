//! The control loop: the one place that decides where input goes.
//!
//! Exactly one machine is a *controller* (it owns the physical keyboard and
//! mouse) and one is a *target*. The controller is authoritative: it announces
//! every change with [`ControlMsg::State`]. The target may ask for the input
//! back with [`ControlMsg::Request`] (its own hotkey), or by pushing the
//! pointer into a screen edge, which is what makes the two machines feel like
//! one desktop.

use crate::clipboard;
use crate::control_socket::Command;
use crate::input::{opposite, Backend, CaptureEvent};
use crate::net::NetEvent;
use mc_core::config::{Config, ControlRole};
use mc_core::hotkey::Hotkey;
use mc_core::proto::{ControlMsg, Edge, Frame, InputEvent, KeyAction, MouseButton, Target};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Clipboard text larger than this is not synchronised; it would be slow and
/// would not fit comfortably in a single frame.
pub const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;

/// Everything the control loop reacts to.
pub enum Event {
    /// Local input was captured.
    Capture(CaptureEvent),
    /// Something happened on the network.
    Net(NetEvent),
    /// The local clipboard changed.
    Clipboard(String),
    /// A local control-socket command.
    Command(Command),
    /// Periodic tick, used to keep the connection honest.
    Heartbeat,
}

/// A snapshot for the UI and the `status` command.
#[derive(Clone, Debug)]
pub struct Status {
    /// Controller or target.
    pub role: ControlRole,
    /// Whether the peer is connected.
    pub connected: bool,
    /// Peer machine name, once known.
    pub peer: Option<String>,
    /// Where input currently goes (controller's view).
    pub target: Target,
    /// The configured hotkey.
    pub hotkey: String,
    /// Whether the peer is currently driving this machine (target's view).
    pub peer_controls_us: bool,
}

impl Status {
    /// One-line summary.
    pub fn line(&self) -> String {
        let mode = match self.role {
            ControlRole::Controller => match self.target {
                Target::Local => "this PC",
                Target::Remote => "the Mac",
            },
            ControlRole::Target => {
                if self.peer_controls_us {
                    "controlled by the PC"
                } else {
                    "idle"
                }
            }
        };
        let link = match (&self.peer, self.connected) {
            (_, false) => "offline".to_string(),
            (Some(name), true) => format!("connected to {name}"),
            (None, true) => "connected".to_string(),
        };
        format!(
            "{} | input -> {} | {} | hotkey {}",
            match self.role {
                ControlRole::Controller => "controller",
                ControlRole::Target => "target",
            },
            mode,
            link,
            self.hotkey
        )
    }
}

/// The control loop state.
pub struct Agent {
    cfg: Config,
    backend: Arc<dyn Backend>,
    hotkey: Hotkey,
    rx: Receiver<Event>,
    out: Option<Sender<Frame>>,
    connected: bool,
    peer: Option<String>,
    /// Controller: where input goes.
    focus: Target,
    /// Controller: the edge we last handed off through.
    last_edge: Edge,
    /// Target: whether the peer is driving us.
    peer_controls_us: bool,
    /// Target: debounce for edge hand-back.
    edge_latched: bool,
    last_clipboard_sent: String,
    last_clipboard_received: String,
    /// Keys/buttons currently held by the peer, in press order, so a hand-back
    /// can release them explicitly instead of leaving modifiers stuck.
    forwarded_keys: Vec<mc_core::key::Key>,
    forwarded_buttons: Vec<MouseButton>,
    /// Keys/buttons we injected into this machine, in press order.
    injected_keys: Vec<mc_core::key::Key>,
    injected_buttons: Vec<MouseButton>,
    status: Status,
    on_status: Arc<dyn Fn(Status) + Send + Sync>,
}

impl Agent {
    /// Build the control loop.
    pub fn new(
        cfg: Config,
        backend: Arc<dyn Backend>,
        hotkey: Hotkey,
        rx: Receiver<Event>,
        on_status: Arc<dyn Fn(Status) + Send + Sync>,
    ) -> Agent {
        let status = Status {
            role: cfg.control.role,
            connected: false,
            peer: None,
            target: Target::Local,
            hotkey: hotkey.to_string(),
            peer_controls_us: false,
        };
        Agent {
            cfg,
            backend,
            hotkey,
            rx,
            out: None,
            connected: false,
            peer: None,
            focus: Target::Local,
            last_edge: Edge::Right,
            peer_controls_us: false,
            edge_latched: false,
            last_clipboard_sent: String::new(),
            last_clipboard_received: String::new(),
            forwarded_keys: Vec::new(),
            forwarded_buttons: Vec::new(),
            injected_keys: Vec::new(),
            injected_buttons: Vec::new(),
            status,
            on_status,
        }
    }

    /// Start capture hooks. Called after construction so the sender exists.
    pub fn start_capture(&self, tx: Sender<Event>) -> anyhow::Result<()> {
        let (capture_tx, capture_rx) = std::sync::mpsc::channel();
        // Bridge capture events into the single control channel.
        std::thread::Builder::new()
            .name("mc-capture-bridge".into())
            .spawn(move || {
                for event in capture_rx {
                    if tx.send(Event::Capture(event)).is_err() {
                        break;
                    }
                }
            })?;
        self.backend.start(
            capture_tx,
            crate::input::CaptureOptions {
                hotkey: self.hotkey,
                edge_push: self.cfg.control.edge_push,
                edge_margin: self.cfg.control.edge_margin,
            },
        )
    }

    /// Run until shutdown.
    pub fn run(mut self) {
        self.emit_status();
        while let Ok(event) = self.rx.recv() {
            let quit = match event {
                Event::Capture(capture) => {
                    self.on_capture(capture);
                    false
                }
                Event::Net(net) => {
                    self.on_net(net);
                    false
                }
                Event::Clipboard(text) => {
                    self.on_local_clipboard(text);
                    false
                }
                Event::Command(command) => self.on_command(command),
                Event::Heartbeat => {
                    // Any write failure is what tells us the peer vanished; a
                    // silent half-open connection would otherwise never fail.
                    if self.connected {
                        self.send(Frame::Control(ControlMsg::Ping));
                    }
                    false
                }
            };
            if quit {
                break;
            }
        }
        log::info!("control loop stopped");
    }

    fn on_capture(&mut self, event: CaptureEvent) {
        if self.cfg.control.role == ControlRole::Target {
            if let CaptureEvent::Hotkey = event {
                self.request_from_peer();
            }
            return;
        }
        match event {
            CaptureEvent::Hotkey => self.toggle(),
            CaptureEvent::Edge(edge) => {
                if self.focus == Target::Local && self.connected {
                    self.switch_to_remote(edge);
                }
            }
            CaptureEvent::Key { action, key, mods } => {
                if self.focus == Target::Remote {
                    match action {
                        KeyAction::Down | KeyAction::Repeat => {
                            if !self.forwarded_keys.contains(&key) {
                                self.forwarded_keys.push(key);
                            }
                        }
                        KeyAction::Up => self.forwarded_keys.retain(|k| *k != key),
                    }
                    self.send_input(InputEvent::Key {
                        action: if action == KeyAction::Repeat {
                            KeyAction::Down
                        } else {
                            action
                        },
                        key: key.id(),
                        mods,
                    });
                }
            }
            CaptureEvent::MouseMove { dx, dy } => {
                if self.focus == Target::Remote {
                    self.send_input(InputEvent::MouseMove { dx, dy });
                }
            }
            CaptureEvent::MouseButton { button, pressed } => {
                if self.focus == Target::Remote {
                    if pressed {
                        if !self.forwarded_buttons.contains(&button) {
                            self.forwarded_buttons.push(button);
                        }
                    } else {
                        self.forwarded_buttons.retain(|b| *b != button);
                    }
                    self.send_input(InputEvent::MouseButton { button, pressed });
                }
            }
            CaptureEvent::Wheel { dx, dy } => {
                if self.focus == Target::Remote {
                    self.send_input(InputEvent::Wheel { dx, dy });
                }
            }
        }
    }

    fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Connected { info, tx } => {
                self.out = Some(tx);
                self.connected = true;
                self.peer = Some(info.name.clone());
                log::info!("connected to {} ({})", info.name, info.platform);
                if self.cfg.control.role == ControlRole::Controller {
                    // We are authoritative: bring the peer in sync.
                    self.send(Frame::Control(ControlMsg::State { target: self.focus }));
                } else {
                    // The target learns where input is from the next State
                    // frame; until then it must not inject anything.
                    self.apply_peer_control(false);
                }
                self.emit_status();
            }
            NetEvent::Disconnected { reason } => {
                log::warn!("peer disconnected: {reason}");
                self.connected = false;
                if self.focus == Target::Remote {
                    self.switch_to_local(None, false);
                }
                self.release_forwarded_input();
                self.out = None;
                self.peer = None;
                self.apply_peer_control(false);
                self.emit_status();
            }
            NetEvent::Frame(frame) => self.on_frame(frame),
        }
    }

    fn on_frame(&mut self, frame: Frame) {
        match frame {
            Frame::Control(msg) => match msg {
                ControlMsg::State { target } => {
                    if self.cfg.control.role == ControlRole::Target {
                        self.apply_peer_control(target == Target::Remote);
                    }
                }
                ControlMsg::Request { target } => {
                    if self.cfg.control.role == ControlRole::Controller {
                        match target {
                            Target::Remote => {
                                let edge = self.nearest_edge();
                                self.switch_to_remote(edge);
                            }
                            Target::Local => self.switch_to_local(Some(self.last_edge), true),
                        }
                    }
                }
                ControlMsg::Edge { edge } => {
                    if self.cfg.control.role == ControlRole::Controller
                        && self.focus == Target::Remote
                    {
                        log::info!("peer pushed past its {edge:?} edge; taking input back");
                        self.switch_to_local(Some(edge), true);
                    }
                }
                ControlMsg::Clipboard { text } => {
                    if text.len() > MAX_CLIPBOARD_BYTES {
                        log::info!("clipboard text too large to apply ({} bytes)", text.len());
                        return;
                    }
                    self.last_clipboard_received = text.clone();
                    clipboard::set_text(&text);
                    log::info!("clipboard synced ({} chars)", text.chars().count());
                    if self.cfg.control.role == ControlRole::Target
                        && self.peer_controls_us
                        && self.cfg.input.auto_paste
                    {
                        self.backend.inject_paste();
                    }
                }
                ControlMsg::Ping => self.send(Frame::Control(ControlMsg::Pong)),
                ControlMsg::Pong => {}
                ControlMsg::Hello { .. } | ControlMsg::HelloOk { .. } => {}
                ControlMsg::Bye => {
                    log::info!("peer said goodbye");
                    // Treat it exactly like a dropped connection: give the
                    // local user their keyboard back immediately.
                    self.on_net(NetEvent::Disconnected {
                        reason: "peer said goodbye".to_string(),
                    });
                }
            },
            Frame::Input(input) => {
                if self.cfg.control.role == ControlRole::Target && self.peer_controls_us {
                    self.deliver(input);
                }
            }
        }
    }

    fn on_local_clipboard(&mut self, text: String) {
        if !self.cfg.input.sync_clipboard || text.is_empty() {
            return;
        }
        if text.len() > MAX_CLIPBOARD_BYTES {
            log::debug!("clipboard text too large to sync ({} bytes)", text.len());
            return;
        }
        if text == self.last_clipboard_sent || text == self.last_clipboard_received {
            return;
        }
        self.last_clipboard_sent = text.clone();
        self.send(Frame::Control(ControlMsg::Clipboard { text }));
    }

    fn on_command(&mut self, command: Command) -> bool {
        match command {
            Command::Toggle => match self.cfg.control.role {
                ControlRole::Controller => self.toggle(),
                ControlRole::Target => self.request_from_peer(),
            },
            Command::Local => match self.cfg.control.role {
                ControlRole::Controller => self.switch_to_local(None, true),
                ControlRole::Target => self.send(Frame::Control(ControlMsg::Request {
                    target: Target::Local,
                })),
            },
            Command::Remote => match self.cfg.control.role {
                ControlRole::Controller => {
                    let edge = self.nearest_edge();
                    self.switch_to_remote(edge);
                }
                ControlRole::Target => self.send(Frame::Control(ControlMsg::Request {
                    target: Target::Remote,
                })),
            },
            Command::Status => {}
            Command::Quit => {
                log::info!("shutting down on request");
                self.send(Frame::Control(ControlMsg::Bye));
                self.backend.set_suppress(false);
                self.backend.set_cursor_visible(true);
                return true;
            }
        }
        false
    }

    fn toggle(&mut self) {
        match self.focus {
            Target::Local => {
                let edge = self.nearest_edge();
                self.switch_to_remote(edge);
            }
            Target::Remote => {
                let edge = self.last_edge;
                self.switch_to_local(Some(edge), true);
            }
        }
    }

    fn switch_to_remote(&mut self, edge: Edge) {
        if !self.connected {
            log::warn!("hotkey pressed but the Mac is not connected yet");
            return;
        }
        if self.focus == Target::Remote {
            return;
        }
        log::info!("input -> Mac (exited through the {edge:?} edge)");
        self.focus = Target::Remote;
        self.last_edge = edge;
        self.backend.set_suppress(true);
        if self.cfg.input.hide_cursor_when_remote {
            self.backend.set_cursor_visible(false);
        }
        self.send(Frame::Control(ControlMsg::State {
            target: Target::Remote,
        }));
        // Land the peer's pointer on the opposite edge so movement continues
        // naturally in the same direction.
        self.send_input(InputEvent::PointerEdge(opposite(edge)));
        self.push_clipboard();
        self.emit_status();
    }

    fn switch_to_local(&mut self, from_edge: Option<Edge>, place_pointer: bool) {
        if self.focus == Target::Local {
            return;
        }
        log::info!("input -> this machine");
        self.release_forwarded_input();
        self.focus = Target::Local;
        self.backend.set_suppress(false);
        self.backend.set_cursor_visible(true);
        self.send(Frame::Control(ControlMsg::State {
            target: Target::Local,
        }));
        if let (true, Some(edge)) = (place_pointer, from_edge) {
            // The peer exited through `edge`, so we resume on the far side.
            self.backend.place_pointer_at_edge(opposite(edge));
        }
        self.push_clipboard();
        self.emit_status();
    }

    fn push_clipboard(&mut self) {
        if !self.cfg.input.sync_clipboard {
            return;
        }
        if let Some(text) = clipboard::get_text() {
            if !text.is_empty()
                && text != self.last_clipboard_sent
                && text != self.last_clipboard_received
            {
                self.last_clipboard_sent = text.clone();
                self.send(Frame::Control(ControlMsg::Clipboard { text }));
            }
        }
    }

    fn request_from_peer(&mut self) {
        if !self.connected {
            return;
        }
        let target = if self.peer_controls_us {
            Target::Local
        } else {
            Target::Remote
        };
        log::info!("asking the controller to move input to {target:?}");
        self.send(Frame::Control(ControlMsg::Request { target }));
    }

    fn apply_peer_control(&mut self, active: bool) {
        if self.cfg.control.role != ControlRole::Target {
            return;
        }
        if self.peer_controls_us == active {
            return;
        }
        if !active {
            // The peer may have handed back in the middle of a chord; release
            // anything still held so this machine is not left with a stuck
            // modifier or button.
            self.release_injected_input();
        }
        self.peer_controls_us = active;
        self.backend.set_suppress(active);
        // The target's pointer always stays visible: it is the screen the user
        // is looking at.
        self.backend.set_cursor_visible(true);
        if !active {
            self.edge_latched = false;
        }
        log::info!(
            "peer {} controlling this machine",
            if active { "is now" } else { "stopped" }
        );
        self.emit_status();
    }

    fn deliver(&mut self, input: InputEvent) {
        match input {
            InputEvent::Key { action, key, mods } => {
                if let Some(key) = mc_core::key::Key::from_id(key) {
                    match action {
                        KeyAction::Down | KeyAction::Repeat => {
                            if !self.injected_keys.contains(&key) {
                                self.injected_keys.push(key);
                            }
                        }
                        KeyAction::Up => self.injected_keys.retain(|k| *k != key),
                    }
                    self.backend.inject_key(action, key, mods);
                }
            }
            InputEvent::MouseMove { dx, dy } => self.deliver_move(dx, dy),
            InputEvent::MouseButton { button, pressed } => {
                if pressed {
                    if !self.injected_buttons.contains(&button) {
                        self.injected_buttons.push(button);
                    }
                } else {
                    self.injected_buttons.retain(|b| *b != button);
                }
                self.backend.inject_button(button, pressed)
            }
            InputEvent::Wheel { dx, dy } => self.backend.inject_wheel(dx, dy),
            InputEvent::PointerEdge(edge) => {
                self.backend.place_pointer_at_edge(edge);
                self.edge_latched = false;
            }
        }
    }

    fn deliver_move(&mut self, dx: i16, dy: i16) {
        if self.cfg.control.edge_push {
            let (x, y) = self.backend.pointer_position();
            let (bx, by, bw, bh) = self.backend.screen_bounds();
            let nx = x + dx as i32;
            let ny = y + dy as i32;
            let margin = self.cfg.control.edge_margin.max(1);
            let edge = if dx > 0 && nx >= bx + bw - margin {
                Some(Edge::Right)
            } else if dx < 0 && nx <= bx + margin {
                Some(Edge::Left)
            } else if dy > 0 && ny >= by + bh - margin {
                Some(Edge::Bottom)
            } else if dy < 0 && ny <= by + margin {
                Some(Edge::Top)
            } else {
                None
            };
            if let Some(edge) = edge {
                if !self.edge_latched {
                    self.edge_latched = true;
                    log::info!("pointer hit the {edge:?} edge; handing control back");
                    self.send(Frame::Control(ControlMsg::Edge { edge }));
                }
                // Stop at the edge rather than wrapping the pointer around.
                return;
            }
            self.edge_latched = false;
        }
        self.backend.inject_move(dx, dy);
    }

    fn nearest_edge(&self) -> Edge {
        let (x, y) = self.backend.pointer_position();
        let (bx, by, bw, bh) = self.backend.screen_bounds();
        let left = (x - bx).abs();
        let right = (bx + bw - x).abs();
        let top = (y - by).abs();
        let bottom = (by + bh - y).abs();
        let min = left.min(right).min(top).min(bottom);
        if min == right {
            Edge::Right
        } else if min == left {
            Edge::Left
        } else if min == bottom {
            Edge::Bottom
        } else {
            Edge::Top
        }
    }

    /// Release every key/button the controller forwarded to the peer. Sent
    /// before the "you are local again" state frame, so it is applied while the
    /// peer is still in control.
    fn release_forwarded_input(&mut self) {
        let keys: Vec<_> = self.forwarded_keys.drain(..).rev().collect();
        for key in keys {
            self.send_input(InputEvent::Key {
                action: KeyAction::Up,
                key: key.id(),
                mods: 0,
            });
        }
        let buttons = std::mem::take(&mut self.forwarded_buttons);
        for button in buttons {
            self.send_input(InputEvent::MouseButton {
                button,
                pressed: false,
            });
        }
    }

    /// Release everything we injected, used when control leaves this machine.
    fn release_injected_input(&mut self) {
        let buttons = std::mem::take(&mut self.injected_buttons);
        for button in buttons {
            self.backend.inject_button(button, false);
        }
        let keys: Vec<_> = self.injected_keys.drain(..).rev().collect();
        for key in keys {
            self.backend.inject_key(KeyAction::Up, key, 0);
        }
    }

    fn send(&self, frame: Frame) {
        if let Some(tx) = &self.out {
            let _ = tx.send(frame);
        }
    }

    fn send_input(&self, input: InputEvent) {
        self.send(Frame::Input(input));
    }

    fn emit_status(&mut self) {
        self.status.connected = self.connected;
        self.status.peer = self.peer.clone();
        self.status.target = self.focus;
        self.status.peer_controls_us = self.peer_controls_us;
        (self.on_status)(self.status.clone());
    }
}

/// Mouse button helper for tests and diagnostics.
#[allow(dead_code)]
fn _all_buttons() -> [MouseButton; 5] {
    [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Extra1,
        MouseButton::Extra2,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{CaptureEvent, CaptureOptions};
    use crate::net::NetEvent;
    use mc_core::config::{ControlConfig, ControlRole, InputConfig, NetworkConfig};
    use mc_core::session::PeerInfo;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct MockBackend {
        suppressed: AtomicBool,
        injected: Mutex<Vec<String>>,
        placed: Mutex<Vec<Edge>>,
        cursor_visible: AtomicBool,
    }

    impl MockBackend {
        fn note(&self, text: impl Into<String>) {
            self.injected.lock().unwrap().push(text.into());
        }

        fn injected(&self) -> Vec<String> {
            self.injected.lock().unwrap().clone()
        }
    }

    impl Backend for MockBackend {
        fn start(&self, _tx: Sender<CaptureEvent>, _opts: CaptureOptions) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_suppress(&self, suppress: bool) {
            self.suppressed.store(suppress, Ordering::SeqCst);
            self.note(format!("suppress:{suppress}"));
        }
        fn inject_key(&self, action: KeyAction, key: mc_core::key::Key, mods: u8) {
            self.note(format!("key:{action:?}:{key:?}:{mods}"));
        }
        fn inject_move(&self, dx: i16, dy: i16) {
            self.note(format!("move:{dx}:{dy}"));
        }
        fn inject_button(&self, button: MouseButton, pressed: bool) {
            self.note(format!("button:{button:?}:{pressed}"));
        }
        fn inject_wheel(&self, dx: i16, dy: i16) {
            self.note(format!("wheel:{dx}:{dy}"));
        }
        fn place_pointer_at_edge(&self, edge: Edge) {
            self.placed.lock().unwrap().push(edge);
            self.note(format!("place:{edge:?}"));
        }
        fn pointer_position(&self) -> (i32, i32) {
            (0, 0)
        }
        fn screen_bounds(&self) -> (i32, i32, i32, i32) {
            (0, 0, 100, 100)
        }
        fn set_cursor_visible(&self, visible: bool) {
            self.cursor_visible.store(visible, Ordering::SeqCst);
        }
    }

    struct Harness {
        events: Sender<Event>,
        from_peer: Receiver<Frame>,
        backend: Arc<MockBackend>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    fn harness(role: ControlRole) -> Harness {
        let backend = Arc::new(MockBackend::default());
        backend.cursor_visible.store(true, Ordering::SeqCst);
        let (events_tx, events_rx) = channel();
        let cfg = Config {
            network: NetworkConfig::default(),
            control: ControlConfig {
                role,
                ..ControlConfig::default()
            },
            input: InputConfig {
                // Keep clipboard access out of the tests.
                sync_clipboard: false,
                ..InputConfig::default()
            },
        };
        let agent = Agent::new(
            cfg,
            backend.clone(),
            Hotkey::parse("ctrl+alt+space").unwrap(),
            events_rx,
            Arc::new(|_status| {}),
        );
        let handle = std::thread::spawn(move || agent.run());
        let (peer_tx, peer_rx) = channel();
        let _ = events_tx.send(Event::Net(NetEvent::Connected {
            info: PeerInfo {
                name: "peer".into(),
                platform: "windows".into(),
                layout: "us".into(),
            },
            tx: peer_tx,
        }));
        Harness {
            events: events_tx,
            from_peer: peer_rx,
            backend,
            handle: Some(handle),
        }
    }

    fn next_frame(h: &Harness) -> Frame {
        h.from_peer
            .recv_timeout(Duration::from_secs(2))
            .expect("expected a frame for the peer")
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(60));
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            drop(std::mem::replace(&mut self.events, channel().0));
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    #[test]
    fn controller_hotkey_hands_control_over_and_back() {
        let h = harness(ControlRole::Controller);
        // Initial sync from the controller.
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Local
            })
        ));

        h.events.send(Event::Capture(CaptureEvent::Hotkey)).unwrap();
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Remote
            })
        ));
        assert!(matches!(
            next_frame(&h),
            Frame::Input(InputEvent::PointerEdge(Edge::Right))
        ));
        assert!(h.backend.suppressed.load(Ordering::SeqCst));

        // Keys and movement now go to the peer, not the local machine.
        h.events
            .send(Event::Capture(CaptureEvent::Key {
                action: KeyAction::Down,
                key: mc_core::key::Key::A,
                mods: 0,
            }))
            .unwrap();
        h.events
            .send(Event::Capture(CaptureEvent::MouseMove { dx: 3, dy: 4 }))
            .unwrap();
        assert!(matches!(
            next_frame(&h),
            Frame::Input(InputEvent::Key { key, .. }) if key == mc_core::key::Key::A.id()
        ));
        assert!(matches!(
            next_frame(&h),
            Frame::Input(InputEvent::MouseMove { dx: 3, dy: 4 })
        ));

        // The peer pushing past its left edge gives control back and parks our
        // pointer on the opposite (right) edge.
        h.events
            .send(Event::Net(NetEvent::Frame(Frame::Control(
                ControlMsg::Edge { edge: Edge::Left },
            ))))
            .unwrap();
        // Keys still held on the PC are released on the Mac first, so nothing
        // is left stuck down.
        assert!(matches!(
            next_frame(&h),
            Frame::Input(InputEvent::Key {
                action: KeyAction::Up,
                ..
            })
        ));
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Local
            })
        ));
        settle();
        assert!(!h.backend.suppressed.load(Ordering::SeqCst));
        assert_eq!(*h.backend.placed.lock().unwrap(), vec![Edge::Right]);
    }

    #[test]
    fn controller_edge_push_switches_to_peer() {
        let h = harness(ControlRole::Controller);
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Local
            })
        ));
        h.events
            .send(Event::Capture(CaptureEvent::Edge(Edge::Bottom)))
            .unwrap();
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Remote
            })
        ));
        // Exiting the bottom hands the peer the top edge.
        assert!(matches!(
            next_frame(&h),
            Frame::Input(InputEvent::PointerEdge(Edge::Top))
        ));
    }

    #[test]
    fn target_only_injects_when_the_peer_is_in_control() {
        let h = harness(ControlRole::Target);
        let input = Frame::Input(InputEvent::Key {
            action: KeyAction::Down,
            key: mc_core::key::Key::B.id(),
            mods: 0,
        });
        // Not in control yet: input must be ignored.
        h.events
            .send(Event::Net(NetEvent::Frame(input.clone())))
            .unwrap();
        settle();
        assert!(h.backend.injected().is_empty());

        // Peer takes control.
        h.events
            .send(Event::Net(NetEvent::Frame(Frame::Control(
                ControlMsg::State {
                    target: Target::Remote,
                },
            ))))
            .unwrap();
        settle();
        assert!(h.backend.suppressed.load(Ordering::SeqCst));

        h.events.send(Event::Net(NetEvent::Frame(input))).unwrap();
        h.events
            .send(Event::Net(NetEvent::Frame(Frame::Input(
                InputEvent::PointerEdge(Edge::Left),
            ))))
            .unwrap();
        settle();
        let injected = h.backend.injected();
        assert!(injected.iter().any(|s| s.starts_with("key:Down:B")));
        assert_eq!(*h.backend.placed.lock().unwrap(), vec![Edge::Left]);

        // Peer releases control.
        h.events
            .send(Event::Net(NetEvent::Frame(Frame::Control(
                ControlMsg::State {
                    target: Target::Local,
                },
            ))))
            .unwrap();
        settle();
        assert!(!h.backend.suppressed.load(Ordering::SeqCst));
    }

    #[test]
    fn target_move_past_the_edge_hands_control_back_once() {
        let h = harness(ControlRole::Target);
        h.events
            .send(Event::Net(NetEvent::Frame(Frame::Control(
                ControlMsg::State {
                    target: Target::Remote,
                },
            ))))
            .unwrap();
        settle();
        // The mock starts at (0,0) and the screen is 100x100, so a negative
        // horizontal delta is already past the left edge.
        for _ in 0..3 {
            h.events
                .send(Event::Net(NetEvent::Frame(Frame::Input(
                    InputEvent::MouseMove { dx: -20, dy: 0 },
                ))))
                .unwrap();
        }
        settle();
        let mut edges = 0;
        while let Ok(frame) = h.from_peer.recv_timeout(Duration::from_millis(50)) {
            if matches!(frame, Frame::Control(ControlMsg::Edge { edge: Edge::Left })) {
                edges += 1;
            }
        }
        assert_eq!(edges, 1, "edge hand-back should be sent exactly once");
    }

    #[test]
    fn disconnect_drops_back_to_local() {
        let h = harness(ControlRole::Controller);
        assert!(matches!(
            next_frame(&h),
            Frame::Control(ControlMsg::State {
                target: Target::Local
            })
        ));
        h.events.send(Event::Capture(CaptureEvent::Hotkey)).unwrap();
        let _ = next_frame(&h); // State{Remote}
        let _ = next_frame(&h); // PointerEdge
        assert!(h.backend.suppressed.load(Ordering::SeqCst));

        h.events
            .send(Event::Net(NetEvent::Disconnected {
                reason: "test".into(),
            }))
            .unwrap();
        settle();
        assert!(!h.backend.suppressed.load(Ordering::SeqCst));
        assert!(h.backend.cursor_visible.load(Ordering::SeqCst));
        // And further hotkeys cannot hand over while offline.
        let before = h.backend.injected().len();
        h.events.send(Event::Capture(CaptureEvent::Hotkey)).unwrap();
        settle();
        assert_eq!(h.backend.injected().len(), before);
    }
}
