//! A tiny loopback control channel.
//!
//! `multcontrol toggle|local|remote|status|quit` connects to the running agent
//! on `127.0.0.1:47779` and sends one line. Loopback traffic is exempt from the
//! Windows firewall, so this needs no rules and no elevation.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// Port used by the local control channel.
pub const CONTROL_PORT: u16 = 47779;

/// Requests the agent understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Flip which machine receives the input.
    Toggle,
    /// Take the input on this machine.
    Local,
    /// Send the input to the peer.
    Remote,
    /// Ask for a status line.
    Status,
    /// Shut down.
    Quit,
}

impl Command {
    /// Parse a command word.
    pub fn parse(text: &str) -> Option<Command> {
        Some(match text.trim() {
            "toggle" => Command::Toggle,
            "local" => Command::Local,
            "remote" => Command::Remote,
            "status" => Command::Status,
            "quit" | "exit" => Command::Quit,
            _ => return None,
        })
    }
}

/// Start the loopback listener.
///
/// Binds immediately so a second agent on the same machine fails here rather
/// than running two sets of input hooks.
pub fn spawn(
    tx: Sender<Command>,
    describe: Arc<dyn Fn() -> String + Send + Sync>,
) -> std::io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, CONTROL_PORT));
    let listener = TcpListener::bind(addr)?;
    std::thread::Builder::new()
        .name("mc-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = handle(stream, &tx, describe.as_ref());
            }
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

fn handle(
    stream: TcpStream,
    tx: &Sender<Command>,
    describe: &(dyn Fn() -> String + Send + Sync),
) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let Some(command) = Command::parse(&line) else {
        let mut out = stream;
        let _ = writeln!(out, "unknown command; use toggle|local|remote|status|quit");
        return Ok(());
    };
    if command == Command::Status {
        let mut out = stream;
        let _ = writeln!(out, "{}", describe());
        return Ok(());
    }
    let _ = tx.send(command);
    let mut out = stream;
    let _ = writeln!(out, "ok");
    Ok(())
}

/// Send a command to a running agent, returning its reply.
pub fn request(text: &str) -> std::io::Result<String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, CONTROL_PORT));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800))?;
    stream.set_read_timeout(Some(Duration::from_millis(1500)))?;
    stream.write_all(text.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut reply = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

/// Whether an agent is currently listening on the control port.
pub fn agent_running() -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, CONTROL_PORT));
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}
