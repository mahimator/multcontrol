#!/usr/bin/env bash
# Install the multcontrol agent on a Mac, per user, with no administrator rights.
#
#   ./scripts/install-macos.sh
#
# What it does:
#   * builds the release binary
#   * wraps it in ~/Applications/MultControl.app (LSUIElement, no Dock icon)
#   * creates ~/Library/Application Support/MultControl/config.toml on first run
#   * installs and starts a per-user LaunchAgent
#
# The only permission macOS asks for is Accessibility, which is a per-user
# privacy setting (System Settings > Privacy & Security > Accessibility).

set -euo pipefail

BUNDLE_ID="com.multcontrol.agent"
APP_NAME="MultControl"
APP_DIR="$HOME/Applications/$APP_NAME.app"
SUPPORT_DIR="$HOME/Library/Application Support/MultControl"
CONFIG="$SUPPORT_DIR/config.toml"
AGENT_PLIST="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_SOURCE="$REPO_ROOT/target/release/multcontrol"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This script is for macOS. On Windows use scripts/install-windows.ps1." >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    else
        echo "cargo not found. Install Rust first: https://rustup.rs" >&2
        exit 1
    fi
fi

echo "==> Building"
(cd "$REPO_ROOT" && cargo build --release -p mc-agent)

echo "==> Installing $APP_NAME.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$BIN_SOURCE" "$APP_DIR/Contents/MacOS/multcontrol"
cp "$REPO_ROOT/packaging/Info.plist" "$APP_DIR/Contents/Info.plist"
chmod +x "$APP_DIR/Contents/MacOS/multcontrol"

# Ad-hoc signing gives TCC a stable identity to remember. Without it macOS can
# forget the Accessibility grant between launches.
if codesign --force --sign - "$APP_DIR" >/dev/null 2>&1; then
    echo "    ad-hoc signed"
else
    echo "    warning: could not ad-hoc sign; macOS may ask for Accessibility again"
fi

echo "==> Configuration"
mkdir -p "$SUPPORT_DIR"
FIRST_RUN=0
if [[ ! -f "$CONFIG" ]]; then
    FIRST_RUN=1
    if command -v openssl >/dev/null 2>&1; then
        PSK="$(openssl rand -hex 32)"
    else
        PSK="$(LC_ALL=C tr -dc 'a-f0-9' </dev/urandom 2>/dev/null | head -c 64 || true)"
    fi
    cat >"$CONFIG" <<EOF
# multcontrol on the Mac.
#
# This machine listens for the PC and accepts input from it.
# Copy the pairing key below into the PC's config.toml as network.psk.

[network]
role = "server"
listen_addr = "0.0.0.0:47778"
discovery = true
discovery_port = 47777
port = 47778
psk = "$PSK"

[control]
role = "target"
hotkey = "ctrl+alt+space"
edge_push = true
edge_margin = 2
hotkey_debounce_ms = 400

[input]
hide_cursor_when_remote = true
sync_clipboard = true
auto_paste = false
clipboard_poll_ms = 500
EOF
    chmod 600 "$CONFIG"
fi

echo "==> LaunchAgent"
mkdir -p "$(dirname "$AGENT_PLIST")"
cat >"$AGENT_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$BUNDLE_ID</string>
    <key>ProgramArguments</key>
    <array>
        <string>$APP_DIR/Contents/MacOS/multcontrol</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <!-- Restart after a crash, but let the quit command stay quit. -->
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>MULTControl_NO_PROMPT</key>
        <string>1</string>
    </dict>
    <key>StandardOutPath</key>
    <string>$SUPPORT_DIR/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>$SUPPORT_DIR/stderr.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST"
launchctl enable "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1 || true

# A convenient CLI on PATH, if the user keeps ~/.local/bin around.
mkdir -p "$HOME/.local/bin"
ln -sf "$APP_DIR/Contents/MacOS/multcontrol" "$HOME/.local/bin/multcontrol"

echo
if [[ "$FIRST_RUN" == "1" ]]; then
    echo "Pairing key (put this in the PC's config.toml as network.psk):"
    echo
    sed -n 's/^psk = "\(.*\)"/  \1/p' "$CONFIG"
    echo
fi
cat <<EOF
Done. Look for "MC" in the menu bar.

Next steps:
  1. Open System Settings > Privacy & Security > Accessibility and enable
     MultControl (macOS will also prompt on first launch).
  2. Click "Allow" if macOS asks whether multcontrol may accept incoming
     network connections. Declining blocks the PC.
  3. On the Windows PC, install with scripts/install-windows.ps1 and paste
     the pairing key above.

Handy commands:
  ~/.local/bin/multcontrol status
  ~/.local/bin/multcontrol toggle
  ~/.local/bin/multcontrol quit
  tail -f "$SUPPORT_DIR/agent.log"
EOF
