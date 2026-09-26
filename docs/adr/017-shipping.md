# ADR-017: The product is four executables, and the window is checked by starting it

**Status:** accepted
**Date:** the day the `bundle` job was run for the first time, and then deleted

## Context

The repository had a `bundle` job in CI that built MSI and NSIS installers. It ran only on a version
tag, and no tag had ever been pushed — so the installers were a claim rather than a fact. When the job
was finally run locally it failed immediately, on three stacked faults:

1. **`beforeBuildCommand` resolved against the wrong directory.** Tauri runs it from the Cargo manifest
   directory, and the `--prefix ../../web` path landed one level short. Fixing it reproduced the same
   fault the other way, because `npm run build --prefix X` and `npm --prefix X run build` resolve `X`
   from different places. A command whose meaning depends on which of two spellings you used is a
   command that will be wrong on somebody's machine.
2. **The artifact paths in CI were wrong.** They pointed at
   `apps/desktop/src-tauri/target/release/bundle/...`, but this is a Cargo workspace: there is one
   `target/` at the repository root, not one per crate. `if-no-files-found: error` would have failed
   the job *after* a successful build, which is the most expensive moment to find out.
3. **`devUrl` was `http://localhost:5173`** while the dev server binds `127.0.0.1`. On Windows
   `localhost` resolves to `::1` first, so a server on the IPv4 loopback is a connection refused from
   the IPv6 one — the same fault that had made the browser suite's health check time out with no
   message that explained it.

Before any of that, three other faults had produced the same user-visible symptom — a window that
opened, could not load the interface, and showed `ERR_CONNECTION_REFUSED`:

* the `tauri` crate was missing the `custom-protocol` feature, so `cfg(dev)` was set and the release
  build baked in `devUrl` instead of the embedded assets;
* `frontendDist` was `../web/dist`, one directory short of `apps/web/dist`;
* `withGlobalTauri` was absent, so the page had no bridge to call.

Every one of those six faults was invisible to 404 Rust tests, a browser suite and a real media cut.
Each of those tests the *interface* or the *engine*, and not one of them starts the application.

## Decision

**No installer.** The deliverable is four executables that run from anywhere:

| Binary | What it is |
|---|---|
| `thetrimmer-desktop.exe` | the window — its interface is compiled into it |
| `thetrimmer.exe` | the command line: `doctor`, `probe`, `cut`, `batch`, `project`, `export`, `verify`, `watch` |
| `ttrim.exe` | the same command line under a short name |
| `trimmer-daemon.exe` | the loopback HTTP/JSON API |

`start.bat` in the repository root builds the interface if `dist` is missing, builds the window, and
starts it. That is the whole install procedure.

**And a job that starts the window.** `tools/smoke-window.ps1` launches the built binary and asks it
what it thinks it is:

1. it opened a DevTools endpoint, so the WebView2 host is up;
2. the loaded page is the embedded interface rather than a dev server or an error page;
3. the interface rendered **from the build that was just made** — the hashed asset filenames are read
   out of `apps/web/dist/index.html` and then looked for in the live DOM;
4. the status bar reports what the `doctor` command returned, which is a full round trip through
   `invoke` to Rust and back.

The third check is the one that is not theatre. `devUrl`'s string is compiled into the binary either
way, so finding it there proves nothing about which URL is used — a grep for `127.0.0.1:5173` in the
release executable finds it even when the build is correct. What cannot be faked is whether the *page
that loaded* is running `index-DBd9F2oT.js`. A window that fell back to a dev server is serving
`/src/main.tsx` and cannot have it; a window that reached nothing has no DOM at all.

It is in CI as the `desktop` job, which builds the interface, builds the window, and starts it.

## What was run, and what it showed

| Step | Result |
|---|---|
| `cargo build --release -p thetrimmer-desktop` | 7.9 MB executable |
| `tools/smoke-window.ps1` | loads `tauri.localhost`, renders from `index-DBd9F2oT.js` + `index-JmpmfhDw.css`, status bar says `ffmpeg ready` |
| the same script with `dist/index.html` hidden | fails with `no interface at …` and exit 1 — so the check is capable of failing |
| the real CLI, end to end, against a generated master | `doctor` exit 0; project created, source added, segment marked; `batch` → `27 frames in 0.2s, checks passed`, exit 0 |
| the delivered file, probed | `h264 640x360 25/1 duration=1.080000 nb_frames=27` |
| the delivered file's body, against the source, by `framemd5` | **identical** — the copied frames are the original packets |

The 27 frames for a 26-frame request is the stream copy's one-frame overshoot: every frame asked for
is present, one extra came with the last packet, and the report says so rather than failing the cut.
That is ADR-004 and ADR-014, observed rather than asserted.

## Consequences

**Good.**

* One artifact to hand somebody, no elevation, no registry entry, no uninstaller, nothing to sign for
  SmartScreen before it will install. Copying the executable *is* the install.
* The class of defect that made the window unreachable is now covered by a job, which is the first
  time any test in this repository has started the application.

**Recorded here because they were built and then removed.**

Installers were built and verified — MSI and NSIS, both installed, run and uninstalled on a real
machine, with the per-machine MSI correctly refusing without elevation and the per-user path working
with `MSIINSTALLPERUSER=1`. They were removed on request. **No auto-update feed was ever built.** The
`UpgradeCode` in a bundle is what makes an upgrade install over a previous version, and there is no
bundle now, so there is nothing to inherit: an update mechanism is a server, a signing key and a
decision about who hosts it, and it will be designed when it is wanted rather than left half-present.
