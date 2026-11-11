# Install the multcontrol agent on Windows, per user, without administrator rights.
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\install-windows.ps1
#
# Everything stays inside the user's profile:
#   * the binary goes to %LOCALAPPDATA%\Programs\MultControl
#   * the config goes to %LOCALAPPDATA%\MultControl
#   * autostart is a HKCU\...\Run entry (no service, no scheduled task)
#
# No firewall rules are created or changed. The agent only makes outbound TCP
# connections, which Windows allows by default.

[CmdletBinding()]
param(
    # Path to multcontrol.exe. Defaults to the release build in this repo.
    [string]$Source = "",
    # Do not start the agent after installing.
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "This script is for Windows. On macOS use scripts/install-macos.sh."
}

$installDir = Join-Path $env:LOCALAPPDATA "Programs\MultControl"
$configDir = Join-Path $env:LOCALAPPDATA "MultControl"
$exe = Join-Path $installDir "multcontrol.exe"
$config = Join-Path $configDir "config.toml"
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

if ([string]::IsNullOrWhiteSpace($Source)) {
    # Repo layout first, then the layout people get from the release page
    # (install-windows.ps1 and multcontrol.exe side by side).
    $candidates = @(
        (Join-Path $PSScriptRoot "..\target\release\multcontrol.exe"),
        (Join-Path $PSScriptRoot "multcontrol.exe")
    )
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $Source = $candidate
            break
        }
    }
    if ([string]::IsNullOrWhiteSpace($Source)) {
        $Source = $candidates[0]
    }
}
if (-not (Test-Path $Source)) {
    throw @"
Could not find $Source.

Build it first on this machine (no admin needed):
    rustup target add x86_64-pc-windows-msvc   # already present with the default toolchain
    cargo build --release -p mc-agent

Then run this script again.
"@
}

Write-Host "==> Stopping any running agent"
if (Test-Path $exe) {
    try { & $exe quit 2>$null | Out-Null } catch { }
    Start-Sleep -Milliseconds 400
}

Write-Host "==> Installing to $installDir"
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
New-Item -ItemType Directory -Force -Path $configDir | Out-Null
Copy-Item -Force $Source $exe
Unblock-File $exe -ErrorAction SilentlyContinue

if (-not (Test-Path $config)) {
    Write-Host ""
    Write-Host "Pairing key from the Mac (run 'multcontrol psk' there, or read network.psk"
    Write-Host "in ~/Library/Application Support/MultControl/config.toml)."
    $psk = Read-Host "Pairing key"
    $psk = $psk.Trim()
    if ($psk -notmatch '^[0-9a-fA-F]{64}$') {
        throw "The pairing key must be 64 hex characters (32 bytes)."
    }
    $psk = $psk.ToLowerInvariant()

    Write-Host ""
    $peer = Read-Host "Mac LAN address (leave empty to find it automatically)"
    $peer = $peer.Trim()
    if ($peer -match '^\d{1,3}(\.\d{1,3}){3}$') {
        $peer = "$peer" + ":47778"
    }
    # An empty line is valid TOML; an empty peer_addr would also fall back
    # to discovery, but omitting the key is cleaner.
    $peerLine = if ($peer) { "peer_addr = `"$peer`"" } else { "" }

    $toml = @"
# multcontrol on the Windows PC.
#
# This machine owns the keyboard and mouse and dials out to the Mac, so it
# needs no inbound firewall rule and no administrator rights.

[network]
role = "client"
$peerLine
discovery = true
discovery_port = 47777
port = 47778
psk = "$psk"

[control]
role = "controller"
hotkey = "ctrl+alt+space"
edge_push = true
edge_margin = 2
hotkey_debounce_ms = 400

[input]
hide_cursor_when_remote = true
sync_clipboard = true
auto_paste = false
clipboard_poll_ms = 500
"@
    # ASCII avoids the BOM that Windows PowerShell 5.1 adds to UTF8 files;
    # the TOML parser does not accept a leading byte-order mark.
    Set-Content -Path $config -Value $toml -Encoding ASCII
    Write-Host "Wrote $config"
}

Write-Host "==> Registering per-user autostart"
New-Item -Path $runKey -Force | Out-Null
Set-ItemProperty -Path $runKey -Name "MultControl" -Value "`"$exe`" run"

if (-not $NoStart) {
    Write-Host "==> Starting the agent"
    Start-Process -FilePath $exe -ArgumentList "run" -WindowStyle Hidden
    Start-Sleep -Seconds 1
    try { & $exe status } catch { }
}

Write-Host ""
Write-Host "Done."
Write-Host ""
Write-Host "  config: $config"
Write-Host "  log:    $configDir\agent.log"
Write-Host "  exe:    $exe"
Write-Host ""
Write-Host "Press Ctrl+Alt+Space to move the keyboard and mouse to the Mac, or push"
Write-Host "the pointer off the right edge of the screen. Press it again (or push off"
Write-Host "the Mac's left edge) to come back."
Write-Host ""
Write-Host "Useful commands:"
Write-Host "  & `"$exe`" status"
Write-Host "  & `"$exe`" toggle"
Write-Host "  & `"$exe`" doctor"
Write-Host "  & `"$exe`" quit"
