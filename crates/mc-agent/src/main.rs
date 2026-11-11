//! multcontrol agent.
//!
//! One binary, two roles. On the Windows PC it runs as the *controller*: it
//! captures the physical keyboard and mouse, dials out to the Mac, and forwards
//! input when the hotkey or a screen edge hands control over. On the Mac it runs
//! as the *target*: it listens, injects whatever the PC sends, and gives control
//! back when you press the hotkey or push the pointer into an edge.
//!
//! Nothing here needs administrator rights on Windows: no service, no firewall
//! rule, no driver. On macOS the only privileged-looking step is a one-time
//! Accessibility permission, granted per user in System Settings.

mod app;
mod clipboard;
mod control_socket;
mod gui;
mod input;
mod net;
mod selftest;
#[cfg(windows)]
mod tray;

use anyhow::{Context, Result};
use app::{Agent, Event, Status};
use mc_core::config::{Config, ControlRole, Role};
use mc_core::crypto::Psk;
use mc_core::hotkey::Hotkey;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

/// The platform name reported to the peer.
pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MultControl")
}

fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("MULTControl_CONFIG") {
        return PathBuf::from(path);
    }
    data_dir().join("config.toml")
}

fn log_path() -> PathBuf {
    data_dir().join("agent.log")
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .map(|mut name| {
            if let Some((head, _)) = name.split_once('.') {
                name = head.to_string();
            }
            name
        })
        .unwrap_or_else(|_| "multcontrol".to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("run");
    let code = match command {
        "run" | "start" => {
            if let Err(e) = run_agent() {
                eprintln!("multcontrol: {e:#}");
                1
            } else {
                0
            }
        }
        "toggle" | "local" | "remote" | "status" | "quit" => {
            match control_socket::request(command) {
                Ok(reply) => {
                    println!("{reply}");
                    0
                }
                Err(e) => {
                    eprintln!("multcontrol: no running agent ({e})");
                    1
                }
            }
        }
        "psk" => match load_or_create_config() {
            Ok((cfg, _)) => {
                println!("{}", cfg.network.psk);
                0
            }
            Err(e) => {
                eprintln!("multcontrol: {e:#}");
                1
            }
        },
        "where" => {
            println!("config: {}", config_path().display());
            println!("log:    {}", log_path().display());
            0
        }
        "discover" => match net::discover(47777) {
            Some(addr) => {
                println!("{addr}");
                0
            }
            None => {
                eprintln!("no peer answered the discovery probe");
                1
            }
        },
        "doctor" => doctor(),
        "selftest" => selftest::run(),
        "version" | "--version" | "-V" => {
            println!("multcontrol {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "--help" | "-h" => {
            usage();
            0
        }
        other => {
            eprintln!("multcontrol: unknown command `{other}`");
            usage();
            2
        }
    };
    std::process::exit(code);
}

fn usage() {
    println!(
        "multcontrol {}\n\
         \n\
         usage: multcontrol [command]\n\
         \n\
         commands:\n\
         \x20 run       start the background agent (default)\n\
         \x20 status    print what the running agent is doing\n\
         \x20 toggle    switch which machine receives the keyboard and mouse\n\
         \x20 local     take the keyboard and mouse on this machine\n\
         \x20 remote    send the keyboard and mouse to the peer\n\
         \x20 quit      stop the running agent\n\
         \x20 psk       print this machine's pairing key\n\
         \x20 where     print the config and log locations\n\
         \x20 discover  find the Mac on the local network\n\
         \x20 doctor    check permissions, ports and config\n\
         \x20 selftest  verify this binary works here (installs nothing)",
        env!("CARGO_PKG_VERSION")
    );
}

fn init_logging(verbose: bool) {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(log::LevelFilter::Info);
    if verbose {
        builder.target(env_logger::Target::Stderr);
    } else {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Keep the log from growing without bound; one old file is enough.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 5 * 1024 * 1024 {
                let _ = std::fs::rename(&path, path.with_extension("log.old"));
            }
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                // The log can mention peer names and addresses; keep it private.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
                }
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            Err(_) => {
                builder.target(env_logger::Target::Stderr);
            }
        }
    }
    let _ = builder.format_timestamp_millis().try_init();
}

/// Load the config, creating a usable default on first run.
fn load_or_create_config() -> Result<(Config, PathBuf)> {
    let path = config_path();
    if path.exists() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg =
            Config::from_toml(&text).with_context(|| format!("parsing {}", path.display()))?;
        return Ok((cfg, path));
    }

    let mut cfg = Config::default();
    if cfg!(windows) {
        cfg.network.role = Role::Client;
        cfg.control.role = ControlRole::Controller;
    } else {
        cfg.network.role = Role::Server;
        cfg.control.role = ControlRole::Target;
        cfg.network.listen_addr = format!("0.0.0.0:{}", cfg.network.port);
    }
    cfg.network.psk = mc_core::crypto::generate_psk_hex();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file =
        std::fs::File::create(&path).with_context(|| format!("writing {}", path.display()))?;
    file.write_all(cfg.to_toml().as_bytes())?;
    println!(
        "First run: wrote a new configuration to {}\n\
         Pairing key (paste into the other machine's config): {}\n",
        path.display(),
        cfg.network.psk
    );
    Ok((cfg, path))
}

fn run_agent() -> Result<()> {
    let verbose = std::env::args().any(|a| a == "--verbose" || a == "-v");
    init_logging(verbose);

    let (cfg, path) = load_or_create_config()?;
    log::info!(
        "multcontrol {} on {} ({})",
        env!("CARGO_PKG_VERSION"),
        hostname(),
        platform_name()
    );
    log::info!("config: {}", path.display());

    let hotkey = Hotkey::parse(&cfg.control.hotkey)
        .map_err(|e| anyhow::anyhow!("invalid hotkey {:?}: {e}", cfg.control.hotkey))?;
    let psk = Psk::from_hex(&cfg.network.psk).map_err(|e| {
        anyhow::anyhow!(
            "network.psk in {} is not valid hex ({e}); run `multcontrol psk` on the other machine",
            path.display()
        )
    })?;

    let backend = input::platform_backend()?;
    let (events_tx, events_rx) = channel::<Event>();
    let (status_tx, status_rx) = channel::<Status>();

    // Bind the loopback channel first: it doubles as a single-instance lock,
    // so logging in twice cannot install two sets of input hooks.
    let status_text = Arc::new(std::sync::Mutex::new(String::from("starting")));
    {
        let (command_tx, command_rx) = channel::<control_socket::Command>();
        let events_tx = events_tx.clone();
        std::thread::Builder::new()
            .name("mc-control-bridge".into())
            .spawn(move || {
                for command in command_rx {
                    if events_tx.send(Event::Command(command)).is_err() {
                        break;
                    }
                }
            })
            .context("spawning control bridge")?;
        let status_text = status_text.clone();
        control_socket::spawn(
            command_tx,
            Arc::new(move || status_text.lock().unwrap().clone()),
        )
        .context("another multcontrol agent is already running")?;
    }

    // Network, bridged into the single control channel.
    let (net_tx, net_rx) = channel::<net::NetEvent>();
    {
        let events_tx = events_tx.clone();
        std::thread::Builder::new()
            .name("mc-net-bridge".into())
            .spawn(move || {
                for event in net_rx {
                    if events_tx.send(Event::Net(event)).is_err() {
                        break;
                    }
                }
            })
            .context("spawning network bridge")?;
    }
    let _net_handle = net::spawn(
        cfg.network.clone(),
        psk,
        hostname(),
        "us".to_string(),
        net_tx,
    );

    // Heartbeat: a write every ten seconds proves the peer is still there and
    // keeps the connection alive through any stateful NAT.
    {
        let events_tx = events_tx.clone();
        std::thread::Builder::new()
            .name("mc-heartbeat".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                if events_tx.send(Event::Heartbeat).is_err() {
                    return;
                }
            })
            .context("spawning heartbeat")?;
    }

    // Clipboard poller.
    if cfg.input.sync_clipboard {
        spawn_clipboard_poller(events_tx.clone(), cfg.input.clipboard_poll_ms);
    }

    // Local control channel, so `multcontrol status` and friends work.

    let status_cb = {
        let status_tx = status_tx.clone();
        let status_text = status_text.clone();
        Arc::new(move |status: Status| {
            *status_text.lock().unwrap() = status.line();
            let _ = status_tx.send(status);
        })
    };

    let agent = Agent::new(cfg.clone(), backend.clone(), hotkey, events_rx, status_cb);
    agent.start_capture(events_tx.clone())?;

    // Prompt for Accessibility (macOS) — side effects only, result handled by UI.
    let _ = check_permission();

    let running = Arc::new(AtomicBool::new(true));
    let agent_running = running.clone();
    let agent_thread = std::thread::Builder::new()
        .name("mc-agent".into())
        .spawn(move || {
            agent.run();
            agent_running.store(false, Ordering::SeqCst);
        })
        .context("spawning control loop")?;

    // Bridge GUI menu commands (Toggle/Local/Remote/Quit) into the agent's
    // control channel so the tray icon / menu bar can drive the agent.
    let (gui_tx, gui_rx) = channel::<gui::GuiCommand>();
    let events_tx_for_gui = events_tx.clone();
    std::thread::Builder::new()
        .name("mc-gui-bridge".into())
        .spawn(move || {
            for cmd in gui_rx {
                let command = match cmd {
                    gui::GuiCommand::Toggle => control_socket::Command::Toggle,
                    gui::GuiCommand::Local => control_socket::Command::Local,
                    gui::GuiCommand::Remote => control_socket::Command::Remote,
                    gui::GuiCommand::Quit => control_socket::Command::Quit,
                };
                if events_tx_for_gui.send(Event::Command(command)).is_err() {
                    break;
                }
            }
        })
        .context("spawning GUI bridge")?;

    // The UI owns the main thread (NSApplication on macOS, message loop on Windows).
    gui::run(status_rx, gui_tx, running.clone());

    let _ = events_tx.send(Event::Command(control_socket::Command::Quit));
    let _ = agent_thread.join();
    input::shutdown();
    Ok(())
}

fn spawn_clipboard_poller(tx: Sender<Event>, poll_ms: u64) {
    std::thread::Builder::new()
        .name("mc-clipboard".into())
        .spawn(move || {
            let mut last: Option<String> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(poll_ms.max(50)));
                let current = clipboard::get_text();
                if current != last {
                    last = current.clone();
                    if let Some(text) = current {
                        if tx.send(Event::Clipboard(text)).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .ok();
}

fn check_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        let trusted = input::accessibility_trusted();
        if !trusted {
            log::warn!(
                "Accessibility permission is not granted. macOS will ask; enable MultControl in \
                 System Settings > Privacy & Security > Accessibility. The agent retries \
                 automatically."
            );
            // This is what adds the app to the Accessibility list; without it
            // the user has to add the binary by hand.
            input::prompt_for_accessibility();
        }
        trusted
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

fn doctor() -> i32 {
    let mut ok = true;
    let mut port = Config::default().network.port;
    println!("multcontrol doctor");
    println!("  platform: {}", platform_name());

    match load_or_create_config() {
        Ok((cfg, path)) => {
            println!("  config:   {}", path.display());
            println!(
                "  role:     network={:?} control={:?}",
                cfg.network.role, cfg.control.role
            );
            println!(
                "  peer:     {}",
                cfg.network
                    .peer_addr
                    .clone()
                    .unwrap_or_else(|| "auto (LAN discovery)".to_string())
            );
            port = cfg.network.port;
            match Psk::from_hex(&cfg.network.psk) {
                Ok(_) => println!("  psk:      ok ({} bytes)", 32),
                Err(e) => {
                    println!("  psk:      INVALID ({e})");
                    ok = false;
                }
            }
            if let Err(e) = Hotkey::parse(&cfg.control.hotkey) {
                println!("  hotkey:   INVALID ({e})");
                ok = false;
            } else {
                println!("  hotkey:   {}", cfg.control.hotkey);
            }
        }
        Err(e) => {
            println!("  config:   ERROR {e:#}");
            ok = false;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if input::accessibility_trusted() {
            println!("  access:   Accessibility granted");
        } else {
            println!("  access:   MISSING — enable MultControl in System Settings > Privacy & Security > Accessibility");
            ok = false;
        }
    }

    if control_socket::agent_running() {
        println!("  agent:    running");
        if let Ok(status) = control_socket::request("status") {
            println!("  status:   {status}");
        }
    } else {
        println!("  agent:    not running");
    }

    println!(
        "  ports:    tcp/{port}, udp/47777 (discovery), tcp/{} (local control)",
        control_socket::CONTROL_PORT
    );

    if ok {
        println!("\nall checks passed");
        0
    } else {
        println!("\nfix the items marked above");
        1
    }
}
