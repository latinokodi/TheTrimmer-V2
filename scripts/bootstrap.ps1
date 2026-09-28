<#
    TheTrimmer - everything the application needs, gathered by itself.

    The promise this file keeps is narrow and absolute: a Windows PC with nothing installed on it,
    and a person who double-clicks `start.bat`, ends up looking at the window. Not "install Python
    first", not "install Node first", not "ffmpeg was not found on PATH" -- those are the
    application failing to do its own job, and they were what the first version of it did.

    So this file finds or installs four things:

      Python 3.10+    the engine
      Node 18+        the window's runtime and the interface's build
      ffmpeg/ffprobe  the encodes and the probes; the engine shells out to both
      the packages    aiohttp and PyAV into the project's venv, npm's trees, Electron's binary

    It writes nothing outside this folder and %LOCALAPPDATA%, so it needs no administrator, and it
    never assumes a PATH entry it cannot see: a running process does not observe a PATH change made
    while it runs, so anything installed here is located by path afterwards rather than by name.

    Everything downloaded is cached under `.tools\downloads`, so a second run is seconds rather
    than a hundred megabytes, and every step is skipped when it is already done.

    Written for Windows PowerShell 5.1 -- the one every supported Windows already has. No
    PowerShell 7 syntax is used, because requiring it would be the same failure in a new costume.
#>

[CmdletBinding()]
param(
    # Prove the whole thing without opening the window. Used by the checks that run this file.
    [switch] $NoLaunch,
    # Provision the project's own artefacts again -- the venv, the npm trees, the bundle -- while
    # still using whatever Python, Node and ffmpeg are already on the machine.
    [switch] $Force,
    # Put the downloaded tools somewhere else. This is how the download-and-unpack path is tested
    # without disturbing a working `.tools` folder.
    [string] $ToolsRoot
)

$ErrorActionPreference = 'Stop'
# Without this, downloads fail with a bare "could not create SSL/TLS secure channel": Windows
# PowerShell still negotiates TLS 1.0 by default on some installs, and python.org, nodejs.org and
# gyan.dev all refuse it.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$ToolsDir = if ($ToolsRoot) { $ToolsRoot } else { Join-Path $ProjectRoot '.tools' }
$DownloadsDir = Join-Path $ToolsDir 'downloads'

# The versions the download URLs were checked against. 3.12 is what the engine was developed and
# tested on; the others are there for the day that one is withdrawn. Node is not pinned -- it is
# asked for from nodejs.org's own index, so this file does not grow stale.
$PythonFallbacks = @('3.12.10', '3.13.7', '3.11.9')
$PythonMinimum = [version] '3.10'
$NodeMinimum = [version] '18.0'

$script:Provisioned = New-Object System.Collections.ArrayList

# ------------------------------------------------------------------------------------------------
#  Talking to the person watching the console
# ------------------------------------------------------------------------------------------------

#  Every stage prints one aligned line -- "  python     using Python 3.12.10 at ..." -- and any
#  further lines sit under the detail column. The console is the only diagnostic surface a
#  double-click has, so it is worth the alignment.
$LabelWidth = 10

function Write-Head {
    Write-Host ''
    Write-Host '  TheTrimmer - frame-exact, verified segment cutting' -ForegroundColor White
    Write-Host ('  ' + ('-' * 66)) -ForegroundColor DarkGray
}

function Write-Stage {
    param([string] $Label, [string] $Detail)
    Write-Host ''
    Write-Host ("  {0,-$LabelWidth} " -f $Label) -ForegroundColor Cyan -NoNewline
    Write-Host $Detail -ForegroundColor Gray
}

function Write-Note {
    param([string] $Message)
    Write-Host ((' ' * (2 + $LabelWidth + 1)) + $Message) -ForegroundColor DarkGray
}

function Write-Good {
    param([string] $Message)
    Write-Host "  $Message" -ForegroundColor Green
}

function Write-Problem {
    param([string] $Message)
    Write-Host "  $Message" -ForegroundColor Red
}

function Wait-ForReader {
    Write-Host ''
    Write-Host '  Press any key to close.' -ForegroundColor DarkGray
    try { $null = $Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown') } catch { Start-Sleep -Seconds 20 }
}

# A failure a person can act on. The console is about to close, so the advice matters as much as
# the message, and the wait keeps both on screen.
function Stop-With {
    param([string] $Message, [string] $Advice)
    Write-Host ''
    Write-Problem $Message
    if ($Advice) {
        Write-Host ''
        foreach ($line in ($Advice -split "`r?`n")) { Write-Note $line.TrimEnd() }
    }
    Wait-ForReader
    exit 1
}

function Ensure-Directory {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        $null = New-Item -ItemType Directory -Path $Path -Force
    }
    return $Path
}

# ------------------------------------------------------------------------------------------------
#  Running other programs
# ------------------------------------------------------------------------------------------------

# Runs a program and reports whether it worked and what it said, so a caller does not have to know
# that a native command's failure and a PowerShell exception arrive by different routes.
function Invoke-Native {
    param([string] $File, [string[]] $Arguments, [string] $WorkDir)

    $pushed = $false
    if ($WorkDir) { Push-Location $WorkDir; $pushed = $true }
    try {
        $output = & $File @Arguments 2>&1 | Out-String
        $code = $LASTEXITCODE
        if ($null -eq $code) { $code = 0 }
        return [pscustomobject]@{ Ok = ($code -eq 0); Code = $code; Text = $output.Trim() }
    } catch {
        return [pscustomobject]@{ Ok = $false; Code = -1; Text = $_.Exception.Message }
    } finally {
        if ($pushed) { Pop-Location }
    }
}

function Save-Download {
    param([string] $Url, [string] $Destination, [string] $What)

    Ensure-Directory (Split-Path -Parent $Destination) | Out-Null
    $partial = "$Destination.part"
    if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }

    # Three attempts. The first run of this file on a fresh machine is exactly when a dropped
    # connection costs the most, and exactly when nobody is in a position to retry it. Each
    # attempt starts the file over, so a half-written download is never kept.
    $attempt = 0
    while ($attempt -lt 3) {
        $attempt++
        try {
            $client = New-Object System.Net.WebClient
            $client.Headers.Add('User-Agent', 'TheTrimmer')
            try { $client.DownloadFile($Url, $partial) } finally { $client.Dispose() }

            if (-not (Test-Path -LiteralPath $partial)) { throw 'nothing was written' }
            $size = (Get-Item -LiteralPath $partial).Length
            if ($size -lt 1024) { throw "only $size bytes arrived" }
            Move-Item -LiteralPath $partial -Destination $Destination -Force
            return $true
        } catch {
            if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
            if ($attempt -ge 3) {
                Write-Note "could not download $What"
                Write-Note "  $Url"
                Write-Note "  $($_.Exception.Message)"
                return $false
            }
            Write-Note "$What did not arrive; trying again ($attempt of 3)"
        }
    }
    return $false
}

function Expand-Zip {
    param([string] $Zip, [string] $Destination)

    Ensure-Directory $Destination | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue
    try {
        [System.IO.Compression.ZipFile]::ExtractToDirectory($Zip, $Destination)
        return $true
    } catch {
        # Expand-Archive is slower but is present even where the compression assembly is not.
        try {
            Expand-Archive -LiteralPath $Zip -DestinationPath $Destination -Force
            return $true
        } catch {
            Write-Note "could not unpack $Zip"
            Write-Note "  $($_.Exception.Message)"
            return $false
        }
    }
}

# ------------------------------------------------------------------------------------------------
#  Python
# ------------------------------------------------------------------------------------------------

# A Python is usable only if it is new enough *and* able to build the project's environment: the
# `venv` module and `ensurepip` are both required, and the embeddable distribution ships neither.
function Test-Python {
    param([string] $Exe)

    if (-not $Exe) { return $null }
    # The Microsoft Store publishes a `python.exe` that is a stub opening the Store when run.
    # Testing it would open a shop window at somebody who only wanted to start an application.
    if ($Exe -like '*\WindowsApps\*') { return $null }
    if (-not (Test-Path -LiteralPath $Exe)) { return $null }

    # Single quotes inside the Python, not double: Windows PowerShell strips embedded double quotes
    # when it hands an argument to a native program, so `print("{0}")` reaches python as
    # `print({0})` and dies of a syntax error -- which this file would then read as "no Python
    # here" and answer by downloading another one.
    $version = Invoke-Native -File $Exe -Arguments @(
        '-c', "import sys;print('%d.%d.%d' % sys.version_info[:3])")
    if (-not $version.Ok) { return $null }
    try { $number = [version] $version.Text.Trim() } catch { return $null }
    if ($number -lt $PythonMinimum) { return $null }

    $modules = Invoke-Native -File $Exe -Arguments @('-c', 'import venv, ensurepip')
    if (-not $modules.Ok) { return $null }

    return [pscustomobject]@{ Exe = $Exe; Version = $number }
}

function Find-Python {
    $candidates = New-Object System.Collections.ArrayList

    # One provisioned by an earlier run wins: it is the interpreter the venv belongs to.
    $null = $candidates.Add((Join-Path $ProjectRoot 'venv\Scripts\python.exe'))

    # The launcher, which is the reliable name on a machine carrying several Pythons.
    $launcher = Get-Command 'py.exe' -ErrorAction SilentlyContinue
    if ($launcher) {
        $resolved = Invoke-Native -File $launcher.Source -Arguments @(
            '-3', '-c', 'import sys;print(sys.executable)')
        if ($resolved.Ok -and $resolved.Text) {
            foreach ($line in ($resolved.Text -split "`r?`n")) {
                $trimmed = $line.Trim()
                if ($trimmed -like '*python*.exe') { $null = $candidates.Add($trimmed) }
            }
        }
    }

    foreach ($name in @('python.exe', 'python3.exe')) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command) { $null = $candidates.Add($command.Source) }
    }

    # Straight from the official installer, in the order it lays them down.
    foreach ($pattern in @(
            (Join-Path $env:LOCALAPPDATA 'Programs\Python\Python3*\python.exe'),
            (Join-Path $env:ProgramFiles 'Python3*\python.exe'),
            'C:\Python3*\python.exe')) {
        foreach ($found in @(Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue)) {
            $null = $candidates.Add($found.FullName)
        }
    }

    foreach ($candidate in $candidates) {
        $usable = Test-Python -Exe $candidate
        if ($usable) { return $usable }
    }
    return $null
}

function Install-Python {
    # Winget first where it exists: it is already on the machine and it installs a version the
    # person can uninstall the ordinary way. It is not everywhere -- Windows Server and the LTSC
    # images ship without the App Installer -- so the official installer is the real path and this
    # is the shortcut.
    $winget = Get-Command 'winget.exe' -ErrorAction SilentlyContinue
    if ($winget) {
        Write-Note 'asking winget for Python 3.12 (it may print its own progress)'
        $null = Invoke-Native -File $winget.Source -Arguments @(
            'install', '--exact', '--id', 'Python.Python.3.12', '--scope', 'user',
            '--silent', '--accept-package-agreements', '--accept-source-agreements',
            '--disable-interactivity')
        $found = Find-Python
        if ($found) { return $found }
        Write-Note 'winget left no usable Python behind; using the official installer instead'
    }

    Ensure-Directory $DownloadsDir | Out-Null
    foreach ($version in $PythonFallbacks) {
        $installer = Join-Path $DownloadsDir "python-$version-amd64.exe"
        if (-not (Test-Path -LiteralPath $installer)) {
            Write-Note "downloading Python $version (about 26 MB)"
            $url = "https://www.python.org/ftp/python/$version/python-$version-amd64.exe"
            if (-not (Save-Download -Url $url -Destination $installer -What "Python $version")) {
                continue
            }
        }

        Write-Note "installing Python $version for this user, so no administrator is needed"
        # `InstallAllUsers=0` is what keeps this inside %LOCALAPPDATA% and out of a UAC prompt.
        # `PrependPath=1` is for the person's own later use of `python`; this file does not rely on
        # it, because a running process never sees a PATH change made while it runs.
        $result = Invoke-Native -File $installer -Arguments @(
            '/quiet', 'InstallAllUsers=0', 'PrependPath=1', 'Include_launcher=1', 'Include_pip=1',
            'Include_test=0', 'Include_doc=0', 'Include_tcltk=0', 'SimpleInstall=1')
        if (-not $result.Ok) { Write-Note "the installer exited $($result.Code)" }

        $found = Find-Python
        if ($found) { return $found }
    }
    return $null
}

# ------------------------------------------------------------------------------------------------
#  Node
# ------------------------------------------------------------------------------------------------

function Test-Node {
    param([string] $Exe)

    if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) { return $null }
    $version = Invoke-Native -File $Exe -Arguments @('--version')
    if (-not $version.Ok) { return $null }
    try { $number = [version] $version.Text.Trim().TrimStart('v') } catch { return $null }
    if ($number -lt $NodeMinimum) { return $null }
    return [pscustomobject]@{ Exe = $Exe; Dir = (Split-Path -Parent $Exe); Version = $number }
}

function Find-Node {
    $candidates = New-Object System.Collections.ArrayList

    $null = $candidates.Add((Join-Path $ToolsDir 'node\node.exe'))
    $command = Get-Command 'node.exe' -ErrorAction SilentlyContinue
    if ($command) { $null = $candidates.Add($command.Source) }
    $null = $candidates.Add((Join-Path $env:ProgramFiles 'nodejs\node.exe'))

    foreach ($candidate in $candidates) {
        $usable = Test-Node -Exe $candidate
        # npm has to be beside it: node alone cannot install anything.
        if ($usable -and (Test-Path -LiteralPath (Join-Path $usable.Dir 'npm.cmd'))) {
            return $usable
        }
    }
    return $null
}

# The newest release nodejs.org calls LTS, asked for rather than pinned, so this file does not go
# stale the way a hard-coded version does. The pinned fallback is for a machine whose access to
# nodejs.org is filtered but which can still reach the dist mirror.
function Get-NodeArchiveUrl {
    try {
        $index = Invoke-RestMethod -Uri 'https://nodejs.org/dist/index.json' -TimeoutSec 30
        $lts = $index | Where-Object { $_.lts } | Select-Object -First 1
        if ($lts -and $lts.version) {
            return "https://nodejs.org/dist/$($lts.version)/node-$($lts.version)-win-x64.zip"
        }
    } catch {
        Write-Note 'could not read the Node release index; falling back to a known version'
    }
    return 'https://nodejs.org/dist/v22.14.0/node-v22.14.0-win-x64.zip'
}

function Install-Node {
    $winget = Get-Command 'winget.exe' -ErrorAction SilentlyContinue
    if ($winget) {
        Write-Note 'asking winget for Node.js LTS (it may print its own progress)'
        $null = Invoke-Native -File $winget.Source -Arguments @(
            'install', '--exact', '--id', 'OpenJS.NodeJS.LTS', '--silent',
            '--accept-package-agreements', '--accept-source-agreements',
            '--disable-interactivity')
        $found = Find-Node
        if ($found) { return $found }
        Write-Note 'winget left no usable Node behind; unpacking the official archive instead'
    }

    # The archive rather than the installer: it needs no administrator, it cannot half-install, and
    # it lands in a folder this project owns and can delete. It is a complete Node -- node.exe, npm
    # and npx are all inside it.
    Ensure-Directory $DownloadsDir | Out-Null
    $archive = Join-Path $DownloadsDir 'node-win-x64.zip'
    if (-not (Test-Path -LiteralPath $archive)) {
        Write-Note 'downloading Node.js (about 36 MB)'
        if (-not (Save-Download -Url (Get-NodeArchiveUrl) -Destination $archive -What 'Node.js')) {
            return $null
        }
    }

    $target = Ensure-Directory (Join-Path $ToolsDir 'node')
    $staging = Join-Path $ToolsDir 'node-unpack'
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    if (-not (Expand-Zip -Zip $archive -Destination $staging)) { return $null }

    # The folder is replaced rather than merged into. Moving the archive's contents over an
    # existing install fails the moment it meets a name that is already there -- `node_modules` is
    # a directory in both -- and `Move-Item -Force` cannot overwrite a directory, so the second
    # run of this file, or any run with -Force, died on it. Renaming the unpacked folder into
    # place cannot collide with anything.
    $inner = @(Get-ChildItem -Path $staging -Directory | Select-Object -First 1)
    if ($inner.Count -eq 0) {
        Write-Note 'the Node archive did not contain a folder to unpack'
        return $null
    }
    if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
    Move-Item -LiteralPath $inner[0].FullName -Destination $target -Force
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue

    # Report the interpreter that was just unpacked rather than asking the locator about it. The
    # locator answers "where is Node?", and it is also the function a caller turns to when it has
    # found none -- so an installer reporting through it can answer "nothing" about a job it
    # completed perfectly. That is not hypothetical: it made this file fetch a second archive for
    # a tool it had already installed, and made the check that runs it report two false failures.
    $unpacked = Test-Node -Exe (Join-Path $target 'node.exe')
    if ($unpacked) { return $unpacked }
    return (Find-Node)
}

# ------------------------------------------------------------------------------------------------
#  ffmpeg
# ------------------------------------------------------------------------------------------------

function Test-Ffmpeg {
    param([string] $Exe)

    if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) { return $null }
    $version = Invoke-Native -File $Exe -Arguments @('-version')
    if (-not $version.Ok) { return $null }
    return [pscustomobject]@{ Exe = $Exe; Dir = (Split-Path -Parent $Exe) }
}

# Both binaries from one folder, because a mismatched ffmpeg and ffprobe is a quiet way to be wrong
# about a file rather than a loud way to fail.
function Find-Ffmpeg {
    $candidates = New-Object System.Collections.ArrayList
    $null = $candidates.Add((Join-Path $ToolsDir 'ffmpeg\bin\ffmpeg.exe'))
    if ($env:THE_TRIMMER_FFMPEG) { $null = $candidates.Add($env:THE_TRIMMER_FFMPEG) }
    $command = Get-Command 'ffmpeg.exe' -ErrorAction SilentlyContinue
    if ($command) { $null = $candidates.Add($command.Source) }
    # Where winget puts its shims, for a machine where ffmpeg was installed earlier.
    $null = $candidates.Add((Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links\ffmpeg.exe'))

    foreach ($candidate in $candidates) {
        $ffmpeg = Test-Ffmpeg -Exe $candidate
        if (-not $ffmpeg) { continue }
        $ffprobe = Join-Path $ffmpeg.Dir 'ffprobe.exe'
        if (-not (Test-Ffmpeg -Exe $ffprobe)) { continue }
        return [pscustomobject]@{ Ffmpeg = $ffmpeg.Exe; Ffprobe = $ffprobe; Dir = $ffmpeg.Dir }
    }
    return $null
}

function Install-Ffmpeg {
    # gyan.dev publishes this exact URL for the current release, so it does not go stale. The
    # build is the "essentials" one, which carries libx264 and libx265; prores_ks and dnxhd are
    # ffmpeg's own encoders and are present in every build. The GitHub build is the fallback.
    $sources = @(
        [pscustomobject]@{
            Url  = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'
            Name = 'ffmpeg-release-essentials.zip'
        },
        [pscustomobject]@{
            Url  = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip'
            Name = 'ffmpeg-master-latest-win64-gpl.zip'
        })

    Ensure-Directory $DownloadsDir | Out-Null
    foreach ($source in $sources) {
        $archive = Join-Path $DownloadsDir $source.Name
        if (-not (Test-Path -LiteralPath $archive)) {
            Write-Note 'downloading ffmpeg (about 110 MB, once)'
            if (-not (Save-Download -Url $source.Url -Destination $archive -What 'ffmpeg')) {
                continue
            }
        }

        Write-Note 'unpacking ffmpeg'
        $staging = Join-Path $ToolsDir 'ffmpeg-unpack'
        if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
        if (-not (Expand-Zip -Zip $archive -Destination $staging)) { continue }

        # The build sits one folder down and its name changes with the release, so the folder
        # holding `bin\ffmpeg.exe` is found rather than guessed.
        $binary = Get-ChildItem -Path $staging -Recurse -Filter 'ffmpeg.exe' -File -ErrorAction SilentlyContinue |
                  Select-Object -First 1
        if (-not $binary) {
            Write-Note 'the ffmpeg archive did not contain ffmpeg.exe'
            Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
            continue
        }

        $target = Ensure-Directory (Join-Path $ToolsDir 'ffmpeg\bin')
        foreach ($name in @('ffmpeg.exe', 'ffprobe.exe')) {
            $from = Join-Path $binary.DirectoryName $name
            if (Test-Path -LiteralPath $from) {
                Copy-Item -LiteralPath $from -Destination (Join-Path $target $name) -Force
            }
        }
        # The licences travel with the binaries. They are the terms of redistributing them, so
        # leaving them behind would be the one thing in this file that is not just plumbing.
        #
        # They are looked for beside the binary *and* one folder up, because where a build puts
        # them is not standardised: this was written expecting them in `bin`, where the build it
        # was tested against keeps none, so a working install reported a missing licence and the
        # real fault was the search. The build root is where they are.
        $buildRoot = Split-Path -Parent $binary.DirectoryName
        foreach ($folder in @($binary.DirectoryName, $buildRoot)) {
            foreach ($licence in @(Get-ChildItem -Path $folder -File -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -match '(?i)^(licen[cs]e|copying|readme)' })) {
                Copy-Item -LiteralPath $licence.FullName -Destination (Join-Path $target $licence.Name) `
                    -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue

        # Report the pair that was just unpacked rather than asking the locator: see the note in
        # Install-Node. An installer that reports through the locator can report nothing about a
        # job it finished, and then fetch a second 190 MB archive for no reason.
        $unpacked = Test-Ffmpeg -Exe (Join-Path $target 'ffmpeg.exe')
        if ($unpacked -and (Test-Ffmpeg -Exe (Join-Path $target 'ffprobe.exe'))) {
            return [pscustomobject]@{
                Ffmpeg  = $unpacked.Exe
                Ffprobe = (Join-Path $target 'ffprobe.exe')
                Dir     = $target
            }
        }
    }
    return $null
}

# ------------------------------------------------------------------------------------------------
#  The project's own environment and dependencies
# ------------------------------------------------------------------------------------------------

function Sync-Venv {
    param($Python)

    $venvRoot = Join-Path $ProjectRoot 'venv'
    $venvPython = Join-Path $venvRoot 'Scripts\python.exe'
    if (-not $Force) {
        $existing = Test-Python -Exe $venvPython
        if ($existing) {
            Write-Stage 'engine' "the environment is ready (Python $($existing.Version))"
            return $existing
        }
    }

    Write-Stage 'engine' "creating the environment with Python $($Python.Version)"
    if (Test-Path -LiteralPath $venvRoot) { Remove-Item -LiteralPath $venvRoot -Recurse -Force }
    $created = Invoke-Native -File $Python.Exe -Arguments @('-m', 'venv', $venvRoot)
    if (-not $created.Ok) {
        Stop-With "The engine's environment could not be created." $created.Text
    }

    Write-Note "installing the engine's packages (aiohttp and PyAV, about 40 MB)"
    $null = Invoke-Native -File $venvPython -Arguments @(
        '-m', 'pip', 'install', '--quiet', '--upgrade', 'pip', '--disable-pip-version-check')
    $requirements = Join-Path $ProjectRoot 'backend\requirements.txt'
    $installed = Invoke-Native -File $venvPython -Arguments @(
        '-m', 'pip', 'install', '--quiet', '--disable-pip-version-check', '-r', $requirements)
    if (-not $installed.Ok) {
        # One retry. The usual cause is a network blip part-way through a large wheel, and the
        # second attempt resumes from the cache rather than starting over.
        Write-Note 'that did not finish; trying once more'
        $installed = Invoke-Native -File $venvPython -Arguments @(
            '-m', 'pip', 'install', '--disable-pip-version-check', '-r', $requirements)
    }
    if (-not $installed.Ok) {
        Stop-With "The engine's packages could not be installed." @"
$($installed.Text)

If this machine reaches the internet through a proxy, set HTTPS_PROXY and run start.bat again.
"@
    }
    $null = $script:Provisioned.Add('the engine''s packages')
    return (Test-Python -Exe $venvPython)
}

function Sync-NodePackages {
    param($Node)

    # For this process and everything it starts: npm, and the build tools npm runs.
    $env:PATH = "$($Node.Dir);$env:PATH"
    $npm = Join-Path $Node.Dir 'npm.cmd'

    if (-not $Force -and (Test-Path -LiteralPath (Join-Path $ProjectRoot 'node_modules'))) {
        Write-Stage 'window' 'the packages are already installed'
    } else {
        Write-Stage 'window' "installing the window's packages with npm (about 90 MB)"
        $result = Invoke-Native -File $npm -Arguments @('install', '--no-audit', '--no-fund') -WorkDir $ProjectRoot
        if (-not $result.Ok) {
            Stop-With "The window's packages could not be installed." $result.Text
        }
        $null = $script:Provisioned.Add('the window''s packages')
    }

    # `npm install` fetches the Electron package; its postinstall step is what downloads the
    # binary, and any `ignore-scripts=true` in the person's npm configuration skips it in silence.
    # The result is "Electron failed to install correctly, please delete node_modules/electron" on
    # the first launch, so the download is checked for and run explicitly rather than trusted to a
    # hook.
    $electron = Join-Path $ProjectRoot 'node_modules\electron\dist\electron.exe'
    if (-not (Test-Path -LiteralPath $electron)) {
        Write-Note 'fetching the Electron runtime (about 100 MB)'
        $installer = Join-Path $ProjectRoot 'node_modules\electron\install.js'
        if (Test-Path -LiteralPath $installer) {
            $result = Invoke-Native -File $Node.Exe -Arguments @($installer) -WorkDir $ProjectRoot
            if (-not $result.Ok) { Write-Note $result.Text }
        }
        if (-not (Test-Path -LiteralPath $electron)) {
            Stop-With 'The Electron runtime could not be downloaded.' @'
Check the network, then delete node_modules\electron and run start.bat again.
'@
        }
        $null = $script:Provisioned.Add('the Electron runtime')
    }
    return $npm
}

function Sync-Frontend {
    param([string] $Npm)

    $frontend = Join-Path $ProjectRoot 'frontend'
    if (-not $Force -and (Test-Path -LiteralPath (Join-Path $frontend 'node_modules'))) {
        Write-Stage 'interface' 'the packages are already installed'
    } else {
        Write-Stage 'interface' "installing the interface's packages (React, Vite and TypeScript)"
        $result = Invoke-Native -File $Npm -Arguments @('install', '--no-audit', '--no-fund') -WorkDir $frontend
        if (-not $result.Ok) {
            Stop-With "The interface's packages could not be installed." $result.Text
        }
    }

    # Rebuilt when a source file is newer than the bundle, so an edit is picked up without anyone
    # having to remember this step. The comparison is against the bundle itself and not against
    # this file: comparing to a fixed timestamp rebuilds for the wrong reason and misses the right
    # one the moment the bundle happens to be newer than it.
    $bundle = Join-Path $frontend 'dist\index.html'
    $stale = $Force -or -not (Test-Path -LiteralPath $bundle)
    if (-not $stale) {
        $built = (Get-Item -LiteralPath $bundle).LastWriteTimeUtc
        $changed = @(Get-ChildItem -Path (Join-Path $frontend 'src') -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Extension -in @('.ts', '.tsx', '.css', '.html') -and $_.LastWriteTimeUtc -gt $built })
        if ($changed.Count -gt 0) { $stale = $true }
        foreach ($name in @('index.html', 'vite.config.ts', 'vite.config.js', 'package.json', 'tsconfig.json')) {
            $path = Join-Path $frontend $name
            if ((Test-Path -LiteralPath $path) -and (Get-Item -LiteralPath $path).LastWriteTimeUtc -gt $built) {
                $stale = $true
            }
        }
    }

    if ($stale) {
        Write-Note 'building the interface'
        $result = Invoke-Native -File $Npm -Arguments @('run', 'build') -WorkDir $frontend
        if (-not $result.Ok) { Stop-With 'The interface did not build.' $result.Text }
        if (-not (Test-Path -LiteralPath $bundle)) {
            Stop-With 'The interface build finished but produced no page.' $null
        }
        $null = $script:Provisioned.Add('the interface bundle')
    } else {
        Write-Note 'the interface is already built'
    }
}

function Invoke-EngineChecks {
    param($Python)

    if ($env:THE_TRIMMER_SKIP_TESTS -eq '1') { return }
    $pytest = Invoke-Native -File $Python.Exe -Arguments @('-c', 'import pytest')
    if (-not $pytest.Ok) { return }

    Write-Stage 'checks' "the engine's own tests; they never launch an encoder"
    $result = Invoke-Native -File $Python.Exe -Arguments @('-m', 'pytest', 'backend\tests', '-q') -WorkDir $ProjectRoot
    if ($result.Ok) {
        Write-Note 'the engine''s checks passed'
    } else {
        # The output is printed, not summarised. The first version of this said "the failure is
        # above" while `Invoke-Native` had captured the output and dropped it, so the message
        # pointed at nothing -- a diagnostic thrown away is worse than no diagnostic, because it
        # reads like one was given.
        Write-Host ''
        foreach ($line in ($result.Text -split "`r?`n")) {
            if ($line.Trim()) { Write-Host "  $line" -ForegroundColor DarkGray }
        }
        Write-Host ''
        # A warning rather than a refusal: the application does not need these tests in order to
        # run, and a person who wants to trim something should not be stopped by a red test.
        Write-Note 'the engine''s checks did NOT pass. The window can still be opened; the output'
        Write-Note 'above is the failure. Nothing the dependencies can do will change it.'
    }
}

# ------------------------------------------------------------------------------------------------
#  Main
# ------------------------------------------------------------------------------------------------

function Start-Trimmer {
    Write-Head
    if ($ToolsRoot) { Write-Note "tools root: $ToolsDir" }

    # 1. Python. Found first whatever else was asked for: `-Force` means "provision this project
    #    again", not "download a second Python over a perfectly good one".
    $python = Find-Python
    if ($python) {
        Write-Stage 'python' "using $($python.Version) at $($python.Exe)"
    } else {
        Write-Stage 'python' 'none found on this machine; installing one'
        $python = Install-Python
        if (-not $python) {
            Stop-With 'Python could not be found or installed.' @'
Install it by hand from https://www.python.org/downloads/ -- ticking "Add python.exe to PATH"
-- and run start.bat again.
'@
        }
        $null = $script:Provisioned.Add("Python $($python.Version)")
    }

    # 2. Node.
    $node = Find-Node
    if ($node) {
        Write-Stage 'node' "using $($node.Version) at $($node.Dir)"
    } else {
        Write-Stage 'node' 'none found on this machine; installing one'
        $node = Install-Node
        if (-not $node) {
            Stop-With 'Node.js could not be found or installed.' @'
Install the LTS build from https://nodejs.org/en/download and run start.bat again.
'@
        }
        $null = $script:Provisioned.Add("Node $($node.Version)")
    }

    # 3. ffmpeg. The engine shells out to ffmpeg and ffprobe for every probe and every encode, so
    #    this is not optional the way a build tool would be.
    $ffmpeg = Find-Ffmpeg
    if ($ffmpeg) {
        Write-Stage 'ffmpeg' "using $($ffmpeg.Ffmpeg)"
    } else {
        Write-Stage 'ffmpeg' 'none found on this machine; installing one'
        $ffmpeg = Install-Ffmpeg
        if (-not $ffmpeg) {
            Stop-With 'ffmpeg could not be found or installed.' @'
Install it with "winget install Gyan.FFmpeg", or download a build from
https://www.gyan.dev/ffmpeg/builds/ and put ffmpeg.exe and ffprobe.exe on PATH.
'@
        }
        $null = $script:Provisioned.Add('ffmpeg')
    }

    # The engine reads these two names before it looks on PATH, so pointing them at the binaries
    # just verified is what makes the application use exactly these and not another copy that
    # happens to come first in PATH. Electron inherits them, and so does the engine it starts.
    $env:THE_TRIMMER_FFMPEG = $ffmpeg.Ffmpeg
    $env:THE_TRIMMER_FFPROBE = $ffmpeg.Ffprobe

    # 4. The engine's environment and packages.
    $venvPython = Sync-Venv -Python $python

    # 5. The window's packages, and Electron's own binary.
    $npm = Sync-NodePackages -Node $node

    # 6. The interface.
    Sync-Frontend -Npm $npm

    # 7. The engine's own tests.
    Invoke-EngineChecks -Python $venvPython

    Write-Host ''
    Write-Host ('  ' + ('-' * 66)) -ForegroundColor DarkGray
    if ($script:Provisioned.Count -gt 0) {
        Write-Good ('provisioned: ' + ($script:Provisioned -join ', '))
    }
    Write-Good 'everything the application needs is present'

    if ($NoLaunch) {
        Write-Note 'not opening the window (--NoLaunch was given)'
        Write-Host ''
        return 0
    }

    Write-Note 'opening the window...'
    Write-Host ''
    # Not through Invoke-Native: that captures output, and Electron is a program a person watches
    # and closes, not one whose console is read after it exits.
    Push-Location $ProjectRoot
    try {
        & $npm 'start'
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if ($code -ne 0) {
        Write-Host ''
        Write-Problem 'The window closed with an error.'
        Wait-ForReader
        return 1
    }
    return 0
}

# Runs when this file is executed. It is dot-sourceable as well -- `. .\bootstrap.ps1` -- which is
# how the checks in `scripts/check-bootstrap.ps1` reach the locators without a machine to test on.
# A dot-sourced file has an InvocationName of '.', and an executed one has the path.
if ($MyInvocation.InvocationName -ne '.') {
    exit (Start-Trimmer)
}
