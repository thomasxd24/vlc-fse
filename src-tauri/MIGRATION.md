# Electron → Tauri migration

Status: **scaffolding + first module ported.** Nothing here replaces Electron yet — `main.js`,
`preload.js` and the app you ship today are untouched. This tree grows alongside them until a Tauri
build has been proven equivalent on a real Windows handheld, at which point we cut over and delete
`main.js`/`preload.js`/`electron-builder`.

## Why Tauri, not just "Rust"

The resource cost of Lounge today is almost entirely Electron's bundled Chromium (renderer + GPU
process); `main.js`/`src/*.js` themselves are cheap I/O glue around VLC/Steam/Tailscale, which do their
own work in separate processes either way. Rewriting only the Node backend in Rust while keeping
Electron would not reduce memory use. Tauri does: it swaps the Node backend for a Rust one *and* drops
the bundled Chromium engine for the OS's own WebView2 (already present on Windows 10/11) — no second
browser engine resident in memory. `renderer/**` (HTML/CSS/JS) stays as-is; only the native side changes.

## Layout

```
src-tauri/
  Cargo.toml          workspace root
  lounge-core/         <- ported business logic (this phase)
  app/                 <- the actual Tauri binary crate (future, Windows-only, see below)
```

`lounge-core` has no dependency on Tauri, WebView, or any windowing toolkit. It's the direct Rust
translation of `src/*.js`'s logic — same function shapes, same behaviour, verified against that file's
existing `test/*.test.js` suite translated 1:1 into `#[test]`s. This crate is what `app` will call into
once it exists.

## What's actually done

- Workspace scaffold (`Cargo.toml`, `lounge-core` crate).
- `src/vdf.js` → `lounge-core/src/vdf.rs`. Chosen first because it's pure (no filesystem/process I/O,
  the safest kind of module to verify by pure translation) and already had a dedicated test
  (`test/games.test.js`: "vdf parser handles nesting, escapes, comments and case"), ported verbatim as
  `parity_nesting_escapes_comments_and_case` plus a few extra edge-case tests. `cargo test` and
  `cargo clippy --all-targets -- -D warnings` both pass.

## What isn't done, and can't be verified from this machine

This development sandbox is Linux with no Rust toolchain pre-installed (added via `mise`, scoped to
this repo only — see `.mise.toml`) and, more importantly, no Windows/WebView2/Steam/VLC/Tailscale to
actually run the app against. Everything below needs a Windows box to build and check by hand before it
can be trusted:

- The `app` crate itself: `tauri.conf.json`, `main.rs`, and one `#[tauri::command]` per IPC handler
  `main.js` currently registers via `ipcMain.handle`.
- A drop-in replacement for `preload.js`'s `contextBridge`-exposed `api` object, so `renderer/**` needs
  little to no change (it already calls everything through that one `api.*` surface).
- Porting the rest of `src/*.js`, roughly in this order (least to most risky):
  1. `parse.js` — filename/title parsing regexes, pure, has `test/parse.test.js`.
  2. `store.js`, `migrate.js` — JSON settings/state persistence.
  3. `steam.js` — Windows registry reads (`reg query`) and VDF-based library scanning; touches `vdf`.
  4. `library.js`, `metadata.js`, `games.js`, `gameinfo.js` — the bulk of the scanning/enrichment logic.
  5. `vlc.js`, `system.js` — process spawning and playback control; needs a real VLC install to verify.
  6. `tailscale.js`, `transfers.js`, `transfer-plan.js`, `remote.js` — network-facing, spawn `tailscale`,
     do FTP transfers.
  7. `updater.js` — talks to GitHub Releases; be careful, this is the same channel real users update
     through, so it's ported last and tested hardest.
  8. `main.js`'s own orchestration (window lifecycle, IPC wiring, playback session tracking).
- `scripts/build-fse.ps1` and `.github/workflows/build.yml` reworked for `cargo`/Tauri's bundler instead
  of `electron-builder`.
- Retiring/porting `test/*.test.js` (currently `node --test` against the JS modules directly).

## Ground rule

No version bump or GitHub Release tag happens for this work until a Tauri build has actually run on a
Windows handheld and been checked against the Electron build side by side — that tag is what the app's
own auto-updater offers to real users.
