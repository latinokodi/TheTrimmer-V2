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
# ## The fault this script then failed to catch, which is why step 3 exists
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
# A check that can be satisfied by a fixture is not a check. So step 3 asserts the thing the fixture
# cannot fake: **that there is no stub** — plus, because the picker is the one capability the page
# holds, that the picker is actually reachable and actually opens.
#
# The fault that produced steps 6, in the same shape
#
# A user then reported being unable to restore, minimize or close the window. Tauri said the window was
# decorated, minimizable, maximizable and closable; Windows said it had no caption at all. Asking the
# wrong party produced a healthy report about an unusable window, twice.
#
#   1. the page loaded at all, and it is the interface rather than an error page;
#   2. the interface rendered, from the assets that were just built;
#   3. the bridge is Rust, not a browser fixture;
#   4. the commands the interface calls on mount answered;
#   5. the window has a titlebar, minimizes, restores and toggles fullscreen;
#   6. clicking Browse opens the operating system's file dialog;
#   7. the window closes when asked to close.
#
# Step 5 asks Windows for the window's style bits rather than asking Tauri, and that is not pedantry:
# `isDecorated()`, `isMinimizable()`, `isMaximizable()` and `isClosable()` all answered `true` on a
# window that had no caption, no system menu and no minimize or maximize box. Tauri describes the window
# that was requested. Only the style bits describe the window that exists.
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

    # 6. The window can be restored, minimized and closed.
    #
    #    A user reported being unable to do any of those three. The cause was that the window opened
    #    borderless fullscreen: `GetWindowLong` on it returned `0x14000000` — `WS_VISIBLE |
    #    WS_CLIPCHILDREN` and nothing else, so there was no caption, no system menu, no minimize box, no
    #    maximize box and no resizable frame. The page could not do it either: `minimize` and
    #    `set_fullscreen` are both refused by the capability file.
    #
    #    This asks **Windows**, not Tauri, and that distinction is the whole reason the fault lasted.
    #    `isDecorated()`, `isMinimizable()`, `isMaximizable()` and `isClosable()` all returned `true` on
    #    that window — Tauri describes the window that was *requested*, and every one of those answers
    #    was wrong about the window that existed. A check built on them would have reported a healthy
    #    window with no titlebar. The style bits cannot lie.
    $proc.Refresh()
    $handle = $proc.MainWindowHandle
    if ($handle -eq 0) {
        throw "the window has no main window handle, so nothing about its frame can be checked."
    }

    Add-Type -Namespace Trimmer -Name Win -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError=true)]
public static extern int GetWindowLong(System.IntPtr hWnd, int nIndex);
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError=true)]
public static extern bool IsIconic(System.IntPtr hWnd);
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError=true)]
public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError=true)]
public static extern bool PostMessage(System.IntPtr hWnd, uint msg, System.IntPtr wParam, System.IntPtr lParam);
'@

    $GWL_STYLE = -16
    $SW_MINIMIZE = 6
    $SW_RESTORE = 9
    $WM_CLOSE = 0x0010

    $style = [Trimmer.Win]::GetWindowLong($handle, $GWL_STYLE)
    if ($style -eq 0) {
        throw "GetWindowLong returned 0 for the window, so the frame cannot be read (last error $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error()))."
    }
    foreach ($bit in @(
            @{ Mask = 0x00C00000; Name = 'WS_CAPTION';     Why = 'a titlebar to drag, double-click and restore from' },
            @{ Mask = 0x00080000; Name = 'WS_SYSMENU';     Why = 'the system menu, which is where Close lives' },
            @{ Mask = 0x00020000; Name = 'WS_MINIMIZEBOX'; Why = 'the minimize button' },
            @{ Mask = 0x00010000; Name = 'WS_MAXIMIZEBOX'; Why = 'the maximize and restore button' },
            @{ Mask = 0x00040000; Name = 'WS_THICKFRAME';  Why = 'a resizable, snappable frame' }
        )) {
        if (($style -band $bit.Mask) -ne $bit.Mask) {
            throw "the window has no $($bit.Name) (GWL_STYLE is 0x$('{0:X8}' -f $style)), so there is $($bit.Why). The window must open maximized with decorations rather than fullscreen; see ADR-020."
        }
    }
    Write-Host "  ok      the window has a titlebar: caption, system menu, minimize and maximize boxes (GWL_STYLE 0x$('{0:X8}' -f $style))" -ForegroundColor Green

    #    How much room the panel was given, printed rather than asserted. The window is now smaller than
    #    it was — a titlebar and a taskbar cost about 80 logical pixels — and the question worth asking
    #    is whether the layout still fits. It does, and the browser suite is what proves it: it renders
    #    the whole panel at 1440x960 and at 1920x1080 and fails on any zone squeezed below its content,
    #    which is a *smaller* window than this one. Asserting it here as well would fail on a machine
    #    whose screen is smaller than the design, which is the environment's limit and not a fault.
    $client = [string] (Invoke-Js "window.__TAURI__.window.getCurrentWindow().innerSize().then((v) => v.width + 'x' + v.height)")
    $needed = [string] (Invoke-Js "(() => { const s = getComputedStyle(document.documentElement); return s.getPropertyValue('--frame-min-width').trim() + 'x' + s.getPropertyValue('--frame-min-height').trim(); })()")
    Write-Host "  ..      client area $client, layout minimum $needed" -ForegroundColor DarkGray

    #    Minimized the way the titlebar's button minimizes it, and restored again. `IsIconic` is Windows
    #    saying the window is minimized, which is the question the user could not get answered.
    [void] [Trimmer.Win]::ShowWindow($handle, $SW_MINIMIZE)
    Start-Sleep -Milliseconds 700
    if (-not [Trimmer.Win]::IsIconic($handle)) {
        throw "the window did not minimize when asked, so its minimize button cannot work."
    }
    [void] [Trimmer.Win]::ShowWindow($handle, $SW_RESTORE)
    Start-Sleep -Milliseconds 700
    if ([Trimmer.Win]::IsIconic($handle)) {
        throw "the window did not restore from minimized."
    }
    Write-Host '  ok      the window minimized and restored' -ForegroundColor Green

    #    Fullscreen, through the interface's own button, because that is the one window state the page
    #    owns — and it is a round trip, since a one-way trip into fullscreen is the fault being fixed.
    $asked = [string] (Invoke-Js @'
(() => {
  const button = [...document.querySelectorAll(".titlebar button")].find(
    (candidate) => /^(Fullscreen|Restore)$/.test(candidate.textContent.trim()),
  );
  if (!button) { return "no fullscreen control on the titlebar"; }
  if (button.textContent.trim() !== "Fullscreen") { return "the control does not start at Fullscreen"; }
  button.click();
  return "clicked";
})()
'@)
    if ($asked -ne 'clicked') {
        throw "could not press the fullscreen control: $asked"
    }
    Start-Sleep -Seconds 2
    $nowFullscreen = [string] (Invoke-Js "window.__TAURI__.window.getCurrentWindow().isFullscreen().then((v) => String(v))")
    if ($nowFullscreen -ne 'true') {
        throw "the interface's fullscreen control did not put the window in fullscreen (isFullscreen is $nowFullscreen). Check ``core:window:allow-set-fullscreen`` in capabilities/default.json."
    }
    [void] (Invoke-Js @'
(() => {
  [...document.querySelectorAll(".titlebar button")]
    .find((candidate) => candidate.textContent.trim() === "Restore")
    .click();
  return "clicked";
})()
'@)
    Start-Sleep -Seconds 2
    $backAgain = [string] (Invoke-Js "window.__TAURI__.window.getCurrentWindow().isFullscreen().then((v) => String(v))")
    if ($backAgain -ne 'false') {
        throw "the window went into fullscreen and would not come back out (isFullscreen is $backAgain). Fullscreen the operator cannot leave is the fault this whole step exists for."
    }
    Write-Host '  ok      fullscreen goes both ways, from the interface''s own control' -ForegroundColor Green

    # 7. Clicking Browse opens the picker.
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

    # 8. And closed, the way the titlebar's X closes it. This is last because it ends the process.
    [void] [Trimmer.Win]::PostMessage($handle, $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $proc.WaitForExit(10000)) {
        throw "WM_CLOSE did not close the window within ten seconds, so the Close button would not either."
    }
    Write-Host '  ok      the window closed when asked to close' -ForegroundColor Green

    $socket.Dispose()
    Write-Host ''
    Write-Host 'PASSED  the window loads its interface from Rust, can open a picker, and can be controlled' -ForegroundColor Green
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
