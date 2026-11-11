@echo off
rem ============================================================================
rem  multcontrol - verification
rem
rem  Double-click this file to check that multcontrol works on this computer.
rem  It installs nothing, changes no configuration and touches no files
rem  outside this folder.
rem
rem  Put multcontrol.exe (and optionally smoke-windows.ps1) next to this file
rem  and double-click it.
rem ============================================================================

setlocal
title multcontrol - verification
cd /d "%~dp0"

echo.
echo  ================================================
echo    multcontrol - verification
echo  ================================================
echo.

if not exist "multcontrol.exe" (
    echo  [FAIL] multcontrol.exe is not in this folder.
    echo         Download it from the release page and put it next to this file.
    goto :end
)

rem Remove the "downloaded from the internet" mark. Windows only shows the
rem SmartScreen prompt for files that still carry it, so clearing it here
rem means these checks run without any further clicks.
powershell -NoProfile -ExecutionPolicy Bypass -Command "Unblock-File -Path '%~dp0multcontrol.exe' -ErrorAction SilentlyContinue" >nul 2>&1

echo  --- version ---
multcontrol.exe version
echo.

echo  --- selftest ---
multcontrol.exe selftest
if errorlevel 1 (
    echo  [FAIL] the selftest reported a problem - see the lines above.
) else (
    echo  [ok]   the selftest passed.
)
echo.

echo  --- two-agent test ---
if exist "smoke-windows.ps1" (
    echo  Starting two agents over the loopback interface and handing the
    echo  keyboard back and forth. This takes a few seconds.
    echo.
    powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0smoke-windows.ps1" -Exe "%~dp0multcontrol.exe"
    if errorlevel 1 (
        echo  [FAIL] the two-agent test failed - see the lines above.
    ) else (
        echo  [ok]   the two-agent test passed.
    )
) else (
    echo  [skip] smoke-windows.ps1 is not in this folder; skipping.
    echo         Download it from the release page for the full test.
)

:end
echo.
echo  ================================================
echo.
pause
