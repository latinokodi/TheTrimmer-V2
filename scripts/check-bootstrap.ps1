<#
    The checks behind `bootstrap.ps1`.

    `bootstrap.ps1` is the one file in this project that has to work on a machine nobody has seen,
    where the failure mode is a person double-clicking an icon and watching a console close. The
    parts of it that run on a clean PC -- download, unpack, find the binary that came out -- are
    therefore the parts that must not be taken on trust, and they cannot be reached at all on a
    machine that already has everything installed.

    So this file dot-sources the bootstrap, saves the real locators, replaces them with stubs that
    report nothing found -- which is the state a clean PC is in -- and calls the installers for
    real. Each has to download its archive, unpack it, and produce a binary that runs.

    Then the real locators are put back, and they have to find what the installers left: that is
    what makes a second run of start.bat fast instead of a second hundred megabytes.

    Nothing outside the tools root is touched, and nothing already installed is disturbed.

        powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-bootstrap.ps1

    The tools root is kept between runs, so only the first run downloads. Pass -Clean to start over,
    or -Quick to check the locators alone and skip the downloads.
#>

[CmdletBinding()]
param(
    [string] $ToolsRoot = (Join-Path $env:TEMP 'thetrimmer-bootstrap-check'),
    [switch] $Clean,
    # Only exercise the locators, skipping the large downloads.
    [switch] $Quick
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$script:Passed = 0
$script:Failed = 0

function Check {
    param([string] $What, [scriptblock] $Body)

    try {
        & $Body
        Write-Host "  ok    $What" -ForegroundColor Green
        $script:Passed++
    } catch {
        Write-Host "  FAIL  $What" -ForegroundColor Red
        Write-Host "        $($_.Exception.Message)" -ForegroundColor DarkGray
        $script:Failed++
    }
}

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

Write-Host ''
Write-Host '  bootstrap checks' -ForegroundColor White
Write-Host ('  ' + ('-' * 66)) -ForegroundColor DarkGray

if ($Clean -and (Test-Path -LiteralPath $ToolsRoot)) {
    Remove-Item -LiteralPath $ToolsRoot -Recurse -Force
}
$null = New-Item -ItemType Directory -Path $ToolsRoot -Force

# Dot-source the bootstrap: its functions land in this scope, which is what allows them to be
# replaced below and why the file does not run its main flow when sourced.
. (Join-Path $PSScriptRoot 'bootstrap.ps1') -ToolsRoot $ToolsRoot

# ------------------------------------------------------------------------------------------------
#  The locators, against what is really on this machine
# ------------------------------------------------------------------------------------------------

Write-Host ''
Write-Host '  locators' -ForegroundColor Cyan

Check 'a real Python is accepted' {
    $python = Find-Python
    Assert-True ($null -ne $python) 'no usable Python was found on this machine'
    Assert-True ($python.Version -ge [version] '3.10') "found $($python.Version), which is too old"
    Assert-True (Test-Path -LiteralPath $python.Exe) "$($python.Exe) does not exist"
}

Check 'the Microsoft Store stub is refused' {
    # The trap that matters: the stub opens the Store when run, so treating it as a Python would
    # open a shop window at somebody who only double-clicked an application.
    $stub = Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\python.exe'
    Assert-True ($null -eq (Test-Python -Exe $stub)) 'the Store stub was accepted as a Python'
}

Check 'a Python that is too old is refused' {
    $fake = Join-Path $ToolsRoot 'old-python.cmd'
    Set-Content -LiteralPath $fake -Value '@echo 3.9.13' -Encoding ASCII
    Assert-True ($null -eq (Test-Python -Exe $fake)) 'a 3.9 Python was accepted'
}

Check 'an interpreter without venv is refused' {
    # The embeddable distribution's exact problem: a real Python that cannot build the project's
    # environment. Accepting it turns a clear failure now into a confusing one much later.
    $fake = Join-Path $ToolsRoot 'no-venv.cmd'
    Set-Content -LiteralPath $fake -Encoding ASCII -Value @'
@echo off
if "%1"=="-c" if "%2"=="import venv, ensurepip" exit /b 1
if "%1"=="-c" echo 3.12.10
exit /b 0
'@
    Assert-True ($null -eq (Test-Python -Exe $fake)) 'a Python without venv was accepted'
}

Check 'a Node older than 18 is refused' {
    $fake = Join-Path $ToolsRoot 'old-node.cmd'
    Set-Content -LiteralPath $fake -Value '@echo v16.20.2' -Encoding ASCII
    Assert-True ($null -eq (Test-Node -Exe $fake)) 'a Node 16 was accepted'
}

Check 'ffmpeg alone, without ffprobe, is not a pair' {
    # A folder holding ffmpeg alone must not pass: the engine runs ffprobe on every cut, and a
    # half-pair fails at the first probe rather than at the start.
    #
    # The tools folder is pointed at an empty directory for this, because otherwise the portable
    # pair a previous run of this check installed is a legitimate earlier candidate and the
    # question being asked -- "is the lonely folder enough?" -- is never reached.
    $lonely = Join-Path $ToolsRoot 'lonely'
    $null = New-Item -ItemType Directory -Path $lonely -Force
    $real = Find-Ffmpeg
    Assert-True ($null -ne $real) 'no ffmpeg on this machine to build the half-pair from'
    Copy-Item -LiteralPath $real.Ffmpeg -Destination (Join-Path $lonely 'ffmpeg.exe') -Force

    $savedTools = $ToolsDir
    $savedPath = $env:PATH
    $savedOverride = $env:THE_TRIMMER_FFMPEG
    try {
        $script:ToolsDir = Ensure-Directory (Join-Path $ToolsRoot 'empty-tools')
        $env:PATH = $lonely
        Remove-Item Env:THE_TRIMMER_FFMPEG -ErrorAction SilentlyContinue
        Assert-True ($null -eq (Find-Ffmpeg)) 'a folder with ffmpeg but no ffprobe was accepted'
    } finally {
        $script:ToolsDir = $savedTools
        $env:PATH = $savedPath
        if ($savedOverride) { $env:THE_TRIMMER_FFMPEG = $savedOverride }
    }
}

if ($Quick) {
    Write-Host ''
    Write-Host "  $script:Passed passed, $script:Failed failed  (-Quick: the installers were not exercised)" -ForegroundColor Gray
    exit $(if ($script:Failed -eq 0) { 0 } else { 1 })
}

# ------------------------------------------------------------------------------------------------
#  The installers, with the machine made to look bare
# ------------------------------------------------------------------------------------------------

Write-Host ''
Write-Host '  installers, with nothing found on the machine' -ForegroundColor Cyan

# Saved before they are replaced, so the "second run" check below can use the real ones again.
$realFindNode = ${function:Find-Node}
$realFindFfmpeg = ${function:Find-Ffmpeg}

function Find-Node { return $null }
function Find-Ffmpeg { return $null }

Check 'Node is downloaded, unpacked and runnable' {
    $node = Install-Node
    Assert-True ($null -ne $node) 'Install-Node produced nothing'
    Assert-True (Test-Path -LiteralPath $node.Exe) "$($node.Exe) is not there"
    Assert-True (Test-Path -LiteralPath (Join-Path $node.Dir 'npm.cmd')) 'npm is not beside node.exe'
    Assert-True ($node.Version -ge [version] '18.0') "the unpacked Node is $($node.Version)"

    # It has to run, not merely exist. Single quotes inside the JavaScript, because Windows
    # PowerShell strips embedded double quotes on the way to a native program: `write("ran")`
    # arrives as `write(ran)` and node reports a ReferenceError. The same trap caught the Python
    # version probe in bootstrap.ps1.
    $ran = Invoke-Native -File $node.Exe -Arguments @('-e', "process.stdout.write('ran')")
    Assert-True ($ran.Ok -and $ran.Text -eq 'ran') "the unpacked node did not run: $($ran.Text)"
}

Check 'ffmpeg and ffprobe are downloaded, unpacked and runnable' {
    $ffmpeg = Install-Ffmpeg
    Assert-True ($null -ne $ffmpeg) 'Install-Ffmpeg produced nothing'
    Assert-True (Test-Path -LiteralPath $ffmpeg.Ffmpeg) "$($ffmpeg.Ffmpeg) is not there"
    Assert-True (Test-Path -LiteralPath $ffmpeg.Ffprobe) "$($ffmpeg.Ffprobe) is not there"

    $version = Invoke-Native -File $ffmpeg.Ffmpeg -Arguments @('-version')
    Assert-True $version.Ok "the unpacked ffmpeg did not run: $($version.Text)"
    Write-Host "        $($version.Text.Split("`n")[0].Trim())" -ForegroundColor DarkGray

    $probe = Invoke-Native -File $ffmpeg.Ffprobe -Arguments @('-version')
    Assert-True $probe.Ok 'the unpacked ffprobe did not run'
}

Check 'the unpacked ffmpeg can do everything this engine asks of it' {
    # The reason the archive is fetched rather than any binary that happens to be on PATH: the
    # product re-encodes to H.264, encodes AAC, joins three pieces with the concat demuxer, and
    # offers ProRes and DNxHD. A build missing any of these fails on a real cut, not at startup.
    $exe = (Join-Path $ToolsRoot 'ffmpeg\bin\ffmpeg.exe')

    $encoders = Invoke-Native -File $exe -Arguments @('-hide_banner', '-encoders')
    foreach ($needed in @('libx264', 'libx265', 'aac', 'prores_ks', 'dnxhd')) {
        Assert-True ($encoders.Text -match [regex]::Escape($needed)) "the build has no $needed encoder"
    }
    $demuxers = Invoke-Native -File $exe -Arguments @('-hide_banner', '-demuxers')
    Assert-True ($demuxers.Text -match '(?m)\sconcat\s') 'the build has no concat demuxer'
    $muxers = Invoke-Native -File $exe -Arguments @('-hide_banner', '-muxers')
    Assert-True ($muxers.Text -match '(?m)\smp4\s') 'the build has no mp4 muxer'

    # The licences came with the binaries: they are the terms of redistributing them. Builds do
    # not agree on what to call them, so anything that looks like one counts -- the first version
    # of this looked only for `*.txt` inside `bin`, and a perfectly good install failed here.
    $licences = @(Get-ChildItem -Path (Join-Path $ToolsRoot 'ffmpeg\bin') -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '(?i)^(licen[cs]e|copying|readme)' })
    Assert-True ($licences.Count -gt 0) 'no licence file was carried over with the binaries'
}

Check 'a second run finds what the first one installed' {
    # This is the whole point of the exercise: put the real locators back and they must detect the
    # unpacked tools, so start.bat does not download them a second time.
    Set-Item -Path function:Find-Node -Value $realFindNode
    Set-Item -Path function:Find-Ffmpeg -Value $realFindFfmpeg

    $node = Find-Node
    Assert-True ($null -ne $node) 'the locator did not find the Node the installer unpacked'
    Assert-True ($node.Dir -eq (Join-Path $ToolsRoot 'node')) `
        "the locator found Node at $($node.Dir), not in the tools folder"

    $ffmpeg = Find-Ffmpeg
    Assert-True ($null -ne $ffmpeg) 'the locator did not find the ffmpeg the installer unpacked'
    Assert-True ($ffmpeg.Dir -eq (Join-Path $ToolsRoot 'ffmpeg\bin')) `
        "the locator found ffmpeg at $($ffmpeg.Dir), not in the tools folder"
}

Write-Host ''
if ($script:Failed -eq 0) {
    Write-Host "  $script:Passed passed" -ForegroundColor Green
} else {
    Write-Host "  $script:Passed passed, $script:Failed FAILED" -ForegroundColor Red
}
Write-Host "  tools root: $ToolsRoot" -ForegroundColor DarkGray
Write-Host ''
exit $(if ($script:Failed -eq 0) { 0 } else { 1 })
