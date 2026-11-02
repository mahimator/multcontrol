//! Wire protocol.
//!
//! Frames are `[u32 len][u8 kind][payload]`, where `len` is the payload length.
//! Control traffic is JSON (low frequency, easy to debug); input traffic is a
//! compact binary encoding (high frequency, low latency).
//!
//! The only stateful machine is the *controller*: the machine holding the
//! physical keyboard and mouse. It owns the "who gets the input" decision and
//! broadcasts it as [`ControlMsg::State`]. The *target* can ask to take the
//! input back with [`ControlMsg::Request`].

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Frame kinds.
pub mod kind {
    /// JSON control message.
    pub const CONTROL: u8 = 0x01;
    /// Binary key event.
    pub const KEY: u8 = 0x02;
    /// Binary relative mouse movement.
    pub const MOUSE_MOVE: u8 = 0x03;
    /// Binary mouse button event.
    pub const MOUSE_BUTTON: u8 = 0x04;
    /// Binary scroll wheel event.
    pub const WHEEL: u8 = 0x05;
    /// Binary pointer placement (used when handing control over).
    pub const POINTER_EDGE: u8 = 0x06;
}

/// Protocol version, bumped on incompatible changes.
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum accepted frame payload, to bound memory on malformed input.
pub const MAX_FRAME_LEN: u32 = 1 << 20;

/// Which machine currently receives the shared input.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// The controller machine itself.
    Local,
    /// The peer machine.
    Remote,
}

/// Screen edge, used for edge-push switching and pointer placement.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// Numeric encoding used by the binary protocol.
    pub fn code(self) -> u8 {
        match self {
            Edge::Left => 0,
            Edge::Right => 1,
            Edge::Top => 2,
            Edge::Bottom => 3,
        }
    }

    /// Inverse of [`Edge::code`].
    pub fn from_code(code: u8) -> Option<Edge> {
        match code {
            0 => Some(Edge::Left),
            1 => Some(Edge::Right),
            2 => Some(Edge::Top),
            3 => Some(Edge::Bottom),
            _ => None,
        }
    }
}

/// Key press state change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyAction {
    Down,
    Up,
    /// Auto-repeat while held down.
    Repeat,
}

impl KeyAction {
    fn code(self) -> u8 {
        match self {
            KeyAction::Down => 0,
            KeyAction::Up => 1,
            KeyAction::Repeat => 2,
        }
    }

    fn from_code(code: u8) -> Option<KeyAction> {
        match code {
            0 => Some(KeyAction::Down),
            1 => Some(KeyAction::Up),
            2 => Some(KeyAction::Repeat),
            _ => None,
        }
    }
}

/// Mouse button identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Extra1,
    Extra2,
}

impl MouseButton {
    fn code(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
            MouseButton::Extra1 => 3,
            MouseButton::Extra2 => 4,
        }
    }

    fn from_code(code: u8) -> Option<MouseButton> {
        match code {
            0 => Some(MouseButton::Left),
            1 => Some(MouseButton::Right),
            2 => Some(MouseButton::Middle),
            3 => Some(MouseButton::Extra1),
            4 => Some(MouseButton::Extra2),
            _ => None,
        }
    }
}

/// High-rate input events, in their binary form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputEvent {
    Key {
        action: KeyAction,
        key: u16,
        mods: u8,
    },
    MouseMove {
        dx: i16,
        dy: i16,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// Place the pointer at the given screen edge (hand-off positioning).
    PointerEdge(Edge),
}

impl InputEvent {
    fn kind(&self) -> u8 {
        match self {
            InputEvent::Key { .. } => kind::KEY,
            InputEvent::MouseMove { .. } => kind::MOUSE_MOVE,
            InputEvent::MouseButton { .. } => kind::MOUSE_BUTTON,
            InputEvent::Wheel { .. } => kind::WHEEL,
            InputEvent::PointerEdge(_) => kind::POINTER_EDGE,
        }
    }

    fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6);
        match *self {
            InputEvent::Key { action, key, mods } => {
                out.push(action.code());
                out.extend_from_slice(&key.to_le_bytes());
                out.push(mods);
            }
            InputEvent::MouseMove { dx, dy } => {
                out.extend_from_slice(&dx.to_le_bytes());
                out.extend_from_slice(&dy.to_le_bytes());
            }
            InputEvent::MouseButton { button, pressed } => {
                out.push(button.code());
                out.push(u8::from(pressed));
            }
            InputEvent::Wheel { dx, dy } => {
                out.extend_from_slice(&dx.to_le_bytes());
                out.extend_from_slice(&dy.to_le_bytes());
            }
            InputEvent::PointerEdge(edge) => out.push(edge.code()),
        }
        out
    }

    /// Decode a binary input event payload.
    pub fn decode(kind: u8, payload: &[u8]) -> Option<InputEvent> {
        Some(match kind {
            kind::KEY => {
                if payload.len() < 4 {
                    return None;
                }
                InputEvent::Key {
                    action: KeyAction::from_code(payload[0])?,
                    key: u16::from_le_bytes([payload[1], payload[2]]),
                    mods: payload[3],
                }
            }
            kind::MOUSE_MOVE => {
                if payload.len() < 4 {
                    return None;
                }
                InputEvent::MouseMove {
                    dx: i16::from_le_bytes([payload[0], payload[1]]),
                    dy: i16::from_le_bytes([payload[2], payload[3]]),
                }
            }
            kind::MOUSE_BUTTON => {
                if payload.len() < 2 {
                    return None;
                }
                InputEvent::MouseButton {
                    button: MouseButton::from_code(payload[0])?,
                    pressed: payload[1] != 0,
                }
            }
            kind::WHEEL => {
                if payload.len() < 4 {
                    return None;
                }
                InputEvent::Wheel {
                    dx: i16::from_le_bytes([payload[0], payload[1]]),
                    dy: i16::from_le_bytes([payload[2], payload[3]]),
                }
            }
            kind::POINTER_EDGE => {
                if payload.is_empty() {
                    return None;
                }
                InputEvent::PointerEdge(Edge::from_code(payload[0])?)
            }
            _ => return None,
        })
    }
}

/// Encode an input event into its binary payload (without the frame header).
pub fn encode_input_payload(ev: &InputEvent) -> Vec<u8> {
    ev.encode_payload()
}

/// Low-rate control messages, JSON encoded.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ControlMsg {
    /// Client -> server handshake.
    Hello {
        v: u16,
        name: String,
        platform: String,
        /// Keyboard layout id, informational.
        layout: String,
    },
    /// Server -> client handshake acknowledgement.
    HelloOk {
        v: u16,
        name: String,
        platform: String,
        layout: String,
    },
    /// Controller -> target: this is now where the input goes.
    State { target: Target },
    /// Target -> controller: please move the input to this target.
    Request { target: Target },
    /// Target -> controller: I hit a screen edge, give the input back.
    Edge { edge: Edge },
    /// Clipboard contents to synchronise.
    Clipboard { text: String },
    /// Keepalive.
    Ping,
    /// Keepalive reply.
    Pong,
    /// Graceful disconnect.
    Bye,
}

/// A decoded frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Frame {
    Control(ControlMsg),
    Input(InputEvent),
}

impl Frame {
    /// Encode a frame, including its length prefix.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Frame::Control(msg) => (
                kind::CONTROL,
                serde_json::to_vec(msg).expect("control message serialises"),
            ),
            Frame::Input(ev) => (ev.kind(), ev.encode_payload()),
        };
        let mut out = Vec::with_capacity(payload.len() + 5);
        // The length covers the kind byte plus the payload, matching
        // `read_from`, which reads one length and then splits the buffer.
        out.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
        out.push(kind);
        out.extend_from_slice(&payload);
        out
    }

    /// Read exactly one frame from a stream.
    pub fn read_from<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes(len_buf);
        if len > MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        let mut buf = vec![0u8; len as usize];
        reader.read_exact(&mut buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(io::ErrorKind::UnexpectedEof, "truncated frame")
            } else {
                e
            }
        })?;
        let kind = buf[0];
        let payload = &buf[1..];
        let frame = if kind == kind::CONTROL {
            match serde_json::from_slice::<ControlMsg>(payload) {
                Ok(msg) => Frame::Control(msg),
                Err(e) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("bad control message: {e}"),
                    ))
                }
            }
        } else {
            match InputEvent::decode(kind, payload) {
                Some(ev) => Frame::Input(ev),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bad input event",
                    ))
                }
            }
        };
        Ok(Some(frame))
    }

    /// Write a frame to a stream.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&self.encode())?;
        writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let frames = vec![
            Frame::Control(ControlMsg::Hello {
                v: PROTOCOL_VERSION,
                name: "pc".into(),
                platform: "windows".into(),
                layout: "us".into(),
            }),
            Frame::Control(ControlMsg::State {
                target: Target::Remote,
            }),
            Frame::Input(InputEvent::Key {
                action: KeyAction::Down,
                key: 1,
                mods: 3,
            }),
            Frame::Input(InputEvent::MouseMove { dx: -3, dy: 12 }),
            Frame::Input(InputEvent::MouseButton {
                button: MouseButton::Right,
                pressed: true,
            }),
            Frame::Input(InputEvent::Wheel { dx: 0, dy: -1 }),
            Frame::Input(InputEvent::PointerEdge(Edge::Right)),
        ];
        let mut buf = Vec::new();
        for f in &frames {
            f.write_to(&mut buf).unwrap();
        }
        let mut cursor = std::io::Cursor::new(buf);
        for expected in frames {
            let got = Frame::read_from(&mut cursor).unwrap().unwrap();
            assert_eq!(got, expected);
        }
        assert!(Frame::read_from(&mut cursor).unwrap().is_none());
    }
}
