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
# ## The fault this script then failed to catch, which is why step 4 exists
#
# For several revisions this script reported PASSED on a window that was running **entirely on the
# browser stub**. `installStub()` was meant to decline inside the real window but tested
# `window.__TAURI__` — the convenience global `withGlobalTauri` injects *late* — while the real IPC
# bridge `window.__TAURI_INTERNALS__` is there from the first script. The stub won the race.
#
# The round trip below therefore went to JavaScript, not to Rust: `doctor` was answered from a fixture
# whose ffmpeg build string had been copied from this machine, so the check "the footer says ffmpeg
# ready" passed on a hard-coded string. Nothing in the application did anything real, and the file
# picker returned a fixed path without opening a dialog — which is exactly what a user reported.
#
# A check that can be satisfied by a fixture is not a check. So step 4 asserts the thing the fixture
# cannot fake: **that there is no stub** — plus, because the picker is the one capability the page
# holds, that the picker is actually reachable and actually opens.
#
#   1. the page loaded at all, and it is the interface rather than an error page;
#   2. the interface rendered, from the assets that were just built;
#   3. the bridge is Rust, not a browser fixture;
#   4. the file picker opens — the operating system's own dialog, which is what Browse calls.
#
# ## How it reaches the page without a human
#
# WebView2 speaks the Chrome DevTools Protocol, so `Runtime.evaluate` runs an expression in the
# interface's own JavaScript context. That is the same mechanism the browser suite uses, and it needs
# no window to be focused, no input queue and no screenshot — which matters because this runs in CI.
#
#   pwsh -File tools/smoke-window.ps1
#   pwsh -File tools/smoke-window.ps1 -SkipPicker     # when no desktop session can show a dialog
#
# Exits 0 when the window is healthy, 1 with a reason when it is not.

[CmdletBinding()]
param(
    [string] $Exe,
    [int]    $Port = 9411,
    [int]    $Seconds = 30,
    [switch] $SkipPicker,
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
            # `exceptionDetails.text` is the useless half — it is literally "Uncaught" for anything
            # thrown in JavaScript. The sentence that says what went wrong is on the exception object,
            # and leaving it out is what made a real failure look like a mystery.
            $details = $reply.result.exceptionDetails
            $why = $details.text
            if ($details.PSObject.Properties['exception'] -and
                $details.exception.PSObject.Properties['description']) {
                $why = $details.exception.description
            }
            $where = if ($details.PSObject.Properties['lineNumber']) { " (line $($details.lineNumber))" } else { '' }
            throw "the interface threw while evaluating ``$Expression``:$where $why"
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

    # 4. The bridge is Rust, not a fixture.
    #
    #    This is the check that was missing. The browser stub sets `window.__TAURI__.mocks` to the
    #    boolean `true`; Tauri's own global has a `mocks` *namespace object* under the same name, so the
    #    test has to be a strict comparison against the boolean and nothing looser. Writing
    #    `String(window.__TAURI__.mocks)` here is what the first attempt did, and it threw
    #    `Cannot convert object to primitive value` — because Tauri's namespace is a null-prototype
    #    object, which is a fact worth knowing and was not the thing being asked about.
    $stub = Invoke-Js "window.__TAURI__ && window.__TAURI__.mocks === true ? 'true' : 'false'"
    if ($stub -eq 'true') {
        throw "the window is running the browser stub. Every command is answered from a fixture in TypeScript and nothing in the application is real. The stub must be behind ``import.meta.env.DEV`` in src/main.tsx so a production build removes it, and ``npm run build`` runs tools/check-bundle.mjs to prove it did."
    }
    $internals = Invoke-Js "typeof window.__TAURI_INTERNALS__"
    if ($internals -ne 'object') {
        throw "the page has no IPC bridge (window.__TAURI_INTERNALS__ is $internals), so no command can reach Rust."
    }

    #    The picker is the single capability the page holds, and Browse depends on it. The plugin's own
    #    global API is what the interface calls, so its absence is a dead Browse button — exactly the
    #    fault the user reported, and one the browser suite cannot see because the stub supplies its own.
    $picker = Invoke-Js "typeof (window.__TAURI__ && window.__TAURI__.dialog && window.__TAURI__.dialog.open)"
    if ($picker -ne 'function') {
        throw "the dialog plugin did not install its global API (window.__TAURI__.dialog.open is $picker), so Browse cannot open a picker."
    }
    $stubNote = Invoke-Js "String(window.__TAURI_STUB__ || '')"
    if ($stubNote -ne '') {
        throw "the stub left a note in the window: `"$stubNote`". It should not have been loaded at all in a production build."
    }
    Write-Host '  ok      the bridge is Rust: no stub, real IPC, and the picker is installed' -ForegroundColor Green

    # 5. The bridge answered, more than once. The footer line carries what two commands returned: the
    #    ffmpeg lamp is `doctor`, and the runnable count is `summary`. Both are a full round trip through
    #    `invoke` to Rust and back, and neither can pass without the command being registered.
    #
    #    A failing command puts a `.btn--danger` in the footer instead, so the negative is checked too:
    #    with the stub gone, a command the interface calls on mount that Rust does not know about is a
    #    visible error rather than a silent fixture answer.
    #
    #    The selector moved once, when the shell was rearranged, and this check is the reason that was
    #    noticed: it failed with an empty string rather than reporting a healthy window.
    $bridge = $false
    $deadline = (Get-Date).AddSeconds($Seconds)
    $status = ''
    while ((Get-Date) -lt $deadline -and -not $bridge) {
        $status = Invoke-Js "(() => { const el = document.querySelector('.footerline'); return el ? el.innerText : ''; })()"
        if ($status -match 'ffmpeg ready|no H\.264 encoder|checking ffmpeg' -and $status -match 'runnable') {
            $bridge = $true
        } else {
            Start-Sleep -Milliseconds 400
        }
    }
    if (-not $bridge) {
        throw "the footer line never carried both `doctor` and `summary`. It says: `"$status`""
    }
    $failure = Invoke-Js "(() => { const el = document.querySelector('.footerline .btn--danger'); return el ? el.innerText : ''; })()"
    if ($failure -ne '') {
        throw "a command failed while the window was starting up: `"$failure`". With no stub in the bundle this is a real fault, not a fixture."
    }
    Write-Host "  ok      the bridge answered: $($status.Split([char]10)[0])" -ForegroundColor Green

    # 6. Clicking Browse opens the picker.
    #
    #    The whole chain, in the shipped window: the button, the handler, `pickFile`, the plugin, and the
    #    operating system's dialog. This is the assertion the reported fault needed — "clicking the browse
    #    button should open the windows native file selection modal" — and it is deliberately made
    #    against the button rather than against `dialog.open`, because reaching the plugin is not the
    #    same claim as a button that reaches it.
    #
    #    `open` is wrapped before the click so the call is recorded without being awaited:
    #    `Runtime.evaluate` with `awaitPromise` would block until a human closed the modal. A promise
    #    still pending after two seconds means the dialog is on screen; a rejection means the capability
    #    or the plugin is missing, and the message says which. The window is killed at the end either
    #    way, so nothing is left open.
    if (-not $SkipPicker) {
        $clicked = [string] (Invoke-Js @'
(() => {
  const bridge = window.__TAURI__ && window.__TAURI__.dialog;
  if (!bridge || typeof bridge.open !== "function") { return "no dialog bridge"; }

  window.__PICKER_CALL__ = "not called";
  window.__PICKER_OUTCOME__ = "not called";
  const original = bridge.open.bind(bridge);
  bridge.open = (options) => {
    window.__PICKER_CALL__ = JSON.stringify(options);
    const pending = original(options);
    window.__PICKER_OUTCOME__ = "pending";
    pending.then(
      () => { window.__PICKER_OUTCOME__ = "resolved"; },
      (error) => { window.__PICKER_OUTCOME__ = "rejected: " + String(error); },
    );
    return pending;
  };

  const button = [...document.querySelectorAll(".app button")].find(
    (candidate) => candidate.textContent.trim() === "Browse",
  );
  if (!button) { return "no Browse button on the panel"; }
  if (button.disabled) { return "the Browse button is disabled"; }
  button.click();
  return "clicked";
})()
'@)
        if ($clicked -ne 'clicked') {
            throw "could not press Browse: $clicked"
        }
        Start-Sleep -Seconds 2

        $call = [string] (Invoke-Js "String(window.__PICKER_CALL__)")
        $outcome = [string] (Invoke-Js "String(window.__PICKER_OUTCOME__)")
        if ($call -eq 'not called') {
            throw "the Browse button did not reach the file picker. The button, its handler, or ``pickFile`` is broken — this is the fault that was reported."
        }
        if ($call -notmatch 'mp4' -or $call -notmatch 'mxf') {
            throw "Browse asked the picker for something other than video files: $call"
        }
        if ($outcome -like 'rejected:*') {
            throw "the file picker refused to open: $outcome. Check ``dialog:allow-open`` in capabilities/default.json and that ``tauri_plugin_dialog::init()`` is registered. If this machine has no desktop session to draw a dialog in, run with ``-SkipPicker`` — but check that is really the reason before you do."
        }
        if ($outcome -ne 'pending') {
            throw "the file picker answered ``$outcome`` without a human present, which means it is not the operating system's dialog."
        }
        Write-Host '  ok      Browse opened the Windows file dialog: it is on screen' -ForegroundColor Green
    }

    $socket.Dispose()
    Write-Host ''
    Write-Host 'PASSED  the built window loads its interface, reaches Rust, and can open a picker' -ForegroundColor Green
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
