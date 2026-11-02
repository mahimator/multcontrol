# Architecture

## Pieces

| Crate | Role |
| --- | --- |
| `mc-core` | Platform-independent: key model, wire protocol, crypto, handshake, config, hotkey parsing |
| `mc-agent` | The daemon: platform input backends, networking, control loop, clipboard, UI |

`mc-agent` is a single binary with two personalities, chosen by
`control.role` in the config:

* **controller** — owns the physical keyboard and mouse (Windows PC)
* **target** — receives synthetic input (Mac)

The *network* role is separate: `network.role = "client"` dials out,
`"server"` listens. In practice the controller is the client, because Windows
cannot accept inbound connections without an administrator firewall rule.

## Threads

```
main thread            macOS: NSStatusItem + CFRunLoopRunInMode (0.2 s slices)
                       Windows: idle loop until shutdown
  │
  ├── control loop     one thread, owns all switching state (`app::Agent`)
  │     ▲
  │     ├── capture bridge thread   platform capture -> Event::Capture
  │     ├── net bridge thread       net events      -> Event::Net
  │     ├── clipboard poller        clipboard text  -> Event::Clipboard
  │     └── control socket bridge   CLI commands    -> Event::Command
  │
  ├── net dial/accept thread        connection lifecycle
  │     ├── net reader thread       encrypted frames -> Event::Net(::Frame)
  │     └── net writer thread       Event -> frames on the wire
  │
  └── input hook thread             WH_*_LL hooks / CGEventTap + run loop
```

All shared decisions (where input goes, whether to suppress, what the clipboard
last saw) live in the control loop thread. Platform backends only expose
atomics (`set_suppress`) and injection calls, so hooks never take a lock that
the control loop holds for long.

The clipboard poller is deliberately separate: reading the Windows clipboard can
block for a few milliseconds when another application holds it, and that must
never stall input forwarding.

## Input capture

### Windows

Low-level hooks, `WH_KEYBOARD_LL` and `WH_MOUSE_LL`, installed on a dedicated
thread with a message pump. These are the only mechanism that sees input
system-wide *and* can swallow it without administrator rights.

* Returning `1` from the hook removes the event before any application sees it.
* Events we inject carry the `LLKHF_INJECTED` / `LLMHF_INJECTED` flags and the
  `dwExtraInfo` marker, so they are never fed back to the peer.
* The keyboard hook maps `(scancode, E0/E1 prefix, virtual-key)` to the canonical
  key. Ambiguous scan codes (`0x37`, `0x45`, `0x52`, `0x47`–`0x51`, `0x53`) are
  disambiguated by the virtual-key code, so Insert/Numpad0, Home/Numpad7,
  PrintScreen/Numpad* and Pause/NumLock all work.
* In remote mode, mouse **buttons and wheel** are swallowed. Mouse **movement**
  is allowed through and then the cursor is warped to the middle of the screen.
  That keeps deltas flowing even when the pointer would otherwise stick to a
  screen edge. The warp generates a synthetic move which is ignored via a
  position guard.
* DPI awareness is set to per-monitor so coordinates are real pixels.

### macOS

A `CGEventTap` at the HID layer, running on its own thread with a run loop. The
callback returns `None` to swallow, `Some(event.clone())` to pass through.

* Accessibility permission is required for both capture and injection.
* Injected events carry `kCGEventSourceUserData = "MCLT"` and are skipped.
* Physical input is swallowed while the peer drives this machine, including
  button events and drags, so the two machines' input never mixes.
* Modifier state is read from the event flags, which is more reliable than
  tracking key codes. `FlagsChanged` is passed through when local and swallowed
  while the peer is in control, so physical modifiers cannot combine with
  injected keys.
* Scroll deltas come from `kCGScrollWheelEventPointDeltaAxis1/2`.

## Injection

* Windows: `SendInput` with scan codes (`KEYEVENTF_SCANCODE`, plus
  `KEYEVENTF_EXTENDEDKEY` for E0 keys). Movement uses `SetCursorPos` so it is
  not mangled by pointer acceleration. Wheel notches are multiplied by 120.
* macOS: `CGEventCreateKeyboardEvent` / `CGEventCreateMouseEvent` /
  `CGEventCreateScrollWheelEvent`, posted at the HID tap location, with flags
  built from the modifier byte. Pointer placement uses
  `CGWarpMouseCursorPosition`. Keys the Mac lacks (PrintScreen, ScrollLock,
  Pause) are mapped to F13/F14/F15; IntlBackslash maps to the ISO section key.

## Switching model

The controller is authoritative and announces every change with
`ControlMsg::State { target }`. The target can ask for changes:

```
controller                                  target
    │  hotkey / edge push                      │
    │────────── State { target: Remote } ─────▶│  suppress local input
    │────────── PointerEdge { opposite } ─────▶│  park pointer on that edge
    │◀───────── Input { … } ──────────────────▶│  inject
    │                                          │
    │◀───────── Edge { edge } ─────────────────│  pointer hit target edge
    │  switch local, park pointer opposite     │
    │◀───────── Request { target } ────────────│  target hotkey
```

Edge handoff parks the pointer on the **opposite** edge of the destination:
exiting the PC through its right edge puts the Mac's pointer on its left edge,
so movement continues in the same direction. Coming back reverses it.

The controller hides its own cursor in remote mode; the target never hides
theirs, because that is the screen the user is looking at.

## Wire protocol

Length-prefixed frames: `u32 little-endian length | u8 kind | payload`, where
the length includes the kind byte. Control messages are JSON; input events are a
compact binary encoding (kind byte identifies the variant).

```
kind 0xF0  handshake hello (plaintext, HMAC-tagged)
kind 0xF1  handshake ack   (plaintext, HMAC-tagged)
kind 0x01  control message (JSON)
kind 0x10  key
kind 0x11  mouse move
kind 0x12  mouse button
kind 0x13  wheel
kind 0x14  pointer edge
```

Control messages: `State`, `Request`, `Edge`, `Clipboard`, `Ping`, `Pong`, `Bye`.

## Handshake and crypto

1. Client sends `Hello { v, name, platform, layout, nonce_c }` plus
   `HMAC-SHA256(psk, "mc-hello-v1" || nonce_c || body)`.
2. Server verifies, replies `HelloOk { …, nonce_s }` plus
   `HMAC-SHA256(psk, "mc-ack-v1" || nonce_c || nonce_s || body)`.
3. Both sides derive 32-byte directional keys with HKDF-SHA256, using the PSK
   as input keying material and `nonce_c || nonce_s` as the salt. Distinct info
   strings (`multcontrol-c2s`, `multcontrol-s2c`) keep the two directions
   separate.
4. Each frame is sealed with ChaCha20-Poly1305. The nonce is
   `counter_be64 || fixed_suffix` and is prepended to the ciphertext. The frame
   kind is the associated data, so a sealed payload cannot be re-labelled.

A wrong key fails at step 1 or 2, before any input is accepted. There is no
ratchet or rekeying in v1; the connection is long-lived and LAN-local.

## Discovery

The server answers UDP probes on port 47777: `MC_DISCOVER 1` in,
`MC_HERE <tcp-port>` out. The client broadcasts to `255.255.255.255`, waits up
to 700 ms, and treats the reply's source address as the peer. A configured
`peer_addr` takes priority; if it stops answering, the client falls back to
discovery (DHCP moved the Mac).

## Failure handling

* The connection dropping returns the controller to local input and unsuppresses
  the target immediately.
* The controller sends a `Ping` every 10 s; the target answers `Pong`. A
  watchdog on both sides closes any session that has been silent for 45 s, so a
  Wi-Fi drop or a sleeping peer cannot leave a half-open connection (and the
  Mac keeps accepting the PC's next attempt).
* The PC retries with exponential backoff (0.5 s → 5 s) forever.
* `multcontrol quit` sends `Bye`, unsuppresses, and exits cleanly; launchd does
  not restart a clean exit because `KeepAlive.SuccessfulExit` is false.
* Hook installation failure is logged and the agent keeps running, so the
  config/CLI/status path still works for diagnosis.
