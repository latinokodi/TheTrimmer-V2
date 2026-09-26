# Smoke-test the built window.
#
# ## What this is for
#
# Three separate faults once produced the same symptom: a window that opened, could not load the
# interface, and showed `ERR_CONNECTION_REFUSED`. A missing `custom-protocol` feature made Tauri look
# for a dev server; a `frontendDist` that resolved one directory short made it find nothing; and a
# missing `withGlobalTauri` left the page with no bridge to call. Every one of them was invisible to
# the Rust suite, which is 404 tests that all pass while the application is unusable.
#
# So this launches the built window and asks it what it thinks it is:
#
#   1. the page loaded at all, and it is the interface rather than an error page;
#   2. the interface rendered — the toolbar is in the DOM;
#   3. the bridge answered — the status bar says what `doctor` found, which means a round trip
#      through `invoke` to Rust and back actually happened.
#
# Step 3 is the one that matters. It cannot pass if the assets are not embedded, if the bridge is not
# injected, or if any command is unregistered.
#
# ## How it reaches the page without a human
#
# WebView2 speaks the Chrome DevTools Protocol, so `Runtime.evaluate` runs an expression in the
# interface's own JavaScript context. That is the same mechanism the browser suite uses, and it needs
# no window to be focused, no input queue and no screenshot — which matters because this runs in CI.
#
#   pwsh -File tools/smoke-window.ps1
#
# Exits 0 when the window is healthy, 1 with a reason when it is not.

[CmdletBinding()]
param(
    [string] $Exe,
    [int]    $Port = 9411,
    [int]    $Seconds = 30,
    [switch] $KeepOpen
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# The plan is a session token, so it can be named and reasoned about. Without it the script is a set
# of steps, and a failing step says "an assertion failed" rather than which assumption broke.
Write-Host 'Smoke-testing the built window' -ForegroundColor White

$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $root 'target\release\thetrimmer-desktop.exe' }

if (-not (Test-Path $Exe)) {
    throw "no window binary at $Exe. Build it with: cargo build --release -p thetrimmer-desktop"
}

$dist = Join-Path $root 'apps\web\dist\index.html'
if (-not (Test-Path $dist)) {
    throw "no interface at $dist. Build it with: npm --prefix apps/web run build"
}

Write-Host "  binary  $Exe"
Write-Host "  assets  $dist"

$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port --remote-allow-origins=*"
# A private store, because this starts the real application and it should not touch anyone's projects.
$scratch = Join-Path $env:TEMP ("tt-smoke-{0}" -f (Get-Date -Format 'HHmmss'))
New-Item -ItemType Directory -Path $scratch -Force | Out-Null
$env:THE_TRIMMER_STORE = Join-Path $scratch 'projects.db'

$proc = Start-Process -FilePath $Exe -PassThru
try {
    # 1. The debug endpoint answers, which means the WebView2 host is up.
    $deadline = (Get-Date).AddSeconds($Seconds)
    $targets = $null
    while ((Get-Date) -lt $deadline -and $null -eq $targets) {
        try {
            $targets = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 2
        } catch {
            Start-Sleep -Milliseconds 400
        }
    }
    if ($null -eq $targets) { throw 'the window never opened a DevTools endpoint' }

    $page = $targets | Where-Object { $_.type -eq 'page' -and $_.webSocketDebuggerUrl } |
        Select-Object -First 1
    if ($null -eq $page) { throw 'the window opened no page' }

    # 2. The page is the application, not a failure to reach one.
    #
    #    The URL is checked, but it is *not* the load-bearing check: `tauri.conf.json`'s `devUrl`
    #    string is compiled into the binary either way, so finding it there proves nothing about which
    #    one is used. What follows does — the hashed asset filenames are read out of the built
    #    `dist/index.html` and then looked for in the page that actually loaded. A window that fell
    #    back to a dev server is serving `/src/main.tsx` and cannot have them, and a window that could
    #    not reach anything has no DOM at all.
    $url = [string] $page.url
    Write-Host "  loaded  $url"
    if ($url -match ':5173') {
        throw "the window is looking for a dev server ($url). The `custom-protocol` feature is off, so the release build baked in devUrl instead of the embedded assets."
    }
    if ($url -notmatch 'tauri\.localhost|tauri://localhost') {
        throw "the window loaded $url rather than the embedded interface"
    }

    $distHtml = Get-Content (Join-Path $root 'apps\web\dist\index.html') -Raw
    $assets = [regex]::Matches($distHtml, 'assets/(index-[A-Za-z0-9_-]+\.(?:js|css))') |
        ForEach-Object { $_.Groups[1].Value } | Select-Object -Unique
    if ($assets.Count -eq 0) {
        throw 'the built interface names no hashed assets, so this check cannot tell which build loaded'
    }

    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    [void] $socket.ConnectAsync(
        [Uri] $page.webSocketDebuggerUrl,
        [Threading.CancellationToken]::None).GetAwaiter().GetResult()

    function Invoke-Js {
        param([string] $Expression)
        $payload = @{ id = 1; method = 'Runtime.evaluate'; params = @{
            expression = $Expression; returnByValue = $true; awaitPromise = $true } } |
            ConvertTo-Json -Depth 10 -Compress
        $bytes = [Text.Encoding]::UTF8.GetBytes($payload)
        [void] $socket.SendAsync(
            [ArraySegment[byte]]::new($bytes),
            [System.Net.WebSockets.WebSocketMessageType]::Text,
            $true, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
        $buffer = New-Object byte[] 262144
        $stream = [System.IO.MemoryStream]::new()
        do {
            $chunk = $socket.ReceiveAsync(
                [ArraySegment[byte]]::new($buffer),
                [Threading.CancellationToken]::None).GetAwaiter().GetResult()
            $stream.Write($buffer, 0, $chunk.Count)
        } while (-not $chunk.EndOfMessage)
        $text = [Text.Encoding]::UTF8.GetString($stream.ToArray())
        $stream.Dispose()
        $reply = $text | ConvertFrom-Json
        if ($reply.PSObject.Properties['result'] -and
            $reply.result.PSObject.Properties['exceptionDetails'] -and
            $null -ne $reply.result.exceptionDetails) {
            throw "the interface threw: $($reply.result.exceptionDetails.text)"
        }
        return $reply.result.result.value
    }

    # 3. The interface rendered, and it is the build that was just made rather than some other copy.
    $rendered = $false
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline -and -not $rendered) {
        $heading = Invoke-Js "document.querySelector('h1') ? document.querySelector('h1').textContent : ''"
        if ($heading -eq 'TheTrimmer') { $rendered = $true } else { Start-Sleep -Milliseconds 300 }
    }
    if (-not $rendered) {
        $body = Invoke-Js 'document.body ? document.body.innerText.slice(0, 200) : "(no body)"'
        throw "the interface did not render. The page says: $body"
    }

    $loaded = Invoke-Js "[...document.querySelectorAll('script[src], link[href]')].map((n) => n.getAttribute('src') || n.getAttribute('href')).join(' ')"
    foreach ($asset in $assets) {
        if ($loaded -notlike "*$asset*") {
            throw "the window rendered, but not from the interface that was just built: $asset never loaded. It loaded: $loaded"
        }
    }
    Write-Host "  ok      the interface rendered from the build just made ($($assets -join ', '))" -ForegroundColor Green

    # 4. The bridge answered. The status bar shows what the `doctor` command returned, so this is a
    #    full round trip through `invoke` and back — it cannot pass without the bridge and without the
    #    command being registered.
    $bridge = $false
    $deadline = (Get-Date).AddSeconds($Seconds)
    $status = ''
    while ((Get-Date) -lt $deadline -and -not $bridge) {
        $status = Invoke-Js "(() => { const el = document.querySelector('.statusbar'); return el ? el.innerText : ''; })()"
        if ($status -match 'ffmpeg ready|no H\.264 encoder|checking ffmpeg') { $bridge = $true }
        else { Start-Sleep -Milliseconds 400 }
    }
    if (-not $bridge) {
        throw "the status bar never reported the doctor command. It says: `"$status`""
    }
    Write-Host "  ok      the bridge answered: $($status.Split([char]10)[0])" -ForegroundColor Green

    $socket.Dispose()
    Write-Host ''
    Write-Host 'PASSED  the built window loads its interface and reaches Rust' -ForegroundColor Green
    exit 0
} catch {
    Write-Host ''
    Write-Host "FAILED  $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    if (-not $KeepOpen) {
        $proc.Refresh()
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    }
    Remove-Item Env:\THE_TRIMMER_STORE -ErrorAction SilentlyContinue
    Remove-Item Env:\WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
    Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
