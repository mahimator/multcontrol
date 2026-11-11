#!/usr/bin/env bash
# Build a drag-and-run macOS disk image for a GitHub release.
#
# The DMG contains MultControl.app (universal when the Intel target is
# installed) plus an installer that sets up the LaunchAgent and prints the
# pairing key. Output: dist/MultControl-<version>-macos.dmg

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "This script runs on macOS (hdiutil is required)." >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    else
        echo "cargo not found. Install Rust first: https://rustup.rs" >&2
        exit 1
    fi
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "$VERSION" ]; then
    echo "could not read the version from Cargo.toml" >&2
    exit 1
fi

# Build (and sign) the app bundle.
bash scripts/build-app.sh

APP="dist/MultControl.app"

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/multcontrol-dmg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/MultControl.app"
cp packaging/install-from-dmg.command "$STAGE/Install MultControl.command"
chmod +x "$STAGE/Install MultControl.command"

cat >"$STAGE/Read Me.txt" <<EOF
MultControl $VERSION
====================

One keyboard and mouse for a Windows PC and this Mac.

Install
-------
1. Double-click "Install MultControl.command".
   If macOS refuses to open it ("unidentified developer"), right-click it,
   choose Open, then click Open again. That is Gatekeeper reacting to an
   unsigned build; the script only copies the app into your own account.

2. The script asks macOS for Accessibility permission. Click "Open System
   Settings" and switch MultControl on under
   Privacy & Security > Accessibility. This is a normal per-user privacy
   setting; no administrator password is needed.

3. Click "Allow" when macOS asks whether multcontrol may accept incoming
   network connections, then copy the pairing key the installer prints.

4. On the Windows PC, run install-windows.ps1 and paste that key.

The menu bar shows the MultControl icon. Hover over it to see the state
(idle, connected, offline) and press Ctrl+Alt+Space to hand the keyboard and
mouse over to the PC.

Verify without installing anything
----------------------------------
The app's binary is also the CLI:

  ~/Applications/MultControl.app/Contents/MacOS/multcontrol selftest

It checks the protocol, crypto, key tables and input hooks, and changes nothing.

Uninstall
---------
  ~/Applications/MultControl.app/Contents/MacOS/multcontrol quit
  launchctl bootout gui/\$(id -u)/com.multcontrol.agent
  rm -rf ~/Applications/MultControl.app \\
         ~/Library/Application\\ Support/MultControl \\
         ~/Library/LaunchAgents/com.multcontrol.agent.plist

More: https://github.com/mahimator/multcontrol
EOF

mkdir -p dist
DMG="dist/MultControl-$VERSION-macos.dmg"
rm -f "$DMG"
echo "==> Creating $DMG"
hdiutil create \
    -volname "MultControl $VERSION" \
    -srcfolder "$STAGE" \
    -ov -format UDZO \
    "$DMG" >/dev/null

hdiutil verify "$DMG" >/dev/null
echo "    $(du -h "$DMG" | cut -f1)  $DMG"
