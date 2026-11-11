#!/bin/bash
# Install MultControl from the disk image.
#
# Double-click this file. Everything stays inside your user account: the app
# goes to ~/Applications, the config to ~/Library/Application Support, and
# autostart is a per-user LaunchAgent. macOS may ask for confirmation the first
# time because the build is not signed with a paid Developer ID.

set -euo pipefail

BUNDLE_ID="com.multcontrol.agent"
APP_DIR="$HOME/Applications/MultControl.app"
SUPPORT_DIR="$HOME/Library/Application Support/MultControl"
CONFIG="$SUPPORT_DIR/config.toml"
AGENT_PLIST="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"

pause() {
    if [ -t 0 ]; then
        read -r -p "Press Return to close this window. " _ || true
        echo
    fi
}

die() {
    printf 'error: %s\n' "$*" >&2
    pause
    exit 1
}

say() {
    printf '%s\n' "$*"
}

HERE="$(cd "$(dirname "$0")" && pwd)"
APP_SRC="$HERE/MultControl.app"

[ "$(uname -s)" = "Darwin" ] || die "this installer is for macOS"
[ -d "$APP_SRC" ] || die "MultControl.app is missing from the disk image"

say "==> Installing $APP_DIR"
mkdir -p "$HOME/Applications"
rm -rf "$APP_DIR"
cp -R "$APP_SRC" "$APP_DIR" || die "could not copy the app"
# A downloaded app carries a quarantine flag. This build is ad-hoc signed (no
# paid Developer ID), so clear it or Gatekeeper refuses to launch the agent.
xattr -dr com.apple.quarantine "$APP_DIR" 2>/dev/null || true

say "==> Configuration"
mkdir -p "$SUPPORT_DIR"
FIRST_RUN=0
if [ ! -f "$CONFIG" ]; then
    FIRST_RUN=1
    if command -v openssl >/dev/null 2>&1; then
        PSK="$(openssl rand -hex 32)"
    else
        PSK="$(LC_ALL=C tr -dc 'a-f0-9' </dev/urandom | head -c 64)"
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

say "==> Login item"
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
    <key>StandardOutPath</key>
    <string>$SUPPORT_DIR/stdout.log</string>
    <key>StandardErrorPath</key>
    <string>$SUPPORT_DIR/stderr.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST" || die "could not start the login item"
launchctl enable "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1 || true

mkdir -p "$HOME/.local/bin"
ln -sf "$APP_DIR/Contents/MacOS/multcontrol" "$HOME/.local/bin/multcontrol"

if [ "$FIRST_RUN" = "1" ]; then
    echo
    echo "Pairing key (put this in the PC's config.toml as network.psk):"
    echo
    sed -n 's/^psk = "\(.*\)"/  \1/p' "$CONFIG"
    echo
fi

cat <<EOF
Done. MultControl is now in your menu bar (look for the mouse-and-screen icon).
Click it to see the state; press Ctrl+Alt+Space to hand the keyboard over.

Next steps:
  1. Click "Open System Settings" on the macOS permission prompt and enable
     MultControl under Privacy & Security > Accessibility. The agent starts
     capturing within a couple of seconds; no restart needed.
  2. Click "Allow" if macOS asks whether multcontrol may accept incoming
     network connections. Declining blocks the PC.
  3. On the Windows PC, run install-windows.ps1 (or copy multcontrol.exe and
     create the config by hand) and paste the pairing key above.

Handy commands:
  ~/.local/bin/multcontrol status
  ~/.local/bin/multcontrol toggle
  ~/.local/bin/multcontrol quit
  tail -f "$SUPPORT_DIR/agent.log"
EOF

if [ "$FIRST_RUN" = "1" ]; then
    open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" >/dev/null 2>&1 || true
fi
