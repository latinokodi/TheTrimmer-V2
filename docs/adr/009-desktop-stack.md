# ADR-009: Tauri v2 and React, not Electron and not a local web server

**Status:** accepted

**Context.** The product is a Windows desktop application with a required installer. V1's UI was
CustomTkinter: two timecode fields and a log. V2's workspace is a project with sources, segments,
a searchable transcript, a cut queue and a proof panel, and — since ADR-006 was overturned — a
timeline strip and a filmstrip of cached thumbnails. Building that in Tkinter is not a serious
option, so the question is which desktop shell carries a web-technology UI.

Three candidates were weighed:

* **Electron.** Bundles Chromium: roughly a **150 MB** runtime, its own release cadence, its own
  CVE stream to track, and its own update surface inside an installer that already has one. It is
  the maturest option and it is a large amount of software to ship for an application whose value
  is a native ffmpeg pipeline and a document exporter.
* **Tauri v2.** Uses the system WebView2. On every supported Windows machine WebView2 is already
  present — it ships with Windows 11 and with updated Windows 10 — and the machine this decision
  was made on reports **version 153**, so the runtime is not a dependency the installer has to
  introduce. Nothing is bundled, so the installer carries the application rather than a browser,
  and the update surface is Windows Update's problem rather than ours. The installer still fetches
  WebView2 when a machine does not have it (see the README's requirements section); that is a
  fallback for a stripped or very old Windows 10, not the normal path.
* **A local HTTP server plus a browser tab.** Rejected. It would have been the cheapest to build,
  and it fails on the product rather than on the technology: the deliverable is a *licensed
  desktop product*. A browser tab cannot be licensed, cannot be associated with a file type,
  cannot own an installer, and cannot present a licence dialog to a studio buyer. The same
  argument rules out shipping the daemon as the whole product — the local API in the README is
  for a studio's pipeline to drive the engine, not the way an editor uses it.

**Decision.** Tauri v2 with a React frontend, shipped as a Windows installer with WebView2
present as a system component. A Node sidecar is not shipped: the engine is Rust, Tauri already
provides the shell, and a Node runtime in the installer would be weight and an upgrade surface
for no gain.

**Consequences.** The shell is small, starts fast, and has no bundled browser to patch. The costs
are real and accepted: the UI is a web page, so it is subject to the WebView2 version on the
machine rather than to a version we control, and a rendering difference between two machines is
now a class of bug that a bundled Chromium would not have had.

The larger consequence is a security boundary that has to be stated because it is easy to erode:
**the frontend is untrusted.** A page is the wrong place to open a file, resolve a path, read a
project, run ffmpeg or hold a licence check, and none of that happens there. Every filesystem
operation and every process launch lives in Rust, and the page reaches them only through typed
IPC commands with declared argument shapes — `trimmer-media` is the only crate that starts a
process, and it is not reachable from the frontend except through those commands. This is the
same rule V1 followed by construction (there was no page), and it is the reason a source file
named `interview; rm -rf ~.mp4` was never a command line: paths are passed as `OsStr` in an
argument array, and nothing is ever assembled into a shell string.

The UI work follows from the shell: tokens rather than literal colours, tabular figures for every
timecode, a keyboard-drivable workspace, and a cut table that is a real grid with row and column
semantics.

**What exists today.** The desktop shell is the current focus of the build and is not in the
repository yet: there is no `apps/` directory, and `apps/desktop/src-tauri` is present in
`Cargo.toml` only as a commented-out workspace member. Everything described above is the decision
the build is being carried out against, not a description of shipped software.
