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

Workspace scaffold (`Cargo.toml`, `lounge-core` crate) plus these modules, each `cargo test`- and
`cargo clippy --all-targets -- -D warnings`-clean on this (Linux) machine:

- **`vdf.js` → `vdf.rs`.** Ported first: pure, no filesystem/process I/O, already had a test
  (`test/games.test.js`: "vdf parser handles nesting, escapes, comments and case"), ported verbatim as
  `parity_nesting_escapes_comments_and_case` plus extra edge cases.
- **`parse.js` → `parse.rs`.** Filename/folder parsing for movies, episodes, show folders. Uses
  `fancy-regex` (not `regex`) because several patterns need lookahead. All 4 `test/parse.test.js` tests
  ported 1:1.
- **`store.js` → `store.rs`.** Generic JSON-object store with debounced, atomic writes. Had no JS test
  file; 7 new tests cover the same behaviour read from the source (defaults merge, corrupt-file
  fallback, debounce timing, atomic flush).
- **`migrate.js` → `migrate.rs`.** Carries the newest legacy (Foyer/Marquee) data folder over to
  Lounge's own, rewriting embedded artwork paths. Both `test/migrate.test.js` tests ported 1:1.
- **`steam.js` → `steam.rs`.** Library/manifest/artwork scanning. Blocking I/O rather than the JS
  version's async (the future Tauri layer wraps calls in `spawn_blocking` instead). Both `scanSteam`
  tests from `test/games.test.js` ported 1:1. `find_steam`'s registry lookup and
  `launch_quietly`/`running_app_id` are `cfg(windows)`-gated, untestable here, but do at least
  type-check and clippy-clean cross-compiled for `x86_64-pc-windows-gnu` (`rustup target add` +
  `cargo check/clippy --target x86_64-pc-windows-gnu`) — worth doing for every module with a
  `cfg(windows)` block, it already caught one real bug (a `Cow<str>` passed where `fancy-regex` needed
  `&str`) that a Linux-only build would never have seen.
- **`games.js` (partial) → `games.rs`.** `title_from_exe`, `split_args`, `manual_id` ported and tested
  against `test/games.test.js`'s "manual game titles and launch options". `GameSession` — the
  `EventEmitter`-based class that polls Steam's `RunningAppID` or watches a spawned child process — is
  **deliberately not ported yet**: it has no existing JS test to verify against, and how it should
  report state back to the UI is a real design decision (events? a channel?) that depends on the Tauri
  `app` crate's shape, which doesn't exist yet. Porting it blind now would likely mean redoing it once
  that shape is known.
- **`library.js` → `library.rs`.** Recursive media walk, movie/show/episode parsing, local art matching,
  episode sort + auto-numbering. Added a `locale` module as a from-scratch stand-in for
  `localeCompare`'s `numeric`/`sensitivity: 'base'` options (no ICU here). The dirCache the JS version
  keeps as module-level state is threaded through explicitly (`DirCache`, scoped to one
  `scan_libraries` call) instead of living in a global. `test/library.test.js`'s one test ported 1:1.
- **`vlc.js` (locate/build-args parts) → `vlc.rs`.** `language_args`, `build_args`, `find_vlc`/`which`
  ported and tested against both `buildArgs` tests in `test/library.test.js`. `VlcSession` (spawns VLC,
  polls its HTTP interface) deliberately not ported — same reasoning as `GameSession`, and its only test
  (`test/vlc-real.test.js`) needs a real installed VLC, unavailable here.
- **`metadata.js` → `metadata.rs`.** TMDB lookups and image caching. Uses `ureq` (blocking) instead of
  `fetch`, and **real OS threads instead of async workers for `enrich`'s 3-way concurrency — this one
  actually required a design change, not just a mechanical port**: the JS version mutates its cache
  object directly and calls `store.save()` after, which is safe only because JS's "concurrent" workers
  interleave at `await` points and never truly run in parallel. Real threads doing the same plain
  get-then-mutate-then-set would race and silently lose an update. Fixed by adding
  `JsonStore::update()` (read-modify-write while holding the store's lock) and routing `enrich`'s
  writes through it instead — with its own test (`update_is_atomic_across_concurrent_callers`) that
  would fail intermittently without the fix. Both `test/metadata.test.js` tests ported 1:1 against a
  local `httpmock` server (a new dev-dependency; adding `ureq` also pulled in `ring` for TLS, which
  needs a C cross-compiler to build for the Windows target — see the Windows cross-check note below).
- **`gameinfo.js` → `gameinfo.rs`.** Steam store details/search and SteamGridDB artwork lookups, plus
  `stripHtml`. `enrich()` here processes games strictly one at a time like the JS version (no worker
  pool), so none of `metadata.rs`'s concurrency concerns applied. Both `test/games.test.js` tests that
  exercise it ("game info: Steam store details, CDN art and manual-game matching", "a refetch that finds
  nothing keeps the info we already had") and the standalone "stripHtml" test are ported 1:1, plus 3 new
  tests giving direct coverage to `store_search`/`sgdb_search`/`sgdb_images` (JS-truthy `type` filtering,
  deriving a SteamGridDB hit's year from a Unix timestamp via `chrono`, a bad SteamGridDB key surfacing
  as fatal) that the original test file only exercised indirectly through `fetchGame`.

**Windows cross-check confirmed working again** as of this module — `mingw-w64-gcc` is now installed, so
`cargo check/clippy --target x86_64-pc-windows-gnu` covers every module from here on, cfg(windows) or not.

- **`system.js` (self-contained parts) → `system.rs`.** Wi-Fi status (`wifi`), sleep/restart/shutdown
  (`power`), process priority (`set_priority`, via `SetPriorityClass` FFI through `windows-sys` rather
  than shelling out per-pid, closer to what Node's `os.setPriority` actually does), and battery detection
  (`has_battery`). No JS test existed for any of this (nothing in `test/*.test.js` references
  `system.js`) and it's all Windows-only, so it's verified only by `cargo check/clippy --target
  x86_64-pc-windows-gnu` — except the `netsh` output parsing, which was pulled out into its own pure
  function (`parse_netsh_output`) specifically so it *is* testable here, independent of actually running
  `netsh`. `SystemHelper` (spawns a long-lived PowerShell process, talks JSON-lines over stdin/stdout for
  volume/brightness/sleep) is deliberately not ported — same reasoning as `GameSession`/`VlcSession`.
- **`tailscale.js` (pure/injectable parts) → `tailscale.rs`.** `parse_status`, `find_auth_url`,
  `exe_of_command`, `locate_cli`/`find_cli` (Tailscale CLI discovery: standard folders, the Windows
  service's registered `ImagePath`, Start-menu-app hints, `PATH`). All 4 relevant `test/tailscale.test.js`
  tests ported 1:1, including the JS version's own dependency-injection style for `locate_cli` (`exists`,
  `query` as injected closures) — a case where the JS was already written the testable way, so the port
  is closer to mechanical than `steam.rs`'s `find_steam` was. Added `win_join`/`win_dirname` since the JS
  version explicitly uses `path.win32` so Windows-style paths parse correctly under test regardless of
  host OS; Rust's `std::path::Path` doesn't understand `\` as a separator unless actually compiled for
  Windows, so this needed its own small platform-independent implementation.
  The `Tailscale` class itself (`status`/`up`/`down`/`setExitNode`/`startLogin`/`cancelLogin` — all
  subprocess-with-timeout or streaming-with-callback) is deliberately not ported: none of it has a test,
  and it's better designed against the Tauri `app` crate's real async runtime than blocking-and-polled
  here — same reasoning as `GameSession`/`VlcSession`/`SystemHelper`.
- **`transfer-plan.js` → `transfer_plan.rs`.** Fully ported: entirely pure (no filesystem/network access
  at all — the JS file's own top comment says as much), so nothing was deferred. Decides film vs. show,
  the destination folder layout, subtitle-to-video ownership matching, season-pack merging into an
  existing show folder, and `safeName`'s path-injection defense (Windows-reserved names, illegal
  characters, no way to escape the library root). All 6 `test/transfer-plan.test.js` tests ported 1:1,
  including the adversarial one that feeds a `rel` containing literal `..\` sequences and checks every
  planned destination still resolves under the library root.
- **`transfers.js` → `transfers.rs` (fully ported).** The download queue itself, unlike most of the
  stateful classes deferred elsewhere in this migration — its own JS test already drives it through a
  fully fake, injected `client` (just `download`/`close`), proving the design doesn't actually need a
  real network connection to test. Runs on one dedicated worker thread for the queue's lifetime rather
  than the JS version's async/event-loop concurrency. The JS "cancelling stops the download..." test is
  ported 1:1 against an equivalent fake client; a second new test (successful download +
  skip-already-downloaded-with-the-right-size, using an in-memory fake client) covers what that one JS
  test doesn't reach. The third JS test in that file needs a real SFTP round trip through `remote.js` and
  isn't ported — see below.
- **`remote.js` (pure/generic parts) → `remote.rs`.** `sort_entries` and `walk` (generic over an injected
  directory-listing closure, so no real client needed). New tests (JS only exercises this indirectly
  through a real embedded SFTP server) cover natural/case-insensitive sorting, depth-first collection
  with hidden-entry skipping, and error propagation. `connect`/`connect_sftp`/`connect_ftp` — actually
  opening a connection — are deliberately not ported: picking an SSH/FTP crate (a pure-Rust one like
  `russh` is tokio-based) is better decided alongside the Tauri `app` crate's async runtime, and
  `test/remote.test.js` only verifies this module against a real embedded SFTP server, which would mean
  either embedding an SSH server in Rust too just to test against, or shipping an unverified client.
- **`updater.js` → `updater.rs` (fully ported).** The self-updater — this is the actual channel real
  users update through, so unlike most of this migration's stateful classes it's ported and tested in
  full, using the same `ureq` + local `httpmock` approach as `metadata.rs`/`gameinfo.rs`. Covers: GitHub
  Releases lookup and asset matching (preferring `Lounge-`-named assets over legacy `Foyer-`-named ones),
  the API-rate-limit fallback to scraping github.com's redirect and verifying against `SHA256SUMS.txt`,
  checksummed download with progress, the NSIS/zip/FSE-specific install commands (including generating
  the `ZIP_APPLY`/`FSE_APPLY` PowerShell scripts with a BOM and CRLF line endings, and launching the FSE
  installer through WMI to escape the MSIX package), and `wait_for_fse`'s status-file polling. All 9 JS
  tests in `test/updater.test.js` ported 1:1, including the multi-step rate-limit-fallback scenario
  against two separate mock servers (API and web) and the FSE status-file state machine test.
  One JS→Rust adaptation worth calling out: `encodeURIComponent`'s unreserved character set (`- _ . ! ~ *
  ' ( )` stay unescaped) had to be reproduced explicitly, since `percent_encoding`'s `NON_ALPHANUMERIC`
  is more aggressive and would have mangled ordinary filenames like `Lounge-FSE-2.2.0.zip` in the
  fallback download URL.

## What isn't done, and can't be verified from this machine

This development sandbox is Linux with no Rust toolchain pre-installed (added via `mise`, scoped to
this repo only — see `.mise.toml`) and, more importantly, no Windows/WebView2/Steam/VLC/Tailscale to
actually run the app against. Everything below needs a Windows box to build and check by hand before it
can be trusted:

- The `app` crate itself: `tauri.conf.json`, `main.rs`, and one `#[tauri::command]` per IPC handler
  `main.js` currently registers via `ipcMain.handle`.
- A drop-in replacement for `preload.js`'s `contextBridge`-exposed `api` object, so `renderer/**` needs
  little to no change (it already calls everything through that one `api.*` surface).
- `GameSession`, `VlcSession`, `SystemHelper` and the `Tailscale` class (see above) — once the `app`
  crate's event/command shape exists to design against, and (for `VlcSession`) a real VLC install to
  verify against.
- `remote.js`'s actual `connect`/`connect_sftp`/`connect_ftp` (see above) — once an SSH/FTP crate and the
  `app` crate's async runtime are decided.
- `main.js`'s own orchestration (window lifecycle, IPC wiring, playback session tracking) — this is the
  last piece of `src/*.js`, and it's really where the `app` crate begins: it doesn't have much logic of
  its own to port so much as it needs to be *rebuilt* as Tauri commands and window/event setup once that
  crate exists.
- `scripts/build-fse.ps1` and `.github/workflows/build.yml` reworked for `cargo`/Tauri's bundler instead
  of `electron-builder`.
- Retiring/porting `test/*.test.js` (currently `node --test` against the JS modules directly).

## Ground rule

No version bump or GitHub Release tag happens for this work until a Tauri build has actually run on a
Windows handheld and been checked against the Electron build side by side — that tag is what the app's
own auto-updater offers to real users.
