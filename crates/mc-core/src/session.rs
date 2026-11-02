//! Session setup: handshake, key derivation and encrypted framing.

use crate::crypto::{
    ack_tag, derive_keys, hello_tag, random_nonce, Crypter, Psk, HANDSHAKE_TAG_LEN,
};
use crate::key::Key;
use crate::proto::{Frame, InputEvent, PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

/// Frame kind used for the plaintext handshake.
const KIND_HELLO: u8 = 0xF0;
/// Frame kind used for the plaintext handshake reply.
const KIND_HELLO_ACK: u8 = 0xF1;

/// Handshake payload, sent in the clear but authenticated by the tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HandshakeBody {
    v: u16,
    name: String,
    platform: String,
    layout: String,
    /// Sender nonce, hex encoded.
    n: String,
}

/// Information about the peer, learned during the handshake.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    /// Human readable machine name.
    pub name: String,
    /// Operating system identifier.
    pub platform: String,
    /// Keyboard layout identifier.
    pub layout: String,
}

/// An established, encrypted session with the peer.
pub struct Session {
    stream: TcpStream,
    enc: Crypter,
    dec: Crypter,
    /// Set when the peer signalled a clean disconnect.
    closed: bool,
    /// Peer machine name from the handshake.
    peer_name: String,
    /// Peer platform from the handshake.
    peer_platform: String,
    /// Peer keyboard layout from the handshake.
    peer_layout: String,
}

impl Session {
    /// Perform the client side of the handshake.
    pub fn client(
        stream: TcpStream,
        psk: &Psk,
        name: &str,
        platform: &str,
        layout: &str,
    ) -> io::Result<Session> {
        let mut stream = stream;
        stream.set_nodelay(true).ok();
        // A peer that accepts and then goes silent must not wedge the reconnect
        // loop. Cleared once the session is established.
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

        let nonce = random_nonce();
        let body = HandshakeBody {
            v: PROTOCOL_VERSION,
            name: name.to_string(),
            platform: platform.to_string(),
            layout: layout.to_string(),
            n: crate::crypto::hex_encode(&nonce),
        };
        let body_bytes = serde_json::to_vec(&body).expect("handshake serialises");
        let tag = hello_tag(psk, &nonce, &body_bytes);
        write_plain(&mut stream, KIND_HELLO, &body_bytes, &tag)?;

        let (ack_kind, ack_bytes, ack_tag_bytes) = read_plain(&mut stream).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "peer closed the connection during the handshake (wrong pre-shared key?)",
                )
            } else {
                e
            }
        })?;
        if ack_kind != KIND_HELLO_ACK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected handshake acknowledgement",
            ));
        }
        let ack: HandshakeBody = serde_json::from_slice(&ack_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let server_nonce = crate::crypto::hex_decode(&ack.n)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if server_nonce.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad server nonce",
            ));
        }
        let mut sn = [0u8; 32];
        sn.copy_from_slice(&server_nonce);

        let expected = ack_tag(psk, &nonce, &sn, &ack_bytes);
        if !constant_time_eq(&expected, &ack_tag_bytes) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer failed authentication: wrong pre-shared key?",
            ));
        }
        if ack.v != PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "peer speaks protocol v{}, we speak v{}",
                    ack.v, PROTOCOL_VERSION
                ),
            ));
        }

        let keys = derive_keys(psk, &nonce, &sn);
        stream.set_read_timeout(None).ok();
        stream.set_write_timeout(None).ok();
        Ok(Session {
            stream,
            enc: Crypter::new(&keys.c2s),
            dec: Crypter::new(&keys.s2c),
            closed: false,
            peer_name: ack.name,
            peer_platform: ack.platform,
            peer_layout: ack.layout,
        })
    }

    /// Perform the server side of the handshake.
    pub fn server(
        stream: TcpStream,
        psk: &Psk,
        name: &str,
        platform: &str,
        layout: &str,
    ) -> io::Result<Session> {
        let mut stream = stream;
        stream.set_nodelay(true).ok();
        // Do not hold an accept loop hostage to an idle or hostile connection.
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

        let (kind, body_bytes, tag) = read_plain(&mut stream)?;
        if kind != KIND_HELLO {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected handshake",
            ));
        }
        let hello: HandshakeBody = serde_json::from_slice(&body_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let client_nonce_bytes = crate::crypto::hex_decode(&hello.n)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if client_nonce_bytes.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad client nonce",
            ));
        }
        let mut cn = [0u8; 32];
        cn.copy_from_slice(&client_nonce_bytes);

        let expected = hello_tag(psk, &cn, &body_bytes);
        if !constant_time_eq(&expected, &tag) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer failed authentication: wrong pre-shared key?",
            ));
        }
        if hello.v != PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "peer speaks protocol v{}, we speak v{}",
                    hello.v, PROTOCOL_VERSION
                ),
            ));
        }

        let server_nonce = random_nonce();
        let ack = HandshakeBody {
            v: PROTOCOL_VERSION,
            name: name.to_string(),
            platform: platform.to_string(),
            layout: layout.to_string(),
            n: crate::crypto::hex_encode(&server_nonce),
        };
        let ack_bytes = serde_json::to_vec(&ack).expect("handshake serialises");
        let ack_tag_bytes = ack_tag(psk, &cn, &server_nonce, &ack_bytes);
        write_plain(&mut stream, KIND_HELLO_ACK, &ack_bytes, &ack_tag_bytes)?;

        let keys = derive_keys(psk, &cn, &server_nonce);
        stream.set_read_timeout(None).ok();
        stream.set_write_timeout(None).ok();
        Ok(Session {
            stream,
            enc: Crypter::new(&keys.s2c),
            dec: Crypter::new(&keys.c2s),
            closed: false,
            peer_name: hello.name,
            peer_platform: hello.platform,
            peer_layout: hello.layout,
        })
    }

    /// Peer information gathered during the handshake.
    pub fn peer_info(&self) -> PeerInfo {
        PeerInfo {
            name: self.peer_name.clone(),
            platform: self.peer_platform.clone(),
            layout: self.peer_layout.clone(),
        }
    }

    /// Split into independent read and write halves so a reader thread and a
    /// writer thread can own the two directions without locking.
    pub fn split(self) -> io::Result<(SessionReader, SessionWriter)> {
        let read_stream = self.stream.try_clone()?;
        Ok((
            SessionReader {
                stream: read_stream,
                dec: self.dec,
            },
            SessionWriter {
                stream: self.stream,
                enc: self.enc,
            },
        ))
    }

    /// Send a frame.
    pub fn send(&mut self, frame: &Frame) -> io::Result<()> {
        let (kind, plaintext) = match frame {
            Frame::Control(msg) => (
                crate::proto::kind::CONTROL,
                serde_json::to_vec(msg).expect("control message serialises"),
            ),
            Frame::Input(ev) => (input_kind(ev), encode_input(ev)),
        };
        let sealed = self.enc.seal(kind, &plaintext);
        let mut header = [0u8; 5];
        header[..4].copy_from_slice(&(sealed.len() as u32).to_le_bytes());
        header[4] = kind;
        self.stream.write_all(&header)?;
        self.stream.write_all(&sealed)?;
        self.stream.flush()
    }

    /// Receive a frame.
    pub fn recv(&mut self) -> io::Result<Frame> {
        let mut header = [0u8; 5];
        match self.stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                self.closed = true;
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer disconnected",
                ));
            }
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        if len > crate::proto::MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        let kind = header[4];
        let mut sealed = vec![0u8; len as usize];
        self.stream.read_exact(&mut sealed)?;
        let plaintext = self
            .dec
            .open(kind, &sealed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if kind == crate::proto::kind::CONTROL {
            let msg = serde_json::from_slice(&plaintext)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            Ok(Frame::Control(msg))
        } else {
            InputEvent::decode(kind, &plaintext)
                .map(Frame::Input)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad input event"))
        }
    }
}

/// Read half of a session.
pub struct SessionReader {
    stream: TcpStream,
    dec: Crypter,
}

/// Write half of a session.
pub struct SessionWriter {
    stream: TcpStream,
    enc: Crypter,
}

impl SessionReader {
    /// Close the underlying socket, waking the writer if it is blocked.
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Receive a frame.
    pub fn recv(&mut self) -> io::Result<Frame> {
        let mut header = [0u8; 5];
        match self.stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer disconnected",
                ))
            }
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        if len > crate::proto::MAX_FRAME_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        let kind = header[4];
        let mut sealed = vec![0u8; len as usize];
        self.stream.read_exact(&mut sealed)?;
        let plaintext = self
            .dec
            .open(kind, &sealed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        if kind == crate::proto::kind::CONTROL {
            let msg = serde_json::from_slice(&plaintext)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            Ok(Frame::Control(msg))
        } else {
            InputEvent::decode(kind, &plaintext)
                .map(Frame::Input)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad input event"))
        }
    }
}

impl SessionWriter {
    /// Close the underlying socket, waking the reader if it is blocked.
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Send a frame.
    pub fn send(&mut self, frame: &Frame) -> io::Result<()> {
        let (kind, plaintext) = match frame {
            Frame::Control(msg) => (
                crate::proto::kind::CONTROL,
                serde_json::to_vec(msg).expect("control message serialises"),
            ),
            Frame::Input(ev) => (input_kind(ev), crate::proto::encode_input_payload(ev)),
        };
        let sealed = self.enc.seal(kind, &plaintext);
        let mut header = [0u8; 5];
        header[..4].copy_from_slice(&(sealed.len() as u32).to_le_bytes());
        header[4] = kind;
        self.stream.write_all(&header)?;
        self.stream.write_all(&sealed)?;
        self.stream.flush()
    }
}

fn input_kind(ev: &InputEvent) -> u8 {
    match ev {
        InputEvent::Key { .. } => crate::proto::kind::KEY,
        InputEvent::MouseMove { .. } => crate::proto::kind::MOUSE_MOVE,
        InputEvent::MouseButton { .. } => crate::proto::kind::MOUSE_BUTTON,
        InputEvent::Wheel { .. } => crate::proto::kind::WHEEL,
        InputEvent::PointerEdge(_) => crate::proto::kind::POINTER_EDGE,
    }
}

fn encode_input(ev: &InputEvent) -> Vec<u8> {
    // The proto crate owns the encoding; re-use it through a public helper.
    crate::proto::encode_input_payload(ev)
}

fn write_plain<W: Write>(writer: &mut W, kind: u8, body: &[u8], tag: &[u8]) -> io::Result<()> {
    let total = body.len() + tag.len();
    let mut header = [0u8; 5];
    header[..4].copy_from_slice(&(total as u32).to_le_bytes());
    header[4] = kind;
    writer.write_all(&header)?;
    writer.write_all(body)?;
    writer.write_all(tag)?;
    writer.flush()
}

fn read_plain<R: Read>(reader: &mut R) -> io::Result<(u8, Vec<u8>, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if len < HANDSHAKE_TAG_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short handshake",
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let kind = header[4];
    let body = buf[..len - HANDSHAKE_TAG_LEN].to_vec();
    let tag = buf[len - HANDSHAKE_TAG_LEN..].to_vec();
    Ok((kind, body, tag))
}

/// Compare two byte strings without leaking timing information.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Convenience alias used by the agent when reporting a key press.
pub type KeyId = u16;

/// Re-export for the agent's convenience.
pub use crate::key::Key as CanonicalKey;

const _: fn() = || {
    // Compile-time reminder that `Key` must stay `Copy`.
    let _ = Key::A;
};
