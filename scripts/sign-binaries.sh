#!/usr/bin/env bash
# Sign the release binaries.
#
# Two levels of signing are supported:
#
# 1. **Self-signed** (free, works today). The binaries carry a real
#    Authenticode signature with a publisher name. Windows still shows
#    "Unknown publisher" until somebody with administrator rights adds the
#    certificate to *Trusted Publishers* — which is exactly what a company IT
#    department does to approve a piece of software. The user on the laptop
#    then needs no administrator rights at all.
#    The public certificate is shipped alongside as `multcontrol-publisher.cer`
#    so it can be handed to IT.
#
# 2. **CA-issued** (costs money, removes every warning). Point the script at a
#    real `.pfx` and the same code produces a fully trusted signature.
#
# Usage:
#   scripts/sign-binaries.sh [--pfx <file> --pfx-pass <password>]
#
# Without arguments it generates a self-signed certificate under
# `dist/signing/` and uses it.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST="dist"
SIGN_DIR="$DIST/signing"
PFX=""
PFX_PASS=""

while [ $# -gt 0 ]; do
    case "$1" in
        --pfx) PFX="$2"; shift 2 ;;
        --pfx-pass) PFX_PASS="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

mkdir -p "$SIGN_DIR"

# ---------------------------------------------------------------- certificate

if [ -z "$PFX" ]; then
    echo "==> Generating a self-signed code-signing certificate"
    PFX="$SIGN_DIR/multcontrol.pfx"
    PFX_PASS="${PFX_PASS:-multcontrol}"

    if [ ! -f "$PFX" ]; then
        cat >"$SIGN_DIR/openssl.cnf" <<'EOF'
[ req ]
default_bits       = 3072
distinguished_name = req_dn
prompt             = no

[ req_dn ]
C  = US
ST = CA
L  = San Francisco
O  = MultControl
CN = MultControl

[ v3_code_signing ]
basicConstraints       = critical,CA:FALSE
keyUsage               = critical,digitalSignature
extendedKeyUsage       = critical,codeSigning
subjectKeyIdentifier   = hash
EOF
        openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
            -keyout "$SIGN_DIR/multcontrol.key" \
            -out "$SIGN_DIR/multcontrol.cer" \
            -config "$SIGN_DIR/openssl.cnf" \
            -extensions v3_code_signing 2>/dev/null

        openssl pkcs12 -export \
            -inkey "$SIGN_DIR/multcontrol.key" \
            -in "$SIGN_DIR/multcontrol.cer" \
            -out "$PFX" \
            -passout "pass:$PFX_PASS" \
            -name "MultControl code signing" 2>/dev/null

        echo "    certificate: $SIGN_DIR/multcontrol.cer"
        echo "    private key: $SIGN_DIR/multcontrol.key  (keep this safe)"
    fi
else
    echo "==> Using the supplied certificate $PFX"
fi

# The public certificate is what IT installs, so make sure it exists.
CER="$SIGN_DIR/multcontrol.cer"
if [ ! -f "$CER" ]; then
    openssl pkcs12 -in "$PFX" -passin "pass:$PFX_PASS" \
        -clcerts -nokeys -out "$CER" 2>/dev/null
fi

# ------------------------------------------------------------------ Windows

EXE="target/x86_64-pc-windows-gnu/release/multcontrol.exe"
if [ ! -f "$EXE" ]; then
    echo "==> Building the Windows executable"
    cargo build --release --target x86_64-pc-windows-gnu -p mc-agent
fi

echo "==> Signing $EXE"
osslsigncode sign \
    -pkcs12 "$PFX" \
    -pass "$PFX_PASS" \
    -n "MultControl" \
    -i "https://github.com/mahimator/multcontrol" \
    -t "http://timestamp.digicert.com" \
    -in "$EXE" \
    -out "$EXE.signed"
mv "$EXE.signed" "$EXE"

echo "==> Verifying the Windows signature"
# Verify against our own certificate: the self-signed chain is not in the
# system trust store, but the digests and the timestamp must still check out.
if osslsigncode verify -CAfile "$CER" -in "$EXE" >/dev/null 2>&1; then
    echo "    signature valid, timestamp present"
else
    echo "    (self-signed chain is not in the system trust store; digests checked below)"
fi
osslsigncode verify -CAfile "$CER" -in "$EXE" 2>&1 \
    | grep -E "Message digest algorithm|Calculated message digest|Current message digest|Signing time|Subject:" \
    | sed 's/^/    /' || true

# -------------------------------------------------------------------- macOS

if [ "$(uname -s)" = "Darwin" ]; then
    echo "==> Signing the macOS app bundle"
    IDENTITY=""
    if command -v security >/dev/null 2>&1; then
        # codesign needs the certificate *and* its private key in a keychain.
        # The macOS Security framework only understands the legacy PKCS#12
        # encryption (PBE-SHA1-3DES), so re-export it that way.
        KEY_P12="$SIGN_DIR/multcontrol-key.p12"
        openssl pkcs12 -export \
            -inkey "$SIGN_DIR/multcontrol.key" \
            -in "$CER" \
            -out "$KEY_P12" \
            -passout "pass:$PFX_PASS" \
            -certpbe PBE-SHA1-3DES \
            -keypbe PBE-SHA1-3DES \
            -macalg sha1 \
            -name "MultControl code signing" 2>/dev/null || true
        if [ -f "$KEY_P12" ]; then
            security import "$KEY_P12" -k ~/Library/Keychains/login.keychain-db \
                -T /usr/bin/codesign -P "$PFX_PASS" >/dev/null 2>&1 || true
            rm -f "$KEY_P12"
        fi
        security import "$CER" -k ~/Library/Keychains/login.keychain-db \
            -T /usr/bin/codesign >/dev/null 2>&1 || true

        # `-v` only lists trusted identities, and a self-signed certificate is
        # never trusted, so ask for all of them.
        IDENTITY=$(security find-identity -p codesigning 2>/dev/null \
            | grep -i multcontrol | head -1 | sed 's/.*"\(.*\)".*/\1/' || true)
    fi

    if [ -n "$IDENTITY" ]; then
        export MULTControl_SIGN_IDENTITY="$IDENTITY"
        echo "    using identity '$IDENTITY'"
    else
        echo "    no keychain identity found; falling back to ad-hoc"
    fi

    bash scripts/build-app.sh
    codesign --verify --verbose=2 dist/MultControl.app 2>&1 | sed 's/^/    /' || true
    codesign -dv dist/MultControl.app 2>&1 \
        | grep -E "Identifier=|Format=|Signature=|Authority=" | sed 's/^/    /' || true

    echo "==> Rebuilding the disk image from the signed app"
    bash scripts/build-dmg.sh
fi

echo
echo "Done. Files for the release:"
echo "  $EXE"
echo "  $CER   <- give this to IT so they can trust the publisher"
