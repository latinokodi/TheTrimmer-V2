@echo off
REM ---------------------------------------------------------------------------------------------
REM  TheTrimmer - double-click this to start the window.
REM
REM  It builds what is out of date and then opens the window. The work is in tools\start.ps1: every
REM  real decision here is a comparison of timestamps, and batch has no dates - it has strings that
REM  sort like dates and a `%~t` syntax that produces the wrong thing in the wrong locale.
REM
REM  This file exists because a `.bat` is what a person can double-click. It is a launcher and
REM  nothing else, so there is one implementation of "build what changed and start it".
REM ---------------------------------------------------------------------------------------------

setlocal
cd /d "%~dp0"

where pwsh >nul 2>&1
if errorlevel 1 goto :nopwsh

pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\start.ps1" %*
exit /b %ERRORLEVEL%

:nopwsh
echo.
echo ---------------------------------------------------------------------
echo  PowerShell 7 ^(pwsh^) was not found on PATH.
echo.
echo  TheTrimmer's launcher is a PowerShell script. Install PowerShell 7:
echo      winget install Microsoft.PowerShell
echo.
echo  Or build and start the window by hand:
echo      cargo build --release
echo      target\release\thetrimmer-desktop.exe
echo ---------------------------------------------------------------------
echo.
pause
exit /b 1
