@echo off
REM ---------------------------------------------------------------------------------------------
REM  TheTrimmer — start the window.
REM
REM  Double-click this file. It opens the application, building it first if anything changed.
REM
REM  The window is `thetrimmer-desktop.exe`, NOT `thetrimmer.exe`. The latter is the command line
REM  tool: running it prints text instead of opening a window, which is a confusing half hour if
REM  you pick the wrong one.
REM
REM  `cargo build` is the change detector rather than a timestamp comparison here, because cargo
REM  already does that job correctly and a hand-rolled one in batch is how you get a stale window.
REM ---------------------------------------------------------------------------------------------

setlocal
cd /d "%~dp0"

set "LIB=target\release\thetrimmer-desktop.exe"
set "WEB=apps\web\dist\index.html"

REM The interface is compiled into the window at build time, so it has to exist before the Rust
REM build runs. A missing dist is what produces a window that says it cannot reach the page.
if not exist "%WEB%" (
  echo.
  echo Building the interface ^(first run only, this takes a moment^)...
  echo.
  pushd apps\web
  if not exist node_modules (
    call npm install --no-audit --no-fund
    if errorlevel 1 goto :npmerror
  )
  call npm run build
  if errorlevel 1 goto :failed
  popd
)

echo.
echo Building TheTrimmer...
echo.
cargo build --release -p thetrimmer-desktop
if errorlevel 1 goto :failed

if not exist "%LIB%" goto :failed

REM Give ffmpeg a moment's warning rather than letting the window fail on the first cut.
if exist "target\release\thetrimmer.exe" (
  target\release\thetrimmer.exe doctor >nul 2>&1
  if errorlevel 1 (
    echo.
    echo NOTE: this machine cannot cut yet - ffmpeg or its H.264 encoder is missing.
    echo       Run  target\release\thetrimmer.exe doctor  to see what was found.
    echo       Fix it with  winget install Gyan.FFmpeg
    echo.
  )
)

echo Starting TheTrimmer...
echo.

REM Launched directly rather than through `start`, because `start` hands the child to a console
REM that is about to exit: the window is created and then taken down with the parent. Running the
REM executable in the foreground means this console stays open for as long as the window does, which
REM is also where a startup failure prints its reason instead of vanishing.
"%LIB%"

REM Reached when the user closes the window.
if errorlevel 1 (
  echo.
  echo TheTrimmer exited with code %errorlevel%.
  pause
)
exit /b 0

:npmerror
popd
echo.
echo ---------------------------------------------------------------------
echo  npm install failed. The window cannot be built without it.
echo  Check that Node.js is installed:  node --version
echo ---------------------------------------------------------------------
echo.
pause
exit /b 1

:failed
echo.
echo ---------------------------------------------------------------------
echo  The build failed, so the window cannot start.
echo.
echo  If the message above mentions ffmpeg, install it:
echo      winget install Gyan.FFmpeg
echo.
echo  Otherwise the error is in the build output above this line.
echo ---------------------------------------------------------------------
echo.
pause
exit /b 1
