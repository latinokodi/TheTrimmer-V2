# ADR-017: Two installers, and what was actually run to believe them

**Status:** accepted
**Date:** the day the bundle job was run for the first time

## Context

The repository has had a `bundle` job in CI since the beginning, and it had **never been executed**.
It runs only on a tag, and no tag had been pushed. So the installers were a claim rather than a fact,
and when the job was finally run locally it failed immediately:

```text
beforeBuildCommand `npm run build --prefix ../../web` failed with exit code -4058
npm error enoent Could not find 'H:\THEROLLUPFILES\TheTrimmer-V2\web\package.json'
```

Three separate faults, stacked, none of which any test covered:

1. **`beforeBuildCommand` resolved against the wrong directory.** Tauri executes it from the Cargo
   manifest directory, so `../../web` should have been `apps/web` — but the prefix landed one level
   short. Fixing the path then reproduced the same fault one level the other way, because
   `npm run build --prefix X` and `npm --prefix X run build` resolve `X` from different places. A
   command whose meaning depends on which of two spellings you used is a command that will be wrong
   on somebody's machine.
2. **The artifact paths in CI were wrong.** They pointed at
   `apps/desktop/src-tauri/target/release/bundle/...`, but this is a Cargo workspace: there is one
   `target/` at the repository root, not one per crate. `if-no-files-found: error` would have failed
   the job *after* a successful build, which is the most expensive moment to find out.
3. **`devUrl` was `http://localhost:5173`** while the dev server binds `127.0.0.1`. On Windows
   `localhost` resolves to `::1` first, and a server listening on the IPv4 loopback is a connection
   refused from the IPv6 one — the same fault that made the browser test's health check time out with
   no message that explained it.

## Decision

**Build both installers, and verify them by installing them.**

### The build

`beforeBuildCommand` is **deleted**, not fixed. The interface is built by an explicit step in CI
(`npm run build` in `apps/web`) and by `start.bat` when `dist` is missing. There is one place that
knows how to build the interface, and the bundler is not it. A hook repeating that command is a second
copy of the same knowledge, and the copy was the wrong one.

`npm run e2e` in the browser suite cannot see any of this, and the Rust suite cannot either: it is
packaging, and packaging is only tested by packaging.

### What was run, and what it showed

| Step | Result |
|---|---|
| `tauri build` (msi + nsis) | two bundles, 4.54 MB and 2.60 MB |
| MSI is a valid OLE compound file | signature `D0CF11E0A1B11AE1`, `ProductName=TheTrimmer`, `ProductVersion=2.0.0`, `UpgradeCode={CDF524D1-…}` |
| NSIS setup is a valid PE | `MZ` header, exit 0 |
| `msiexec /i … /qn` **without** elevation | **1603, Error 1925** — "insufficient privileges for all users" |
| `msiexec /i … /qn MSIINSTALLPERUSER=1 ALLUSERS=2` | exit 0, installed to `%LOCALAPPDATA%\Programs\TheTrimmer`, uninstalled cleanly |
| NSIS setup `/S` | exit 0, installed to `C:\Program Files\TheTrimmer`, registered in `HKLM\…\Uninstall` |
| the **installed** binary launched | window opened, title `TheTrimmer` |
| NSIS `uninstall.exe /S` | exit 0, `Program Files\TheTrimmer` gone, registry entry gone |

The 1603 is not a defect. The MSI is declared per-machine because a studio workstation may be shared:
per-machine means one install for every editor who logs in, rather than one copy per account with its
own shortcuts and its own update path. Windows requires elevation for that, which is correct
behaviour, and the same package installs per-user when asked with `MSIINSTALLPERUSER=1`. That was
tested rather than assumed, because "it needs admin" and "it is broken" produce the same 1603.

The NSIS installer elevates through UAC on its own, which is why the silent run succeeded from a
non-elevated shell.

### User data is left alone

Uninstalling removes the program and leaves `%APPDATA%\TheTrimmer\projects.db`. A project is the
editor's work; an installer that deletes it on the way out is an installer that loses somebody's
afternoon. Removing that file is a decision for the person who owns it, and `project export` exists
for the case where they want it in a document rather than a database.

## Consequences

**Good.**

* Both installers are real, and the claim is supported by an install rather than by an exit code.
* `tauri build` now works from the repository root, which is where a person runs it.
* The CI artifact paths are right, so a tagged release will actually upload something.

**What is still true and worth stating plainly.**

* The installers are **unsigned**. SmartScreen will warn on first run until a code-signing certificate
  is bought, and the CI `bundle` job stops rather than producing one without it. That is a commercial
  decision — a certificate costs money and is issued to a legal entity — not a technical gap that can
  be closed from inside this repository.
* There is no auto-update feed. `tauri build` can produce one, and the `UpgradeCode` is stable so an
  upgrade installs over the previous version rather than beside it, but nothing publishes and nothing
  checks. Adding it is a server and a key, not code.
