# Start TheTrimmer.
#
# ## What this does, and what it refuses to do
#
# It makes sure the interface and the window are current, then launches the window. It is 40 lines of
# PowerShell rather than 90 lines of batch because every real decision here is a comparison of
# timestamps, and batch has no dates: it has strings that sort like dates and a `%~t` syntax that
# silently produces the wrong thing in the wrong locale.
#
# ## Why it is not just `cargo run`
#
# Three separate faults showed up the first time this was used properly, and each one is handled here:
#
# 1. **A stale interface.** The window embeds `apps/web/dist` at compile time, so editing a component
#    and building only the Rust produces a window with the old screen in it — with no error, because
#    from Rust's point of view nothing changed. The interface is rebuilt whenever a source file is
#    newer than the bundle.
#
# 2. **A needless rebuild.** `npm run build` rewrites `dist` with new content hashes, which makes the
#    window's build script see a changed asset and relink the whole binary. Doing that on every launch
#    turned a one-second start into a two-minute one for no reason. So the interface is only rebuilt
#    when it is actually out of date.
#
# 3. **A running window.** Windows will not let the linker replace an executable that is open, and a
#    window that is already up *is* that executable. Cargo reports it as `failed to remove file …:
#    Access is denied`, which reads like a permissions problem and is not. The running window is closed
#    first, and it is closed deliberately rather than by a build that then fails.
#
#   pwsh -File tools\start.ps1            build if needed, then launch
#   pwsh -File tools\start.ps1 -Rebuild   build the interface and the window, then launch
#   pwsh -File tools\start.ps1 -Check     say what is out of date and exit
#   pwsh -File tools\start.ps1 -NoLaunch  build if needed, do not launch
#
# `start.bat` in the repository root is the double-click entry point; it calls this.

[CmdletBinding()]
param(
    [switch] $Rebuild,
    [switch] $NoLaunch,
    [switch] $Check
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\thetrimmer-desktop.exe'
$dist = Join-Path $root 'apps\web\dist\index.html'
$web = Join-Path $root 'apps\web'

function Write-Step {
    param([string] $Text)
    Write-Host ''
    Write-Host "  $Text" -ForegroundColor Cyan
}

function Write-Detail {
    param([string] $Text)
    Write-Host "    $Text" -ForegroundColor DarkGray
}

function Newest-File {
    param([string] $Folder, [string[]] $Include)

    if (-not (Test-Path $Folder)) { return $null }
    $newest = Get-ChildItem -Path $Folder -Recurse -File -Include $Include -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notlike '*\node_modules\*' -and $_.FullName -notlike '*\dist\*' } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    return $newest
}

# ---------------------------------------------------------------------------------------------
# Is anything out of date?
# ---------------------------------------------------------------------------------------------

$webSources = Newest-File -Folder (Join-Path $web 'src') -Include @('*.ts', '*.tsx', '*.css')
$webConfig = Newest-File -Folder $web -Include @('package.json', 'vite.config.ts', 'index.html')
$newestWeb = @($webSources, $webConfig) | Where-Object { $null -ne $_ } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

$needsInterface = $true
$whyInterface = 'no interface has been built yet'
if (Test-Path $dist) {
    $built = (Get-Item $dist).LastWriteTime
    if ($null -eq $newestWeb) {
        $needsInterface = $false
        $whyInterface = 'nothing to compare against'
    } elseif ($newestWeb.LastWriteTime -le $built) {
        $needsInterface = $false
        $whyInterface = "up to date ($($newestWeb.Name), $($newestWeb.LastWriteTime.ToString('HH:mm:ss')))"
    } else {
        $whyInterface = "$($newestWeb.Name) changed at $($newestWeb.LastWriteTime.ToString('HH:mm:ss')), after the build at $($built.ToString('HH:mm:ss'))"
    }
}

# The window is out of date if it is missing, or if anything it is built *from* is newer than it:
# the Rust sources, the configuration, or the interface sources.
#
# `apps/web/dist` is deliberately not one of those inputs. The assets are embedded at compile time, but
# a build rewrites that folder with new content hashes and therefore a fresh timestamp — so comparing
# the window against it makes the window stale every time the interface is rebuilt, and the interface is
# rebuilt *by* this script. The first version of this check did exactly that and relinked the whole
# binary on every launch, which is the two-minute start it was written to prevent. A web source file
# newer than the bundle is what means the assets changed; the bundle's own timestamp means nothing.
$newestRust = Newest-File -Folder (Join-Path $root 'crates') -Include @('*.rs')
$newestDesktop = Newest-File -Folder (Join-Path $root 'apps\desktop\src-tauri') -Include @('*.rs', '*.json', '*.toml')
$newestInput = @($newestRust, $newestDesktop, $newestWeb) | Where-Object { $null -ne $_ } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

$needsWindow = $true
$whyWindow = 'the window has not been built yet'
if (Test-Path $exe) {
    $built = (Get-Item $exe).LastWriteTime
    if ($null -eq $newestInput -or $newestInput.LastWriteTime -le $built) {
        $needsWindow = $false
        $whyWindow = "up to date (built $($built.ToString('HH:mm:ss')))"
    } else {
        $whyWindow = "$($newestInput.Name) is newer than the window built at $($built.ToString('HH:mm:ss'))"
    }
}

if ($Rebuild) {
    $needsInterface = $true
    $needsWindow = $true
    $whyInterface = 'asked to rebuild'
    $whyWindow = 'asked to rebuild'
}

Write-Host 'TheTrimmer' -ForegroundColor White
Write-Detail "interface: $whyInterface"
Write-Detail "window:    $whyWindow"

if ($Check) {
    Write-Host ''
    Write-Host $(if ($needsInterface -or $needsWindow) { 'Out of date.' } else { 'Everything is current.' })
    exit 0
}

# ---------------------------------------------------------------------------------------------
# Build what is out of date
# ---------------------------------------------------------------------------------------------

if ($needsInterface) {
    Write-Step 'Building the interface'
    Push-Location $web
    try {
        if (-not (Test-Path (Join-Path $web 'node_modules'))) {
            Write-Detail 'installing dependencies (first run only)'
            & npm install --no-audit --no-fund
            if ($LASTEXITCODE -ne 0) { throw 'npm install failed. Is Node.js installed? Try: node --version' }
        }
        & npm run build
        if ($LASTEXITCODE -ne 0) { throw 'the interface build failed' }
    } finally {
        Pop-Location
    }
    if (-not (Test-Path $dist)) {
        throw "the interface build reported success but wrote no $dist"
    }
}

if ($needsWindow) {
    # A window that is already open *is* the executable the linker is about to replace, and Windows
    # refuses. Cargo says `failed to remove file …: Access is denied`, which reads as a permissions
    # problem.
    $running = Get-Process -Name 'thetrimmer-desktop' -ErrorAction SilentlyContinue
    if ($null -ne $running) {
        Write-Step 'Closing the window that is already open'
        Write-Detail "pid $($running.Id -join ', ') — the linker cannot replace a running executable"
        $running | Stop-Process -Force
        $deadline = (Get-Date).AddSeconds(10)
        while ((Get-Date) -lt $deadline -and
               $null -ne (Get-Process -Name 'thetrimmer-desktop' -ErrorAction SilentlyContinue)) {
            Start-Sleep -Milliseconds 200
        }
        if ($null -ne (Get-Process -Name 'thetrimmer-desktop' -ErrorAction SilentlyContinue)) {
            throw 'the window would not close. Close it by hand and run this again.'
        }
    }

    Write-Step 'Building the window'
    Write-Detail 'the first build on a new machine compiles the toolchain and takes a few minutes;'
    Write-Detail 'after that it is seconds, and this step is skipped entirely when nothing changed.'
    Push-Location $root
    try {
        # All four binaries, not only the window: the command line and the daemon are the same product
        # and a release that builds one of them is a release with a stale tool in the folder.
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'the build failed — the error is in the output above' }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $exe)) {
    throw "there is no window at $exe"
}

# Warn rather than fail: the window opens without ffmpeg and says so, and a person may be launching it
# to read that message.
$cli = Join-Path $root 'target\release\thetrimmer.exe'
if (Test-Path $cli) {
    & $cli doctor *> $null
    if ($LASTEXITCODE -ne 0) {
        Write-Host ''
        Write-Host '  NOTE: this machine cannot cut yet — ffmpeg or its H.264 encoder is missing.' -ForegroundColor Yellow
        Write-Host '        Run  target\release\thetrimmer.exe doctor  to see what was found,'
        Write-Host '        or fix it with  winget install Gyan.FFmpeg'
    }
}

if ($NoLaunch) {
    Write-Host ''
    Write-Host '  Built. Not launching, as asked.' -ForegroundColor Green
    exit 0
}

Write-Step 'Starting TheTrimmer'
Write-Detail $exe

# Run in the foreground rather than through `Start-Process`: this process stays alive for as long as the
# window does, so a startup failure prints its reason here instead of vanishing into a detached child.
& $exe
exit $LASTEXITCODE
