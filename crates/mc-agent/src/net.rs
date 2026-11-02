//! Connection management.
//!
//! The Windows agent is always the *client*: it dials out to the Mac's LAN
//! address. Outbound TCP is permitted by default on Windows, so the agent
//! never needs an inbound firewall rule (which would require administrator
//! rights). The Mac agent is the listener.
//!
//! A small UDP broadcast lets the client find the Mac without hard-coding an
//! address; a static address in the config always takes priority.

use mc_core::config::{NetworkConfig, Role};
use mc_core::crypto::Psk;
use mc_core::proto::Frame;
use mc_core::session::{PeerInfo, Session};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Connection lifecycle events, delivered to the control loop.
pub enum NetEvent {
    /// A peer completed the handshake. `tx` feeds the outbound direction.
    Connected {
        /// Information the peer reported about itself.
        info: PeerInfo,
        /// Channel for frames to send to the peer.
        tx: Sender<Frame>,
    },
    /// The connection ended.
    Disconnected {
        /// Human readable reason, for the log.
        reason: String,
    },
    /// A frame arrived from the peer.
    Frame(Frame),
}

/// Handle used to stop the networking threads.
pub struct NetHandle {
    stop: Arc<AtomicBool>,
}

impl NetHandle {
    /// Ask the network threads to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for NetHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the networking thread(s) for the configured role.
pub fn spawn(
    cfg: NetworkConfig,
    psk: Psk,
    name: String,
    layout: String,
    events: Sender<NetEvent>,
) -> NetHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = NetHandle { stop: stop.clone() };

    if cfg.role == Role::Server {
        if cfg.discovery {
            spawn_discovery_responder(cfg.discovery_port, cfg.port, stop.clone());
        }
        std::thread::Builder::new()
            .name("mc-net-accept".into())
            .spawn(move || accept_loop(cfg, psk, name, layout, events, stop))
            .expect("spawning accept thread");
    } else {
        std::thread::Builder::new()
            .name("mc-net-dial".into())
            .spawn(move || dial_loop(cfg, psk, name, layout, events, stop))
            .expect("spawning dial thread");
    }
    handle
}

fn accept_loop(
    cfg: NetworkConfig,
    psk: Psk,
    name: String,
    layout: String,
    events: Sender<NetEvent>,
    stop: Arc<AtomicBool>,
) {
    let addr = cfg
        .listen_addr
        .clone()
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], cfg.port)));
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            let _ = events.send(NetEvent::Disconnected {
                reason: format!("cannot listen on {addr}: {e}"),
            });
            log::error!("cannot listen on {addr}: {e}");
            return;
        }
    };
    log::info!("listening for the PC on {addr} (local-only, no firewall rule needed on the PC)");
    // A short accept timeout lets us notice shutdown requests.
    let _ = listener.set_nonblocking(true);
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                log::info!("incoming connection from {peer}");
                // BSD/macOS accepted sockets inherit O_NONBLOCK from the
                // listener, which would make every read fail with EWOULDBLOCK.
                // Linux does not, so set it explicitly on both.
                if let Err(e) = stream.set_nonblocking(false) {
                    log::warn!("could not make the session socket blocking: {e}");
                    continue;
                }
                if let Err(e) = run_session(
                    stream,
                    Role::Server,
                    &psk,
                    &name,
                    crate::platform_name(),
                    &layout,
                    &events,
                ) {
                    log::warn!("session ended: {e}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => {
                log::warn!("accept failed: {e}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn dial_loop(
    cfg: NetworkConfig,
    psk: Psk,
    name: String,
    layout: String,
    events: Sender<NetEvent>,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_millis(500);
    let mut discovery_reported = false;
    while !stop.load(Ordering::SeqCst) {
        let addr = match resolve_peer(&cfg) {
            Some(a) => a,
            None => {
                if !discovery_reported {
                    log::info!("looking for the Mac on the local network (UDP broadcast)…");
                    discovery_reported = true;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(5));
                continue;
            }
        };
        log::info!("connecting to {addr}");
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(stream) => {
                discovery_reported = false;
                backoff = Duration::from_millis(500);
                if let Err(e) = run_session(
                    stream,
                    Role::Client,
                    &psk,
                    &name,
                    crate::platform_name(),
                    &layout,
                    &events,
                ) {
                    log::warn!("session ended: {e}");
                }
            }
            Err(e) => {
                log::warn!("cannot reach {addr}: {e}");
                // The configured address may be stale after a DHCP change;
                // ask the network instead.
                if cfg.discovery && cfg.peer_addr.is_some() {
                    if let Some(found) = discover(cfg.discovery_port) {
                        log::info!("discovery found the Mac at {found}");
                        if let Ok(stream) =
                            TcpStream::connect_timeout(&found, Duration::from_secs(3))
                        {
                            if let Err(e) = run_session(
                                stream,
                                Role::Client,
                                &psk,
                                &name,
                                crate::platform_name(),
                                &layout,
                                &events,
                            ) {
                                log::warn!("session ended: {e}");
                            }
                            continue;
                        }
                    }
                }
            }
        }
        // Wait before retrying, but stay responsive to shutdown.
        let deadline = std::time::Instant::now() + backoff;
        while std::time::Instant::now() < deadline {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn resolve_peer(cfg: &NetworkConfig) -> Option<SocketAddr> {
    if let Some(addr) = &cfg.peer_addr {
        if let Ok(parsed) = addr.parse::<SocketAddr>() {
            return Some(parsed);
        }
        // Accept "host:port" too.
        if let Some(found) = resolve_host_port(addr) {
            return Some(found);
        }
    }
    if cfg.discovery {
        return discover(cfg.discovery_port);
    }
    None
}

fn resolve_host_port(text: &str) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    text.to_socket_addrs().ok()?.next()
}

/// Broadcast a probe and wait for the Mac's reply.
pub fn discover(port: u16) -> Option<SocketAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_broadcast(true).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(700)))
        .ok()?;
    sock.send_to(b"MC_DISCOVER 1", ("255.255.255.255", port))
        .ok()?;
    let mut buf = [0u8; 128];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    let text = std::str::from_utf8(&buf[..n]).ok()?;
    let tcp_port: u16 = text.strip_prefix("MC_HERE ")?.trim().parse().ok()?;
    Some(SocketAddr::new(from.ip(), tcp_port))
}

fn spawn_discovery_responder(port: u16, tcp_port: u16, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("mc-discovery".into())
        .spawn(move || {
            let sock = match UdpSocket::bind(("0.0.0.0", port)) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("cannot bind discovery port {port}: {e}");
                    return;
                }
            };
            let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
            let reply = format!("MC_HERE {tcp_port}");
            let mut buf = [0u8; 128];
            while !stop.load(Ordering::SeqCst) {
                match sock.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        if buf[..n].starts_with(b"MC_DISCOVER") {
                            let _ = sock.send_to(reply.as_bytes(), from);
                        }
                    }
                    Err(_) => continue,
                }
            }
        })
        .ok();
}

fn run_session(
    stream: TcpStream,
    role: Role,
    psk: &Psk,
    name: &str,
    platform: &str,
    layout: &str,
    events: &Sender<NetEvent>,
) -> anyhow::Result<()> {
    // Kept back so the watchdog can tear the connection down even if the
    // reader is blocked in a read.
    let watchdog_sock = stream.try_clone()?;
    let session = match role {
        Role::Client => Session::client(stream, psk, name, platform, layout)?,
        Role::Server => Session::server(stream, psk, name, platform, layout)?,
    };
    let info = session.peer_info();
    let (mut reader, mut writer) = session.split()?;
    let (tx, rx): (Sender<Frame>, Receiver<Frame>) = channel();
    let _ = events.send(NetEvent::Connected { info, tx });
    log::info!("session established");

    let alive = Arc::new(AtomicBool::new(true));

    // A TCP connection can go silent without ever closing: the peer's Wi-Fi
    // drops, it sleeps, or the router forgets the flow. The controller pings
    // every ten seconds, so a long silence means the connection is dead. This
    // matters most on the Mac, which would otherwise block forever in `recv`
    // and never accept the PC's reconnection.
    let started = std::time::Instant::now();
    let last_rx = Arc::new(AtomicU64::new(0));
    let watchdog_alive = alive.clone();
    let watchdog_last = last_rx.clone();
    let watchdog_thread = std::thread::Builder::new()
        .name("mc-net-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            if !watchdog_alive.load(Ordering::SeqCst) {
                return;
            }
            let idle = started.elapsed().as_millis() as u64 - watchdog_last.load(Ordering::Relaxed);
            if idle > 45_000 {
                log::warn!("no traffic from the peer for 45 s; dropping the connection");
                let _ = watchdog_sock.shutdown(Shutdown::Both);
                return;
            }
        })?;

    let writer_alive = alive.clone();
    let writer_thread = std::thread::Builder::new()
        .name("mc-net-write".into())
        .spawn(move || {
            while writer_alive.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(frame) => {
                        if writer.send(&frame).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // Wake a reader blocked on this socket; otherwise a failed write
            // would leave the session half-open forever.
            writer.shutdown();
            writer_alive.store(false, Ordering::SeqCst);
        })?;

    let reason = loop {
        match reader.recv() {
            Ok(frame) => {
                last_rx.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                if matches!(frame, Frame::Control(mc_core::proto::ControlMsg::Bye)) {
                    break "peer said goodbye".to_string();
                }
                if events.send(NetEvent::Frame(frame)).is_err() {
                    break "agent shutting down".to_string();
                }
            }
            Err(e) => break e.to_string(),
        }
    };
    alive.store(false, Ordering::SeqCst);
    let _ = writer_thread.join();
    let _ = watchdog_thread.join();
    let _ = events.send(NetEvent::Disconnected {
        reason: reason.clone(),
    });
    log::info!("session closed: {reason}");
    Ok(())
}
