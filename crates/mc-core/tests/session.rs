//! End-to-end framing test: a real TCP socket, the real handshake, real
//! encryption. This is the closest thing to an integration test for the wire
//! protocol without two machines.

use mc_core::crypto::Psk;
use mc_core::proto::{ControlMsg, Edge, Frame, InputEvent, KeyAction, MouseButton, Target};
use mc_core::session::Session;
use std::net::{TcpListener, TcpStream};

#[test]
fn handshake_and_bidirectional_frames() {
    let psk = Psk::generate();
    let server_psk = Psk::from_hex(&psk.to_hex()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let session = Session::server(stream, &server_psk, "mac", "macos", "us").unwrap();
        assert_eq!(session.peer_info().name, "pc");
        assert_eq!(session.peer_info().platform, "windows");
        let (mut reader, mut writer) = session.split().unwrap();

        // Client -> server.
        assert_eq!(
            reader.recv().unwrap(),
            Frame::Control(ControlMsg::State {
                target: Target::Remote
            })
        );
        assert_eq!(
            reader.recv().unwrap(),
            Frame::Input(InputEvent::MouseMove { dx: 7, dy: -2 })
        );

        // Server -> client, including every input variant.
        for frame in [
            Frame::Control(ControlMsg::Edge { edge: Edge::Left }),
            Frame::Control(ControlMsg::Clipboard {
                text: "héllo ✓".to_string(),
            }),
            Frame::Input(InputEvent::Key {
                action: KeyAction::Down,
                key: 42,
                mods: 5,
            }),
            Frame::Input(InputEvent::MouseButton {
                button: MouseButton::Extra2,
                pressed: false,
            }),
            Frame::Input(InputEvent::Wheel { dx: -1, dy: 3 }),
            Frame::Input(InputEvent::PointerEdge(Edge::Bottom)),
        ] {
            writer.send(&frame).unwrap();
        }
    });

    let stream = TcpStream::connect(addr).unwrap();
    let session = Session::client(stream, &psk, "pc", "windows", "us").unwrap();
    assert_eq!(session.peer_info().name, "mac");
    let (mut reader, mut writer) = session.split().unwrap();

    writer
        .send(&Frame::Control(ControlMsg::State {
            target: Target::Remote,
        }))
        .unwrap();
    writer
        .send(&Frame::Input(InputEvent::MouseMove { dx: 7, dy: -2 }))
        .unwrap();

    let expected = [
        Frame::Control(ControlMsg::Edge { edge: Edge::Left }),
        Frame::Control(ControlMsg::Clipboard {
            text: "héllo ✓".to_string(),
        }),
        Frame::Input(InputEvent::Key {
            action: KeyAction::Down,
            key: 42,
            mods: 5,
        }),
        Frame::Input(InputEvent::MouseButton {
            button: MouseButton::Extra2,
            pressed: false,
        }),
        Frame::Input(InputEvent::Wheel { dx: -1, dy: 3 }),
        Frame::Input(InputEvent::PointerEdge(Edge::Bottom)),
    ];
    for want in expected {
        assert_eq!(reader.recv().unwrap(), want);
    }

    server.join().unwrap();
}

#[test]
fn wrong_pre_shared_key_is_rejected() {
    let good = Psk::generate();
    let bad = Psk::generate();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let result = Session::server(stream, &bad, "mac", "macos", "us");
        assert!(result.is_err(), "server accepted the wrong key");
    });

    let stream = TcpStream::connect(addr).unwrap();
    let result = Session::client(stream, &good, "pc", "windows", "us");
    assert!(result.is_err(), "client accepted the wrong key");
    server.join().unwrap();
}
