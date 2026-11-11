#!/usr/bin/env bash
# Build the macOS app bundle into dist/MultControl.app.
#
# Kept separate from build-dmg.sh so the signing step can sign the bundle and
# then let the disk image be built from the signed copy.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    else
        echo "cargo not found. Install Rust first: https://rustup.rs" >&2
        exit 1
    fi
fi

if [ ! -f assets/multcontrol.ico ] || [ ! -d assets/MultControl.iconset ]; then
    echo "==> Generating the app icon"
    python3 scripts/make-icon.py assets
fi
if [ ! -f assets/MultControl.icns ]; then
    iconutil --convert icns assets/MultControl.iconset --output assets/MultControl.icns
fi

echo "==> Building the agent"
cargo build --release -p mc-agent

APP="dist/MultControl.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp packaging/Info.plist "$APP/Contents/Info.plist"
cp assets/MultControl.icns "$APP/Contents/Resources/AppIcon.icns"

# Prefer a universal binary so the same download runs on Intel and Apple
# silicon. Falling back is fine: the runner/developer just built one arch.
INTEL_TARGET="x86_64-apple-darwin"
NATIVE_BIN="target/release/multcontrol"
UNIVERSAL="$APP/Contents/MacOS/multcontrol"
if rustup target list --installed 2>/dev/null | grep -qx "$INTEL_TARGET"; then
    echo "    building the Intel slice too (universal app)"
    cargo build --release -p mc-agent --target "$INTEL_TARGET"
    lipo -create -output "$UNIVERSAL" \
        "$NATIVE_BIN" "target/$INTEL_TARGET/release/multcontrol"
else
    echo "    note: $INTEL_TARGET is not installed; the app is $(uname -m) only"
    cp "$NATIVE_BIN" "$UNIVERSAL"
fi
chmod +x "$UNIVERSAL"

# Sign with a real identity when one is available, otherwise ad-hoc.
if [ -n "${MULTControl_SIGN_IDENTITY:-}" ]; then
    echo "==> Signing with '$MULTControl_SIGN_IDENTITY'"
    codesign --force --deep --options runtime --timestamp \
        --sign "$MULTControl_SIGN_IDENTITY" "$APP"
else
    echo "==> Signing (ad-hoc)"
    codesign --force --sign - "$APP"
fi

codesign --verify --verbose=1 "$APP" 2>&1 | sed 's/^/    /'
echo "    $APP"
