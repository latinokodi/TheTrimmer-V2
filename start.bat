@echo off
REM ---------------------------------------------------------------------------------------------
REM  TheTrimmer - double-click this to start the window.
REM
REM  This file deliberately does almost nothing. Everything that has to work on a machine nobody
REM  has seen -- finding or installing Python, Node and ffmpeg, building the interface, fetching
REM  Electron -- lives in scripts\bootstrap.ps1, because a batch file cannot download over TLS,
REM  cannot unpack an archive, and cannot reliably compare version numbers. Trying to do those
REM  here is how a start script ends up working on the machine it was written on and nowhere else.
REM
REM  What is left here is the part batch is genuinely good at: being double-clickable, and
REM  starting PowerShell in a way that cannot be blocked by an execution policy.
REM
REM  The promise: a Windows PC with nothing installed on it, and a person who double-clicks this,
REM  ends up looking at the window. It needs an internet connection the first time, and no
REM  administrator at any time.
REM
REM  Note the doubled percent signs: a single one is a parameter substitution even inside a REM
REM  line, so a comment naming the syntax is itself parsed as the syntax. That is how this file
REM  first failed, with "the usage of the path operator in batch-parameter substitution is invalid".
REM ---------------------------------------------------------------------------------------------

setlocal
cd /d "%~dp0"

REM  PowerShell 5.1 is part of every supported Windows, and it lives here. It is named by full path
REM  rather than by name so that a machine whose PATH has been emptied still runs this.
set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"

if not exist "%POWERSHELL%" (
    echo.
    echo   Windows PowerShell was not found at
    echo     %POWERSHELL%
    echo.
    echo   It is part of Windows, so this usually means the system files have been altered.
    echo   Installing it again restores it. Nothing else about this application can start
    echo   without it.
    echo.
    pause
    exit /b 1
)

REM  -ExecutionPolicy Bypass so a restricted policy cannot stop the bootstrap, -NoProfile so a
REM  broken profile cannot either, and -NoLogo to keep the console readable.
"%POWERSHELL%" -NoLogo -NoProfile -ExecutionPolicy Bypass -File "scripts\bootstrap.ps1" %*
set "CODE=%ERRORLEVEL%"

REM  Only reached when the window has closed or the bootstrap failed. Either way the console is
REM  about to disappear, so it waits when something went wrong.
if not "%CODE%"=="0" (
    echo.
    echo   TheTrimmer did not start. The reason is above.
    echo.
    pause
)

endlocal
exit /b %CODE%
