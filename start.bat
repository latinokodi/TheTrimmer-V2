@echo off
REM ---------------------------------------------------------------------------------------------
REM  TheTrimmer - double-click this to start the window.
REM
REM  It creates the Python environment if it is missing, installs what the backend needs, builds the
REM  interface, and starts Electron. Every step is skipped when it is already done, so the second
REM  launch is a couple of seconds.
REM
REM  Nothing here needs a compiler. The engine is Python, the window is Electron, and the interface
REM  is a web page -- which is the whole reason for that choice.
REM
REM  Note the doubled percent signs: a single one is a parameter substitution even inside a REM
REM  line, so a comment naming the syntax is itself parsed as the syntax. That is how this file
REM  first failed, with "the usage of the path operator in batch-parameter substitution is invalid".
REM ---------------------------------------------------------------------------------------------

setlocal EnableDelayedExpansion
cd /d "%~dp0"

echo.
echo   TheTrimmer - frame-exact, verified segment cutting
echo   ------------------------------------------------------------------
echo.

REM --- Python -----------------------------------------------------------------------------------
if not exist "venv\Scripts\python.exe" (
    echo   Creating the Python environment...
    python -m venv venv
    if errorlevel 1 (
        echo.
        echo   Python 3.10 or newer is required. Install it from https://python.org
        pause
        exit /b 1
    )
    echo   Installing the engine's dependencies...
    venv\Scripts\python.exe -m pip install --quiet --upgrade pip --disable-pip-version-check
    venv\Scripts\python.exe -m pip install --quiet --disable-pip-version-check -r backend\requirements.txt
    if errorlevel 1 (
        echo.
        echo   Could not install the backend's dependencies.
        pause
        exit /b 1
    )
)
echo   Python environment ready.

REM --- Node -------------------------------------------------------------------------------------
where node >nul 2>nul
if errorlevel 1 (
    echo.
    echo   Node.js 18 or newer is required. Install it from https://nodejs.org
    pause
    exit /b 1
)

if not exist "node_modules" (
    echo   Installing the window's dependencies ^(first run only, this takes a minute^)...
    call npm install
    if errorlevel 1 (
        echo.
        echo   Could not install the window's dependencies.
        pause
        exit /b 1
    )
)

REM --- Electron itself --------------------------------------------------------------------------
REM  `npm install` fetches the Electron package but its postinstall step is what downloads the
REM  binary, and any `ignore-scripts=true` in the user's npm config skips it silently. The result
REM  is "Electron failed to install correctly, please delete node_modules/electron" on the first
REM  launch, so the download is checked for and run explicitly rather than trusted to a hook.
if not exist "node_modules\electron\dist\electron.exe" (
    echo   Fetching the Electron runtime ^(first run only^)...
    call node "node_modules\electron\install.js"
    if not exist "node_modules\electron\dist\electron.exe" (
        echo.
        echo   Electron did not download. Check the network, then delete node_modules\electron
        echo   and run this file again.
        pause
        exit /b 1
    )
)

if not exist "frontend\node_modules" (
    echo   Installing the interface's dependencies ^(first run only^)...
    call npm --prefix frontend install
    if errorlevel 1 (
        echo.
        echo   Could not install the interface's dependencies.
        pause
        exit /b 1
    )
)

REM --- The interface ----------------------------------------------------------------------------
REM  Rebuilt when a source file is newer than the bundle, so an edit is picked up without anyone
REM  having to remember this step. The comparison is against the bundle itself and not against
REM  this file: comparing to a fixed timestamp rebuilds for the wrong reason and misses the right
REM  one the moment the bundle happens to be newer than it.
set NEEDS_BUILD=0
if not exist "frontend\dist\index.html" (
    set NEEDS_BUILD=1
) else (
    for %%f in ("frontend\dist\index.html") do set BUNDLE_TIME=%%~tf
    for /f %%f in ('dir /b /s "frontend\src" 2^>nul ^| findstr /i "\.tsx$ \.ts$ \.css$"') do (
        if %%~tf GTR !BUNDLE_TIME! set NEEDS_BUILD=1
    )
)
if "%NEEDS_BUILD%"=="1" (
    echo   Building the interface...
    call npm run build
    if errorlevel 1 (
        echo.
        echo   The interface did not build.
        pause
        exit /b 1
    )
)
echo   Interface ready.

REM --- The engine's own tests --------------------------------------------------------------------
REM  Fast, and they never launch ffmpeg's encoder: timecode arithmetic, the plan's decisions, and
REM  the HTTP contract. Skipped when pytest is not installed, because the application does not need
REM  it to run.
if exist "venv\Scripts\python.exe" (
    venv\Scripts\python.exe -c "import pytest" >nul 2>nul
    if not errorlevel 1 (
        echo   Checking the engine...
        venv\Scripts\python.exe -m pytest backend\tests -q
        if errorlevel 1 (
            echo.
            echo   The engine's tests failed. The window can still be started; the failure is above.
            pause
        )
    )
)

REM --- Start ------------------------------------------------------------------------------------
echo.
echo   Starting TheTrimmer...
echo.
call npm start
endlocal
