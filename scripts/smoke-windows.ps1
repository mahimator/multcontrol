# Smoke test for the Windows build.
#
# Runs two copies of multcontrol.exe over loopback: one as the Mac-side target,
# one as the PC-side controller. Checks the handshake, status reporting and a
# full toggle round-trip through the real control socket.
#
# It installs nothing, writes no registry keys, and uses a temporary folder.
# Run it on a machine where you are about to install, before installing:
#
#   powershell -ExecutionPolicy Bypass -File .\scripts\smoke-windows.ps1 `
#       -Exe .\multcontrol.exe
#
# Exit code 0 means the binary works on this machine.

[CmdletBinding()]
param(
    [string]$Exe = ""
)

$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "This script is for Windows."
}

if ([string]::IsNullOrWhiteSpace($Exe)) {
    $Exe = Join-Path $PSScriptRoot "..\target\release\multcontrol.exe"
}
if (-not (Test-Path $Exe)) {
    throw "multcontrol.exe not found at $Exe. Build it or pass -Exe."
}
$Exe = (Resolve-Path $Exe).Path

$root = Join-Path $env:TEMP ("multcontrol-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 10))
New-Item -ItemType Directory -Force -Path $root | Out-Null

$psk = ([guid]::NewGuid().ToString("N") + [guid]::NewGuid().ToString("N"))
$tcpPort = 47810
$serverControl = 47811
$clientControl = 47812

$serverConfig = Join-Path $root "server.toml"
$clientConfig = Join-Path $root "client.toml"

@"
[network]
role = "server"
listen_addr = "127.0.0.1:$tcpPort"
discovery = false
discovery_port = 47813
port = $tcpPort
psk = "$psk"

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
"@ | Set-Content -Path $serverConfig -Encoding ASCII

@"
[network]
role = "client"
peer_addr = "127.0.0.1:$tcpPort"
discovery = false
discovery_port = 47813
port = $tcpPort
psk = "$psk"

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
"@ | Set-Content -Path $clientConfig -Encoding ASCII

$failures = 0

function Step($text) {
    Write-Host "==> $text"
}

function Check($ok, $what) {
    if ($ok) {
        Write-Host "    [ok]   $what"
    } else {
        Write-Host "    [FAIL] $what"
        $script:failures++
    }
}

function Start-Agent($config, $controlPort) {
    $env:MULTControl_CONFIG = $config
    $env:MULTControl_CONTROL_PORT = "$controlPort"
    Start-Process -FilePath $Exe -ArgumentList "run" -PassThru -WindowStyle Hidden
}

function Invoke-Ctl($config, $controlPort, $command) {
    $env:MULTControl_CONFIG = $config
    $env:MULTControl_CONTROL_PORT = "$controlPort"
    $output = & $Exe $command 2>&1 | Out-String
    return $output.Trim()
}

function Wait-For($config, $controlPort, $needle, $timeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($timeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $status = Invoke-Ctl $config $controlPort "status"
        if ($status -match $needle) {
            return $status
        }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

function Stop-Agent($config, $controlPort, $process) {
    try { Invoke-Ctl $config $controlPort "quit" | Out-Null } catch { }
    Start-Sleep -Milliseconds 500
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}

$server = $null
$client = $null

try {
    Step "selftest"
    $self = & $Exe "selftest" 2>&1 | Out-String
    Write-Host $self.TrimEnd()
    Check ($LASTEXITCODE -eq 0) "selftest exit code"

    Step "starting the target (server)"
    $server = Start-Agent $serverConfig $serverControl
    Start-Sleep -Seconds 1

    Step "starting the controller (client)"
    $client = Start-Agent $clientConfig $clientControl

    Step "handshake"
    $status = Wait-For $clientConfig $clientControl "connected to" 15
    Check ($null -ne $status) "controller sees the peer"
    if ($status) { Write-Host "         $status" }

    $status = Wait-For $serverConfig $serverControl "connected to" 10
    Check ($null -ne $status) "target sees the controller"
    if ($status) { Write-Host "         $status" }

    Step "toggle to the target"
    $reply = Invoke-Ctl $clientConfig $clientControl "toggle"
    Check ($reply -eq "ok") "toggle acknowledged"
    $status = Wait-For $clientConfig $clientControl "input -> the Mac" 5
    Check ($null -ne $status) "controller reports remote"
    if ($status) { Write-Host "         $status" }
    $status = Wait-For $serverConfig $serverControl "controlled by the PC" 5
    Check ($null -ne $status) "target reports controlled"
    if ($status) { Write-Host "         $status" }

    Step "toggle back"
    $reply = Invoke-Ctl $clientConfig $clientControl "toggle"
    Check ($reply -eq "ok") "toggle acknowledged"
    $status = Wait-For $clientConfig $clientControl "input -> this PC" 5
    Check ($null -ne $status) "controller reports local"
    if ($status) { Write-Host "         $status" }
    $status = Wait-For $serverConfig $serverControl "input -> idle" 5
    Check ($null -ne $status) "target reports idle"
    if ($status) { Write-Host "         $status" }
}
finally {
    Step "stopping"
    Stop-Agent $clientConfig $clientControl $client
    Stop-Agent $serverConfig $serverControl $server
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}

Write-Host ""
if ($failures -eq 0) {
    Write-Host "result: PASS"
    exit 0
} else {
    Write-Host "result: FAIL ($failures check(s) failed)"
    exit 1
}
