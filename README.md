# multcontrol

One keyboard. One mouse. A Windows PC and a Mac.

`multcontrol` is a small, self-contained keyboard/mouse sharing agent for a
Windows PC and a Mac on the same LAN. The PC keeps the physical keyboard and
mouse; press **Ctrl+Alt+Space** (or push the pointer off the screen edge) and the
same devices drive the Mac. Press it again and they come back. The clipboard
follows along.

No cloud, no relay, no accounts. Two static binaries, one encrypted direct
connection over your local network.

```
   Windows PC (controller)                    Mac (target)
   ┌───────────────────────┐                  ┌───────────────────────┐
   │ low-level input hooks │  TCP 47778, TLS- │ CGEventTap + CGEvent  │
   │ capture + suppress    │  like AEAD, PSK  │ inject, never loop    │
   │ dials out (no admin)  │ ───────────────▶ │ listens               │
   └───────────────────────┘                  └───────────────────────┘
```

## Why the PC dials out

Windows cannot accept inbound connections without a firewall rule, and adding
one requires administrator rights. Outbound TCP is allowed by default, so the
Windows agent is the TCP **client** and the Mac is the **server**. Nothing is
installed as a service, no driver is loaded, and no firewall rule is touched on
either machine.

On the Mac, the agent runs as a per-user LaunchAgent and asks for one thing:
**Accessibility** permission, which is a normal per-user privacy setting.

## Requirements

* Windows 10 or 11, 64-bit. A normal user account — no administrator.
* macOS 11 or newer (Apple silicon or Intel). No administrator.
* Both machines on the same LAN.
* Rust 1.75+ to build. The binaries have no runtime dependencies.

## Install

### 1. Mac (the target)

```bash
git clone <this repo> multcontrol && cd multcontrol
./scripts/install-macos.sh
```

The script builds the agent, installs `~/Applications/MultControl.app`,
creates `~/Library/Application Support/MultControl/config.toml`, and starts a
per-user LaunchAgent. It prints a **pairing key** — copy it.

Then:

1. Open **System Settings → Privacy & Security → Accessibility** and enable
   **MultControl**. (macOS also shows a prompt on first launch; the agent starts
   capturing as soon as the switch is on.)
2. Click **Allow** if macOS asks whether `multcontrol` may accept incoming
   network connections. Declining this blocks the PC.

A small **MC** item appears in the menu bar. It shows `MC offline`, `MC idle`,
`MC controlled`, or `MC !` when Accessibility is missing.

### 2. Windows PC (the controller)

Build on the Windows machine:

```powershell
cargo build --release -p mc-agent
powershell -ExecutionPolicy Bypass -File .\scripts\install-windows.ps1
```

The installer asks for the Mac's pairing key and (optionally) its LAN address,
copies the binary to `%LOCALAPPDATA%\Programs\MultControl`, writes the config to
`%LOCALAPPDATA%\MultControl\config.toml`, and registers a
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` entry so it starts at
login. It then starts the agent in the background.

If you leave the address blank, the agent finds the Mac with a UDP broadcast on
port 47777. A static address is more reliable if your router blocks broadcasts.

### Cross-compiling the Windows binary from macOS

Optional. You need mingw-w64 (`brew install mingw-w64`):

```bash
rustup target add x86_64-pc-windows-gnu
cargo build --release -p mc-agent --target x86_64-pc-windows-gnu
```

Copy `target/x86_64-pc-windows-gnu/release/multcontrol.exe` to the PC and run the
installer with `-Source`.

## Using it

| Action | What happens |
| --- | --- |
| **Ctrl+Alt+Space** | Hand the keyboard and mouse to the other machine and back |
| Move the pointer off the right/left/top/bottom edge | Hand over control through that edge (the other machine's pointer continues from the opposite edge) |
| Press the hotkey on the Mac | Asks the PC for control (or gives it back) |
| Copy on one machine | The text appears on the other machine's clipboard |
| Copy on one machine, paste on the other | The text is on the other clipboard, ready for the normal paste shortcut |

The hotkey works on both machines. Edge pushing is one-way by design: the PC
owns the physical devices, so it decides where they go, and the Mac can only
*ask* (hotkey) or *hand back* (edge).

### Commands

The same binary is also the CLI. On the Mac it is symlinked to
`~/.local/bin/multcontrol`; on Windows use the full path to
`multcontrol.exe`.

```
multcontrol status     what the running agent is doing
multcontrol toggle     switch machines
multcontrol local      take the keyboard and mouse here
multcontrol remote     send them to the peer
multcontrol quit       stop the agent (launchd will not restart a clean exit)
multcontrol doctor     check config, permissions and ports
multcontrol psk        print this machine's pairing key
multcontrol discover   find the peer with a UDP probe
multcontrol where      print config and log paths
```

## Configuration

`config.toml`, on the Mac at
`~/Library/Application Support/MultControl/config.toml`, on Windows at
`%LOCALAPPDATA%\MultControl\config.toml`. Changes take effect after restarting
the agent.

```toml
[network]
role = "server"              # "server" on the Mac, "client" on the PC
listen_addr = "0.0.0.0:47778"
peer_addr = "192.168.1.20:47778"   # optional; omit to use discovery
discovery = true             # UDP broadcast probe on port 47777
discovery_port = 47777
port = 47778
psk = "…64 hex characters…"  # must match on both machines

[control]
role = "target"              # "controller" on the PC, "target" on the Mac
hotkey = "ctrl+alt+space"
edge_push = true
edge_margin = 2              # pixels from the edge that count as "at the edge"
hotkey_debounce_ms = 400

[input]
hide_cursor_when_remote = true   # hide the pointer on the machine not in use
sync_clipboard = true
auto_paste = false               # true = paste immediately on the Mac when the PC copies
clipboard_poll_ms = 500
```

Hotkeys accept `ctrl`, `alt`, `shift`, `meta`/`cmd`/`super`/`win` plus a key
name: `ctrl+alt+space`, `ctrl+alt+f12`, `ctrl+shift+backquote`, …

## Troubleshooting

**The menu bar says `MC !`.**
Accessibility permission is missing. Enable MultControl under Privacy &
Security → Accessibility. The agent notices within a couple of seconds and
installs its event tap; no restart is needed. (If it stays on `MC !`, the binary
was rebuilt and macOS is holding an old grant — remove the entry with the minus
button and enable it again.)

**The PC shows `connected` but nothing types on the Mac.**
Accessibility again: without it the Mac can neither post events nor suppress
its own. Check `multcontrol doctor` on the Mac.

**The PC never connects.**
Run `multcontrol discover` on the PC. If it finds nothing, put the Mac's address
in `peer_addr`. Check that both configs have the same `psk` (a mismatch is
logged as *peer failed authentication: wrong pre-shared key?*).

**macOS asked about incoming connections and I clicked "Don't Allow".**
System Settings → Network → Firewall → Options, or reset it with
`sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add
~/Applications/MultControl.app/Contents/MacOS/multcontrol`.

**The pointer stops at the edge of the Mac screen.**
That is the hand-back gesture. Push a little further (or press the hotkey) to
return to the PC.

**Logs.** Mac: `~/Library/Application Support/MultControl/agent.log`, plus
`stdout.log`/`stderr.log` from launchd. Windows:
`%LOCALAPPDATA%\MultControl\agent.log`.

## Security

* Traffic is encrypted with ChaCha20-Poly1305. Keys are derived with HKDF-SHA256
  from a 32-byte pre-shared key and fresh nonces from both sides.
* The handshake is mutually authenticated with HMAC-SHA256 tags; a wrong key is
  rejected before any input is accepted.
* Each frame's type is authenticated as associated data, so frames cannot be
  replayed as a different type.
* Nonces are per-direction counters, and keys are per-session, so a recorded
  session cannot be replayed into a new one.
* Anyone on your LAN who knows the pairing key can control the target machine.
  Treat the key like a password; it is stored in a `chmod 600` file on the Mac.
* There is no relay and no internet exposure: the agent only talks to the
  address it is configured with or discovers on the LAN.

## Building and testing

```bash
cargo test                     # protocol, crypto, key tables, state machine
cargo build --release -p mc-agent
cargo check --target x86_64-pc-windows-gnu -p mc-agent   # type-check the Windows side
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design, the wire
protocol, and the threading model.

## Limitations (v1)

* Keyboard, mouse and clipboard text only — no file transfer, no audio.
* Single monitor per machine (the virtual desktop is used as one rectangle).
* The controller is the machine with the physical devices; only one target.
* Characters are sent as physical key positions, so both machines should use the
  same keyboard layout for symbols to match.
* No auto-update and no installer signing; macOS may ask for Accessibility again
  after a rebuild because the ad-hoc signature changes.
