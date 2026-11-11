//! `multcontrol selftest`: prove this binary works on this machine.
//!
//! This is meant for the machine that is about to run the agent, especially a
//! locked-down one where installing something unverified is not an option. It
//! installs nothing, touches no config and does not change the clipboard; it
//! exercises the same code the agent uses on the wire and in the input layer,
//! then exits.
//!
//! Exit code 0 means every hard check passed. Warnings (missing Accessibility,
//! an empty clipboard, a busy control port) do not fail the run.

use crate::input::{self, CaptureOptions};
use crate::platform_name;
use mc_core::config::Config;
use mc_core::crypto::Psk;
use mc_core::hotkey::Hotkey;
use mc_core::key::Key;
use mc_core::proto::{ControlMsg, Frame};
use mc_core::session::Session;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

fn ok(name: &str, detail: impl std::fmt::Display) {
    println!("  [ok]   {name:<12} {detail}");
}

fn warn(name: &str, detail: impl std::fmt::Display) {
    println!("  [warn] {name:<12} {detail}");
}

fn fail(name: &str, detail: impl std::fmt::Display) {
    println!("  [FAIL] {name:<12} {detail}");
}

/// Run every check and return a process exit code.
pub fn run() -> i32 {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".to_string());
    println!("multcontrol {} selftest", env!("CARGO_PKG_VERSION"));
    println!("  binary      {exe}");
    println!(
        "  host        {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        platform_name()
    );
    println!();

    let mut failures = 0usize;

    match config_roundtrip() {
        Ok(detail) => ok("config", detail),
        Err(e) => {
            fail("config", e);
            failures += 1;
        }
    }

    match key_tables() {
        Ok(n) => ok("key tables", format!("{n} keys round-trip")),
        Err(e) => {
            fail("key tables", e);
            failures += 1;
        }
    }

    match handshake() {
        Ok(()) => ok("handshake", "mutual auth + encrypted frames over loopback"),
        Err(e) => {
            fail("handshake", e);
            failures += 1;
        }
    }

    match wrong_key_rejected() {
        Ok(()) => ok("wrong key", "rejected before any input is accepted"),
        Err(e) => {
            fail("wrong key", e);
            failures += 1;
        }
    }

    match clipboard() {
        Ok(Some(chars)) => ok("clipboard", format!("readable ({chars} chars)")),
        Ok(None) => warn("clipboard", "empty or unavailable (not fatal)"),
        Err(e) => warn("clipboard", e),
    }

    let port = crate::control_socket::control_port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(socket) => {
            drop(socket);
            ok("control port", format!("{port} is free on loopback"));
        }
        Err(e) => warn(
            "control port",
            format!("{port} is busy ({e}); an agent may already be running"),
        ),
    }

    #[cfg(target_os = "macos")]
    if input::accessibility_trusted() {
        ok("permission", "Accessibility granted");
    } else {
        warn(
            "permission",
            "Accessibility not granted; capture/injection need it (System Settings)",
        );
    }

    // Last, because it installs the real hooks. They are live only for the rest
    // of this short-lived process and pass input through untouched.
    #[cfg(target_os = "macos")]
    let hooks_blocked = !input::accessibility_trusted();
    #[cfg(not(target_os = "macos"))]
    let hooks_blocked = false;

    if hooks_blocked {
        warn(
            "input hooks",
            "skipped until Accessibility is granted; everything else is fine",
        );
    } else {
        match input_hooks() {
            Ok(()) => ok("input hooks", "capture hooks installed"),
            Err(e) => {
                fail("input hooks", e);
                failures += 1;
            }
        }
    }
    input::shutdown();

    println!();
    if failures == 0 {
        println!("result: PASS (warnings above are informational)");
        0
    } else {
        println!("result: FAIL ({failures} check(s) failed)");
        1
    }
}

fn config_roundtrip() -> Result<String, String> {
    let cfg = Config::default();
    let text = cfg.to_toml();
    let back = Config::from_toml(&text).map_err(|e| e.to_string())?;
    if back.network.psk != cfg.network.psk {
        return Err("pre-shared key changed across a round-trip".into());
    }
    if back.control.hotkey != cfg.control.hotkey {
        return Err("hotkey changed across a round-trip".into());
    }
    if back.control.role != cfg.control.role || back.network.role != cfg.network.role {
        return Err("roles changed across a round-trip".into());
    }
    Ok(format!(
        "{} bytes, roles {}, hotkey {}",
        text.len(),
        format_args!("{:?}", back.control.role),
        back.control.hotkey
    ))
}

fn key_tables() -> Result<usize, String> {
    let mut bad = Vec::new();
    for &key in Key::ALL {
        if Key::from_id(key.id()) != Some(key) {
            bad.push(format!("{key:?}: wire id does not round-trip"));
        }

        #[cfg(windows)]
        {
            match key.to_win_scan() {
                Some((scan, vk)) => {
                    if Key::from_win_scan(scan, vk) != Some(key) {
                        bad.push(format!("{key:?}: scan code does not round-trip"));
                    }
                }
                None => bad.push(format!("{key:?}: no Windows scan code")),
            }
        }

        #[cfg(target_os = "macos")]
        {
            match key.to_mac_keycode() {
                Some(code) => {
                    if Key::from_mac_keycode(code) != Some(key) {
                        bad.push(format!("{key:?}: keycode does not round-trip"));
                    }
                }
                None => bad.push(format!("{key:?}: no macOS keycode")),
            }
        }
    }
    if bad.is_empty() {
        Ok(Key::ALL.len())
    } else {
        Err(bad.join("; "))
    }
}

fn handshake() -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let psk = Psk::generate();

    let server = std::thread::spawn(move || -> Result<(), String> {
        let (stream, _) = listener.accept().map_err(|e| e.to_string())?;
        arm(&stream)?;
        let session =
            Session::server(stream, &psk, "selftest-server", "selftest", "us")
                .map_err(|e| e.to_string())?;
        let (mut reader, mut writer) = session.split().map_err(|e| e.to_string())?;

        let frame = reader.recv().map_err(|e| e.to_string())?;
        if !matches!(frame, Frame::Control(ControlMsg::Ping)) {
            return Err(format!("server expected Ping, got {frame:?}"));
        }
        writer
            .send(&Frame::Control(ControlMsg::Pong))
            .map_err(|e| e.to_string())?;

        let frame = reader.recv().map_err(|e| e.to_string())?;
        match frame {
            Frame::Control(ControlMsg::Clipboard { text }) if text == "selftest" => Ok(()),
            other => Err(format!("server expected clipboard, got {other:?}")),
        }
    });

    let stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    arm(&stream)?;
    let session = Session::client(stream, &psk, "selftest-client", "selftest", "us")
        .map_err(|e| e.to_string())?;
    let (mut reader, mut writer) = session.split().map_err(|e| e.to_string())?;

    writer
        .send(&Frame::Control(ControlMsg::Ping))
        .map_err(|e| e.to_string())?;
    writer
        .send(&Frame::Control(ControlMsg::Clipboard {
            text: "selftest".to_string(),
        }))
        .map_err(|e| e.to_string())?;

    let frame = reader.recv().map_err(|e| e.to_string())?;
    if !matches!(frame, Frame::Control(ControlMsg::Pong)) {
        return Err(format!("client expected Pong, got {frame:?}"));
    }
    server
        .join()
        .map_err(|_| "handshake server thread panicked".to_string())?
}

fn wrong_key_rejected() -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let correct = Psk::generate();
    let wrong = Psk::generate();

    let server = std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            arm(&stream).ok();
            // This must fail; the result is checked on the client side too.
            let accepted = Session::server(stream, &correct, "selftest", "selftest", "us");
            if accepted.is_ok() {
                return false;
            }
        }
        true
    });

    let stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    arm(&stream)?;
    let accepted = Session::client(stream, &wrong, "selftest", "selftest", "us");
    let client_rejected = accepted.is_err();
    let server_rejected = server.join().map_err(|_| "thread panicked".to_string())?;
    if client_rejected && server_rejected {
        Ok(())
    } else if !client_rejected {
        Err("a wrong pre-shared key was accepted by the client".into())
    } else {
        Err("a wrong pre-shared key was accepted by the server".into())
    }
}

fn clipboard() -> Result<Option<usize>, String> {
    match crate::clipboard::get_text() {
        Some(text) => Ok(Some(text.chars().count())),
        None => Ok(None),
    }
}

fn input_hooks() -> Result<(), String> {
    let backend = input::platform_backend().map_err(|e| format!("{e:#}"))?;
    let hotkey = Hotkey::parse("ctrl+alt+space").map_err(|e| format!("{e:?}"))?;
    let (tx, _rx) = std::sync::mpsc::channel();
    let opts = CaptureOptions {
        hotkey,
        edge_push: true,
        edge_margin: 2,
    };
    backend.start(tx, opts).map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// Bound every socket used by the selftest, so a broken network stack fails
/// quickly instead of hanging the command.
fn arm(stream: &TcpStream) -> Result<(), String> {
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout).map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(timeout)
        .map_err(|e| e.to_string())
}
