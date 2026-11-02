//! Configuration file handling.

use serde::{Deserialize, Serialize};
use std::io;

/// Which side of the connection this agent plays.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Dial out to the peer. Used on Windows, where inbound connections would
    /// need a firewall rule (and therefore administrator rights).
    #[default]
    Client,
    /// Listen for the peer. Used on the Mac.
    Server,
}

/// Whether this machine owns the physical keyboard and mouse.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlRole {
    /// Holds the physical keyboard and mouse; decides where input goes.
    #[default]
    Controller,
    /// Receives synthetic input from the controller.
    Target,
}

/// Network settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// Client or server.
    pub role: Role,
    /// Address to listen on when acting as server.
    pub listen_addr: String,
    /// Address of the peer when acting as client.
    pub peer_addr: Option<String>,
    /// Broadcast a discovery probe on the local network to find the peer.
    pub discovery: bool,
    /// UDP port used for discovery.
    pub discovery_port: u16,
    /// TCP port used for the control connection.
    pub port: u16,
    /// Pre-shared key, hex encoded. Both machines must share it.
    pub psk: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            role: Role::Client,
            listen_addr: "0.0.0.0:47778".to_string(),
            peer_addr: None,
            discovery: true,
            discovery_port: 47777,
            port: 47778,
            psk: String::new(),
        }
    }
}

/// Switching behaviour.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    /// Whether this machine owns the physical keyboard and mouse.
    pub role: ControlRole,
    /// Global hotkey that toggles which machine receives the input.
    pub hotkey: String,
    /// Switch when the pointer is pushed past a screen edge.
    pub edge_push: bool,
    /// How close to the edge (in pixels) counts as "at the edge".
    pub edge_margin: i32,
    /// Ignore repeated hotkey presses within this many milliseconds.
    pub hotkey_debounce_ms: u64,
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            role: ControlRole::Controller,
            hotkey: "ctrl+alt+space".to_string(),
            edge_push: true,
            edge_margin: 2,
            hotkey_debounce_ms: 400,
        }
    }
}

/// Input and clipboard behaviour.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Hide the local pointer while the peer is being controlled.
    pub hide_cursor_when_remote: bool,
    /// Mirror clipboard contents between the two machines.
    pub sync_clipboard: bool,
    /// Paste automatically on the peer when the clipboard changes.
    ///
    /// Off by default: pasting into whatever window happens to be focused is
    /// surprising. Most people prefer to press the paste shortcut themselves.
    pub auto_paste: bool,
    /// How often to poll the clipboard for changes, in milliseconds.
    pub clipboard_poll_ms: u64,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            hide_cursor_when_remote: true,
            sync_clipboard: true,
            auto_paste: false,
            clipboard_poll_ms: 500,
        }
    }
}

/// Top level configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Network settings.
    pub network: NetworkConfig,
    /// Switching behaviour.
    pub control: ControlConfig,
    /// Input and clipboard behaviour.
    pub input: InputConfig,
}

impl Config {
    /// Parse a configuration file.
    pub fn from_toml(text: &str) -> io::Result<Config> {
        toml::from_str(text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Render the configuration as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_gets_defaults() {
        let cfg = Config::from_toml("[network]\nrole = \"server\"\n").unwrap();
        assert_eq!(cfg.network.role, Role::Server);
        assert_eq!(cfg.network.port, 47778);
        assert_eq!(cfg.control.hotkey, "ctrl+alt+space");
        assert!(cfg.input.sync_clipboard);
    }

    #[test]
    fn config_roundtrips() {
        let cfg = Config::from_toml(
            r#"
[network]
role = "client"
peer_addr = "192.168.1.50:47778"
psk = "abcd"

[control]
hotkey = "ctrl+alt+f12"
edge_push = false
"#,
        )
        .unwrap();
        let text = cfg.to_toml();
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(back.network.peer_addr, cfg.network.peer_addr);
        assert_eq!(back.control.hotkey, "ctrl+alt+f12");
        assert!(!back.control.edge_push);
    }
}
