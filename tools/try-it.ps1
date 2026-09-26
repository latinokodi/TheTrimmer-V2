# A guided tour of TheTrimmer, on media this script makes itself.
#
#   pwsh -File tools\try-it.ps1
#
# It builds the workspace, makes a 12-second test clip with a known keyframe grid, then walks
# through every surface the product has: the environment report, a probe, a dry run, a real cut
# with the strictest verification, the transcript sidecar, the export formats, and the local API.
#
# Nothing it does is destructive: everything lives in a scratch folder under TEMP, and it prints
# that folder at the end so you can look inside. Run it a second time and it starts over.

param(
    [string]$Work = (Join-Path $env:TEMP 'thetrimmer-tour')
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\thetrimmer.exe'
$daemon = Join-Path $root 'target\release\trimmer-daemon.exe'

function Step($number, $title) {
    Write-Host ''
    Write-Host ("=" * 78) -ForegroundColor DarkGray
    Write-Host "  $number. $title" -ForegroundColor Cyan
    Write-Host ("=" * 78) -ForegroundColor DarkGray
}

function Run($label, $block) {
    Write-Host "`n> $label" -ForegroundColor Yellow
    & $block
}

# ---------------------------------------------------------------------------------------------
Step 0 'Build'
# ---------------------------------------------------------------------------------------------
Push-Location $root
cargo build --workspace --release 2>&1 | Select-Object -Last 2
Pop-Location
if (-not (Test-Path $exe)) {
    Write-Error "the build did not produce $exe"
    exit 1
}

# ---------------------------------------------------------------------------------------------
Step 1 'Make a test master'
# ---------------------------------------------------------------------------------------------
# 300 frames at 25 fps, a keyframe every 25 frames, and the frame number burned into the picture so
# that a frame compared against its neighbours is visibly different. This is the same fixture the
# end-to-end test uses.
Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$master = Join-Path $Work 'master.mp4'

Run "ffmpeg -f lavfi -i testsrc ... -g 25 $master" {
    ffmpeg -hide_banner -v error -y `
        -f lavfi -i 'testsrc=size=640x360:rate=25:duration=12' `
        -vf "drawtext=text='%{n}':fontsize=48:fontcolor=white:x=20:y=20,format=yuv420p" `
        -c:v libx264 -preset veryfast -crf 18 -g 25 -keyint_min 25 -sc_threshold 0 `
        -video_track_timescale 90000 $master
}
Write-Host "wrote $master ($((Get-Item $master).Length) bytes, 300 frames, keyframe every 25)"

# A transcript beside it, so the caption machinery has something to work on.
@'
1
00:00:01,500 --> 00:00:03,000
The first thing he says
2
00:00:03,100 --> 00:00:04,500
and the second thing
3
00:00:09,000 --> 00:00:10,000
well outside the segment
'@ | Set-Content -Path (Join-Path $Work 'master.srt') -Encoding utf8

# ---------------------------------------------------------------------------------------------
Step 2 'doctor — can this machine cut?'
# ---------------------------------------------------------------------------------------------
# This is the first thing to run on any new machine. It resolves ffmpeg and ffprobe, prints what
# the build can encode, and exits 1 when it cannot cut.
Run 'thetrimmer doctor' { & $exe doctor }
Write-Host "exit code: $LASTEXITCODE  (0 means this machine can cut)"

# ---------------------------------------------------------------------------------------------
Step 3 'probe — every fact the cutter depends on'
# ---------------------------------------------------------------------------------------------
Run "thetrimmer probe `"$master`"" { & $exe probe $master }

# ---------------------------------------------------------------------------------------------
Step 4 'A dry run, then a real cut'
# ---------------------------------------------------------------------------------------------
# `--out` is the LAST FRAME KEPT, the way Premiere's Out point works. `--out-exclusive` is for when
# your out point is the frame after the last one.
#
# The in point is deliberately at 00:00:01:12 — frame 37, which is *between* the keyframes at 25 and
# 50. That is what forces the head-patch path: frames 37..49 are re-encoded, frame 50 onward is the
# original packets. Choosing an in point that lands on a keyframe would take the easy copy path and
# tell you much less.
$cut = Join-Path $Work 'cut.mp4'

Run 'thetrimmer cut --dry-run  (shows the exact ffmpeg commands, writes nothing)' {
    & $exe cut $master --in 00:00:01:12 --out 00:00:05:12 -o $cut --dry-run
}

Run 'thetrimmer cut --verify forensic  (cuts, then measures the result)' {
    & $exe cut $master --in 00:00:01:12 --out 00:00:05:12 -o $cut --verify forensic -y
}
Write-Host "exit code: $LASTEXITCODE  (0 means every check passed)"

# ---------------------------------------------------------------------------------------------
Step 5 'What came out'
# ---------------------------------------------------------------------------------------------
# The retimed transcript is written beside the video with the video's name, so a player loads it with
# no further setup. Cue 3 was outside the window and was left out; the first two were shifted.
Run 'the retimed sidecar' { Get-Content (Join-Path $Work 'cut.srt') }
Run 'is the copied body the original packets?  (compare a window frame by frame)' {
    # Two framemd5 lists covering **the same frames**, which is the whole trick and the part that is
    # easy to get wrong.
    #
    # The cut is 00:00:01:12 to 00:00:05:12 at 25 fps: in frame 37, out frame 137. The first keyframe
    # at or after 37 is 50, so frames 37..49 are re-encoded and frames 50..137 are the original
    # packets, copied. Comparing from the keyframe on is therefore the comparison that matters, and
    # the two sides have to be read from the right offsets:
    #
    #   source  frame 50  = 50 / 25 = 2.000 s
    #   cut     frame 50  = (50 - 37) / 25 = 0.520 s
    #
    # An earlier version of this step used 1.48 s for the cut, which starts at source frame 87 — so it
    # compared frames 50..54 against frames 87..91 and reported DIFFERENT for a cut that was
    # byte-identical. The numbers were the bug, not the product.
    $sourceFrom = 50 / 25
    $cutFrom = (50 - 37) / 25
    $a = ffmpeg -hide_banner -v error -ss $sourceFrom -i $master -map 0:v:0 -an -frames:v 5 -f framemd5 - 2>&1 |
        Where-Object { $_ -notmatch '^#' -and $_.Trim() } | ForEach-Object { ($_ -split ',')[-1].Trim() }
    $b = ffmpeg -hide_banner -v error -ss $cutFrom -i $cut -map 0:v:0 -an -frames:v 5 -f framemd5 - 2>&1 |
        Where-Object { $_ -notmatch '^#' -and $_.Trim() } | ForEach-Object { ($_ -split ',')[-1].Trim() }
    Write-Host "source frames 50..54 (from $sourceFrom s): $($a -join ' ')"
    Write-Host "cut    frames 50..54 (from $cutFrom s): $($b -join ' ')"
    if ($a.Count -eq 0 -or $b.Count -eq 0) {
        Write-Host 'COULD NOT MEASURE — ffmpeg returned no frames for one side' -ForegroundColor Yellow
    } elseif (($a -join '') -eq ($b -join '')) {
        Write-Host 'IDENTICAL — the body is the original packets' -ForegroundColor Green
    } else {
        Write-Host 'DIFFERENT — something re-encoded frames it should have copied' -ForegroundColor Red
    }
}

# ---------------------------------------------------------------------------------------------
Step 6 'Export a timeline an editor can import'
# ---------------------------------------------------------------------------------------------
$project = Join-Path $Work 'tour.sqlite'
$masterEscaped = $master

Run "project new --name Tour --store $project" {
    & $exe project new --name 'Tour' --store $project
}
# `project new` prints the id it made, which is the thing every other project command needs.
$projectId = (& $exe project new --name 'Tour 2' --store $project 2>&1 |
    Select-String -Pattern '([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})' |
    ForEach-Object { $_.Matches[0].Value } | Select-Object -First 1)
Write-Host "using project id $projectId"
& $exe project list --store $project

Run 'project add-source <PROJECT> <PATH>' {
    & $exe project add-source $projectId $master --store $project
}

Run 'project add-segment <PROJECT> --source ... --name ... --in ... --out ...' {
    & $exe project add-segment $projectId --source $master --name 'cold open' `
        --in 00:00:01:12 --out 00:00:05:12 --store $project
}
Write-Host "exit code: $LASTEXITCODE"

Run 'project show — the marks, and what will happen to them' {
    & $exe project show $projectId --store $project
}

foreach ($format in 'premiere', 'fcpxml', 'edl', 'csv') {
    $out = Join-Path $Work "timeline.$format"
    Run "export --format $format --output $out" {
        & $exe export $projectId --format $format --output $out --store $project
    }
    if (Test-Path $out) {
        Write-Host "--- first 12 lines of $(Split-Path -Leaf $out) ---" -ForegroundColor DarkGray
        Get-Content $out | Select-Object -First 12
    }
}

# ---------------------------------------------------------------------------------------------
Step 7 'The local API'
# ---------------------------------------------------------------------------------------------
# Loopback only, bearer token required. This is what a studio's pipeline script drives.
$port = 8791
$token = 'a-tour-token-long-enough'
$log = Join-Path $Work 'daemon.log'
$proc = Start-Process -FilePath $daemon `
    -ArgumentList '--port', $port, '--token', $token, '--store', (Join-Path $Work 'api.sqlite') `
    -PassThru -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError "$log.err"
Start-Sleep -Seconds 3

$headers = @{ Authorization = "Bearer $token"; 'Content-Type' = 'application/json' }
$base = "http://127.0.0.1:$port/v1"

function Post($url, $body) {
    (Invoke-WebRequest -Uri $url -Method POST -Headers $headers -Body ($body | ConvertTo-Json -Compress) `
        -UseBasicParsing -TimeoutSec 120).Content
}
function GetJson($url) {
    (Invoke-WebRequest -Uri $url -Headers $headers -UseBasicParsing -TimeoutSec 120).Content
}

Run 'GET /v1/health without a token (expect 401)' {
    try {
        Invoke-WebRequest -Uri "$base/health" -UseBasicParsing -TimeoutSec 5 | Out-Null
        Write-Host 'UNEXPECTED: it answered without a token' -ForegroundColor Red
    } catch {
        Write-Host "refused with $($_.Exception.Response.StatusCode.value__)" -ForegroundColor Green
    }
}

Run 'GET /v1/health with the token' { GetJson "$base/health" }
Run 'GET /v1/capabilities' {
    (GetJson "$base/capabilities" | ConvertFrom-Json) |
        Select-Object version, ffmpeg, libx264, libx265 | Format-List
}

$id = ((Post "$base/projects" @{ name = 'API tour'; created_by = 'tour' }) | ConvertFrom-Json).id
Post "$base/projects/$id/sources" @{ path = $master } | Out-Null
Post "$base/projects/$id/segments" @{
    source = $master; name = 'api cut'; start_frame = 37; end_frame = 138
    preset = $null; handle_frames = 0
} | Out-Null

Run 'GET /v1/projects/{id}/preview  (the dry run, as JSON)' {
    $pv = (GetJson "$base/projects/$id/preview" | ConvertFrom-Json)[0]
    Write-Host "preset=$($pv.preset)  forcesFullEncode=$($pv.forcesFullEncode)"
    Write-Host "commands=$($pv.commands.Count):"
    $pv.commands | ForEach-Object { Write-Host "  - $($_.label)" }
}

Run 'POST /v1/projects/{id}/run  then poll until it finishes' {
    $runId = ((Post "$base/projects/$id/run" @{}) | ConvertFrom-Json).runId
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Seconds 2
        $state = (GetJson "$base/runs/$runId" | ConvertFrom-Json)
        if ($state.state -notin @('queued', 'running')) { break }
    }
    Write-Host "state:  $($state.state)"
    Write-Host "result: succeeded=$($state.outcome.succeeded) unverified=$($state.outcome.unverified) failed=$($state.outcome.failed)"
    Write-Host "$($state.outcome.items[0].status)"
}

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------------------------
Step 8 'The window'
# ---------------------------------------------------------------------------------------------
Write-Host @'
The desktop app is its own binary. It must NOT be `thetrimmer.exe` — that is the command line.

    .\target\release\thetrimmer-desktop.exe

It opens a window titled TheTrimmer with no project loaded. To try it:

  1. Click the project control at the top right ("no project"), type a name, press
     Enter. The project opens and the dialog closes.
  2. "Choose a video..." -> answer the picker with the file this script made:
'@ -ForegroundColor Gray
Write-Host "        $master" -ForegroundColor White
Write-Host @'
     The video appears with its rate, frame count and caption cues.
  3. Type the two timecodes. Try `00:00:01:12` and `00:00:05:12` in the two big
     fields. The frame numbers appear underneath as you type, the length line fills
     in, and it says which of the three things will happen: a lossless copy, a head
     patch, or a full re-encode.
  4. Press "Trim this segment". The queue section opens on the finished file and the
     checks it was measured against. A check that did not run says so rather than
     reporting itself as passed.

The window is a thin shell over the same engine the command line uses, so anything
that works there works here.
'@ -ForegroundColor Gray

# ---------------------------------------------------------------------------------------------
Step 9 'Working on the interface, without building anything'
# ---------------------------------------------------------------------------------------------
Write-Host @'
The interface runs in a plain browser. There is no Rust, no WebView2 and no build in
this loop — which is why an interface change costs a second rather than two minutes:

    cd apps\web
    npm ci
    npm run dev            the whole interface, with a fixture project: edit, refresh,
                           see it. Hot-reloads in well under a second.
    npm test               the formatting arithmetic            (~0.3 s)
    npm run e2e            the interface driven as a person does (~6 s, 10 tests)

`apps/web/src/ipc/stub.ts` is what makes that work: it installs a bridge of the same
shape the window provides and answers the commands with a fixture project. It refuses
to replace a real bridge, so it is a no-op inside the window.

The contract has two tests, of two different things, and neither replaces the other:

    apps\web\tests\*.spec.ts              the interface's behaviour, in a browser
    cargo test -p thetrimmer-desktop --test ipc_contract -- --ignored
                                          the SHAPE of what Rust sends, through
                                          Tauri's real invoke handler

The second is what finds a renamed JSON key; the first cannot, because its stub agrees
with the interface by construction.
'@ -ForegroundColor Gray

# ---------------------------------------------------------------------------------------------
Step 10 'The test suite'
# ---------------------------------------------------------------------------------------------
Write-Host @'
Everything above is also asserted by the suite, on media it generates itself:

    cargo test --workspace              404 tests, including a real cut
    cargo test -p trimmer-media --test end_to_end -- --nocapture
                                        the media proof: byte-identical body, head on
                                        the mark, exact length, timescale pinned
    cargo test -p trimmer-core --test oracle -- --nocapture
                                        the differential oracle against the V1 engine
                                        (631 cases; skips loudly if V1 is not present)

    cargo clippy --workspace --all-targets -- -D warnings
    cd apps\web; npx tsc --noEmit; npm run build
'@ -ForegroundColor Gray

Write-Host "`nEverything this tour made is in:" -ForegroundColor Cyan
Write-Host "  $Work" -ForegroundColor White
Write-Host 'Delete that folder and nothing is left behind.'
