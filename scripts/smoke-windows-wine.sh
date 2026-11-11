#!/bin/bash
# Two-agent end-to-end test for the Windows build, driven through Wine.
#
# This mirrors scripts/smoke-windows.ps1: one agent plays the Mac-side target,
# the other the PC-side controller. It checks the handshake, the status
# reporting and a full toggle round-trip.
set -uo pipefail

cd /Users/madhavtrivedi/multcontrol
EXE="target/x86_64-pc-windows-gnu/release/multcontrol.exe"
export WINEPREFIX="${WINEPREFIX:-$HOME/wine-mc}"
export WINEDEBUG=-all

WORK=$(mktemp -d "${TMPDIR:-/tmp}/mcwine.XXXXXX")
PSK="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
TCP=47810
SERVER_CTL=47811
CLIENT_CTL=47812

cat >"$WORK/server.toml" <<EOF
[network]
role = "server"
listen_addr = "127.0.0.1:$TCP"
discovery = false
discovery_port = 47813
port = $TCP
psk = "$PSK"

[control]
role = "target"
hotkey = "ctrl+alt+space"
edge_push = false
edge_margin = 2
hotkey_debounce_ms = 400

[input]
hide_cursor_when_remote = false
sync_clipboard = false
auto_paste = false
clipboard_poll_ms = 500
EOF

cat >"$WORK/client.toml" <<EOF
[network]
role = "client"
peer_addr = "127.0.0.1:$TCP"
discovery = false
discovery_port = 47813
port = $TCP
psk = "$PSK"

[control]
role = "controller"
hotkey = "ctrl+alt+space"
edge_push = false
edge_margin = 2
hotkey_debounce_ms = 400

[input]
hide_cursor_when_remote = false
sync_clipboard = false
auto_paste = false
clipboard_poll_ms = 500
EOF

failures=0
ok()   { echo "    [ok]   $1"; }
bad()  { echo "    [FAIL] $1"; failures=$((failures + 1)); }

ctl() { # ctl <config> <port> <command>
    MULTControl_CONFIG="$1" MULTControl_CONTROL_PORT="$2" \
        wine64 "$EXE" "$3" 2>/dev/null | tr -d '\r'
}

wait_for() { # wait_for <config> <port> <needle> <seconds>
    local deadline=$(( $(date +%s) + $4 ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        local s
        s=$(ctl "$1" "$2" status)
        case "$s" in *"$3"*) echo "$s"; return 0 ;; esac
        sleep 0.5
    done
    return 1
}

echo "==> starting the target (server)"
MULTControl_CONFIG="$WORK/server.toml" MULTControl_CONTROL_PORT="$SERVER_CTL" \
    wine64 "$EXE" run >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
sleep 3

echo "==> starting the controller (client)"
MULTControl_CONFIG="$WORK/client.toml" MULTControl_CONTROL_PORT="$CLIENT_CTL" \
    wine64 "$EXE" run >"$WORK/client.log" 2>&1 &
CLIENT_PID=$!
sleep 3

cleanup() {
    ctl "$WORK/client.toml" "$CLIENT_CTL" quit >/dev/null 2>&1
    ctl "$WORK/server.toml" "$SERVER_CTL" quit >/dev/null 2>&1
    sleep 1
    kill $SERVER_PID $CLIENT_PID 2>/dev/null
    wait $SERVER_PID $CLIENT_PID 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

echo "==> handshake"
if s=$(wait_for "$WORK/client.toml" "$CLIENT_CTL" "connected to" 20); then
    ok "controller sees the peer: $s"
else
    bad "controller never connected"
    echo "--- server log ---"; cat "$WORK/server.log"
    echo "--- client log ---"; cat "$WORK/client.log"
    exit 1
fi

if s=$(wait_for "$WORK/server.toml" "$SERVER_CTL" "connected to" 15); then
    ok "target sees the controller: $s"
else
    bad "target never saw the controller"
fi

echo "==> toggle to the target"
reply=$(ctl "$WORK/client.toml" "$CLIENT_CTL" toggle)
[ "$reply" = "ok" ] && ok "toggle acknowledged" || bad "toggle reply was '$reply'"

if s=$(wait_for "$WORK/client.toml" "$CLIENT_CTL" "input -> the Mac" 10); then
    ok "controller reports remote: $s"
else
    bad "controller did not switch to remote"
fi

if s=$(wait_for "$WORK/server.toml" "$SERVER_CTL" "controlled by the PC" 10); then
    ok "target reports controlled: $s"
else
    bad "target did not see the hand-over"
fi

echo "==> toggle back"
reply=$(ctl "$WORK/client.toml" "$CLIENT_CTL" toggle)
[ "$reply" = "ok" ] && ok "toggle acknowledged" || bad "toggle reply was '$reply'"

if s=$(wait_for "$WORK/client.toml" "$CLIENT_CTL" "input -> this PC" 10); then
    ok "controller reports local: $s"
else
    bad "controller did not switch back"
fi

if s=$(wait_for "$WORK/server.toml" "$SERVER_CTL" "input -> idle" 10); then
    ok "target reports idle: $s"
else
    bad "target did not go back to idle"
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "result: PASS"
    exit 0
fi
echo "result: FAIL ($failures check(s) failed)"
exit 1
