# Electron → Tauri migration

Status: **every `src/*.js` module ported or deliberately deferred; a real, running Tauri shell exists
and has been verified end to end on Linux.** Nothing here replaces Electron yet — `main.js`, `preload.js`
and the app you ship today are untouched. This tree grows alongside them until a Tauri build has been
proven equivalent on a real Windows handheld, at which point we cut over and delete
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
  lounge-core/         <- ported business logic, no Tauri/WebView dependency (71 tests)
  app/                 <- the Tauri binary crate: window, commands, the real (if partial) shell
```

`lounge-core` has no dependency on Tauri, WebView, or any windowing toolkit. It's the direct Rust
translation of `src/*.js`'s logic — same function shapes, same behaviour, verified against that file's
existing `test/*.test.js` suite translated 1:1 into `#[test]`s. `app` is what actually calls into it.

## The `app` crate: real, running, honestly scoped

This is **not** a 1:1 port of `main.js` — see `app/src/lib.rs`'s module doc for exactly why (in short:
`main.js` integrates a long list of Electron APIs with no Rust equivalent chosen yet, plus every piece
this migration deliberately deferred, all at once — porting that blind would be a far bigger unverified
bet than anything else in this migration). Instead it proves the *architecture* end to end: a real
window, loading the actual `renderer/**` UI completely unmodified, driven by real `lounge-core` calls
through Tauri commands.

**Verified by actually running it** (this sandbox has webkit2gtk and a real display, so unlike everything
`cfg(windows)`, this could be built and run for real rather than just type-checked): `cargo run -p
lounge-app` opens a real window, plays Lounge's actual startup intro, and lands on the real "Welcome to
Lounge" screen — same HTML/CSS/JS as the Electron build, unmodified. "VLC not found yet. Install it from
videolan.org…" on screen is `vlc::find_vlc()` genuinely reporting no VLC on this machine, not a
placeholder string.

Running it for real caught a bug static checking wouldn't have: `renderer/app.js`'s boot sequence does
`await api.detectVlc()` with no `.catch`, so a stubbed command that *rejects* (as every not-yet-wired
command in this shell deliberately does, to be honest about what isn't implemented) silently aborted the
rest of `boot()` — including ending the startup intro, which is why the app sat on the splash screen
indefinitely the first time it ran. Fixed by backing `detect_vlc` with the real `vlc::find_vlc` instead
of the stub, which doubles as the first real proof that a `lounge-core` module and a Tauri command
compose correctly. The lesson generalizes: any command the renderer calls *unconditionally* during boot
needs a real implementation or a safe neutral default, never a rejection — reserve "not implemented"
stubs for commands only reachable through explicit user action (a button click), where an error toast is
acceptable UX for now.

What's wired for real in `app/src/lib.rs`: `get_state` (settings from a real `JsonStore` merged over
defaults; library from a real `library::scan_libraries` call), `rescan`, `save_settings`, `detect_vlc`,
`quit`, `minimize`, `toggle_fullscreen`, plus `window.lounge` itself — recreated as a small
`initialization_script` mapping every method `preload.js` exposes to `window.__TAURI__.core.invoke(...)`,
so `renderer/**` needed zero changes to run against either shell. Every other command (`play`, `playGame`,
`addGame`, dialogs, Tailscale, transfers, system controls, updates, …) is present as a real, callable
`window.lounge.*` method but rejects with a clearly-labelled "not yet implemented" error — the renderer
can call it and gets a real (if unhelpful) answer, not a silent hang or a thrown "unknown command".

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
this repo only — see `.mise.toml`). It does have webkit2gtk and a real display, which is how `app` could
be built and actually run rather than merely type-checked — but it has no Windows/WebView2, and no
Steam/VLC/Tailscale/real media library to exercise those integrations against. See the checklist below
for specifics; broadly, everything needs a Windows box, and most of it also real installs of the things
it talks to, before it can be trusted.

## TODO: everything left before a cutover is even on the table

Checked = actually wired in `app` and working (verified here, on Linux, except where noted). Unchecked =
not done. Nothing in this list should be treated as "mostly there" — an unchecked box means the
`window.lounge.*` method either doesn't exist or currently rejects with "not yet implemented".

### IPC commands — Media & playback

- [x] `get-state` — real (settings + scanned library)
- [ ] `play` / `stop` / `np-command` — need `VlcSession` (see below) built and wired
- [ ] `set-watched`, `set-languages`, `set-pref` — need `progressStore`/`prefsStore` (plain `JsonStore`
      instances, mechanical) plus the field-merge logic `main.js` does around them
- [ ] `get-stats` — needs `statsStore` + the `statsData()` view assembly

### IPC commands — Games

- [ ] `play-game` / `end-game` / `back-to-game` — need `GameSession` (see below)
- [ ] `add-game` — needs a folder/file picker (see Electron-API replacements) + `gamesStore` + `iconFor`
      (`app.getFileIcon` equivalent)
- [ ] `edit-game` / `remove-game` — needs `gamesStore` overrides plus calling `gameInfo.forget` +
      re-enriching; the underlying `gameinfo.rs` logic is already ported
- [ ] `search-steam` / `search-sgdb` / `sgdb-images` / `set-game-art` / `screenshot` — the underlying
      `gameinfo::store_search`/`sgdb_search`/`sgdb_images`/`download` are **already ported and tested** in
      `lounge-core`; this is "just" wiring a `GameInfo` instance into `AppState` and mapping these commands
      to it, plus `screenshot`'s CDN-domain allowlist check
- [ ] `show-game-folder` — needs a "reveal in file manager" equivalent (see Electron-API replacements)

### IPC commands — Settings & dialogs

- [x] `save-settings` — only the allowed-keys merge; missing the side effects `main.js` does around it
      (relaunching a rescan when `libraries`/`steamEnabled`/`steamPath` change, re-enriching on a language
      or `sgdbKey` change, `setLoginItemSettings`, live `setFullScreen`)
- [ ] `pick-folder` / `pick-vlc` / `pick-key-file` — need a native file/folder picker
- [ ] `clear-metadata` — needs `metaStore`/`gameInfoStore` cleared + re-triggering `enrich`/`enrichGames`
- [ ] `show-in-folder` — needs a "reveal in file manager" equivalent

### IPC commands — System (quick menu & status bar)

- [ ] `system-get` / `system-set` — need `SystemHelper` (see below)
- [ ] `wifi` — `system::wifi` is **already ported**; just needs wiring (and the `platform === 'win32'`
      gate the renderer itself already applies)
- [ ] `power` — `system::power` is **already ported**; needs wiring plus flushing every `JsonStore`
      before sleep/restart/shutdown, same as `main.js` does
- [ ] `open-external` — needs a "open URL in default browser" equivalent, restricted to `https://` and
      `ms-settings:` like the original

### IPC commands — Servers, remote browsing & transfers

- [ ] `server-save` / `server-remove` / `server-forget-key` / `server-test` — need `serversStore` plus
      (for `-test`) a real remote client
- [ ] `remote-list` / `remote-plan` / `remote-download` — `transfer_plan.rs` is **already ported**; all
      three need `remote.js`'s actual SFTP/FTP client to exist first (see below)
- [ ] `transfer-cancel` / `transfer-clear` / `transfer-retry` — `transfers.rs`'s `TransferQueue` is
      **already ported and tested**; needs a `TransferQueue` instance in `AppState` (constructed with a
      real `connect` closure once the remote client exists) and its `update`/`finished` events wired to
      `app.emit`

### IPC commands — Apps & Tailscale

- [ ] `apps-rescan` / `app-launch` / `app-hide` — need `src/apps.js` ported first (not started; Windows
      Start-menu scanning via `Get-StartApps`, entirely untested territory)
- [ ] `tailscale-status` — `tailscale::parse_status` is **already ported**; needs the `Tailscale`
      struct's actual `tailscale status --json` invocation built
- [ ] `tailscale-locate` — `tailscale::locate_cli`/`find_cli` are **already ported and tested**; needs
      wiring with real `fs`/registry closures
- [ ] `tailscale-action` (up/down/exit-node), `tailscale-login`, `tailscale-cancel-login` — need the
      `Tailscale` struct's process-spawning methods built (see below); `tailscale-login` also needs a QR
      code generator (`qrcode` on the JS side — pick a Rust equivalent, e.g. the `qrcode` crate)
- [ ] `tailscale-open-app` — depends on `apps.js`

### IPC commands — Updates

- [ ] `update-check` — `updater.rs` is **fully ported and tested**; this is close to a pure wiring task
      (construct an `Updater` in `AppState`, call `.check()`, emit its state)
- [ ] `update-install` — needs the actual install flow: flushing every store, spawning the installer
      command detached (NSIS/zip) or running the FSE flow (`install_command` + `wait_for_fse`, both
      already ported), then quitting
- [ ] `update-skip` — trivial once `save-settings`-style store access exists

### Stateful `lounge-core` pieces still to build (all currently just documented as deferred)

- [ ] `GameSession` — tracks a running Steam or manual game; needs a concrete design for how it reports
      state back to `app` (a channel? direct `AppHandle::emit` calls from a background thread?) — this is
      an actual design decision, not a mechanical port
- [ ] `VlcSession` — spawns VLC, polls its HTTP interface for playback progress; same kind of design
      decision as `GameSession`, and needs a real VLC install to verify against
- [ ] `SystemHelper` — the PowerShell/COM volume-brightness-sleep helper process; JSON-line stdin/stdout
      protocol, needs designing against `app`'s process-management approach
- [ ] `Tailscale`'s process methods (`status`/`up`/`down`/`setExitNode`/`startLogin`/`cancelLogin`) —
      subprocess-with-timeout and streaming-with-callback; same category as the above
- [ ] `remote.js`'s actual `connect`/`connect_sftp`/`connect_ftp` — needs an SSH/FTP crate decision (a
      pure-Rust SSH crate like `russh` is tokio-based, which has knock-on effects for whether `app` adopts
      an async runtime at all)
- [ ] `src/apps.js` — Start-menu app scanning, known-folder GUID resolution, Store package logos; entirely
      unlooked-at so far

### Electron-API replacements needed in `app`

None of these exist yet in the Tauri shell:

- [ ] Native file/folder picker (`dialog.showOpenDialog` equivalent) — needed by `add-game`, `pick-folder`,
      `pick-vlc`, `pick-key-file`
- [ ] "Reveal in file manager" (`shell.showItemInFolder`/`shell.openPath`) — needed by `show-in-folder`,
      `show-game-folder`
- [ ] "Open URL in default browser" (`shell.openExternal`) — needed by `open-external`, and internally
      wherever `win.webContents.setWindowOpenHandler` currently intercepts a link
- [ ] Encrypted secret storage (`safeStorage`, DPAPI-backed on Windows) — needed for saved server
      passwords; `sealSecret`/`openSecret` in `main.js` is the exact shape to match
- [ ] `powerSaveBlocker` equivalent — keep the display awake during playback
- [ ] `app.getFileIcon` equivalent — game/app icons extracted from an `.exe`
- [ ] WebHID device-permission handling for the Legion Go's controllers (`setDevicePermissionHandler`,
      `select-hid-device`) — a Tauri/wry-level capability, may need investigating what's exposed there
- [ ] Single-instance locking (`app.requestSingleInstanceLock` equivalent) — Tauri has a
      single-instance plugin; needs adopting and wiring to the same "focus/resume the existing window"
      behaviour as `app.on('second-instance', ...)`
- [ ] "Launch at login" (`app.setLoginItemSettings` equivalent)
- [ ] The rest of window setup: prevent navigation away from the app, intercept `target=_blank` links to
      open externally instead, restore fullscreen/size preferences, the icon/title/background-color
      already set in `tauri.conf.json` should carry over but needs checking once there's more to show

### Windows-specific things that can only be verified on Windows

- [ ] Everything already flagged `cfg(windows)`-only across `lounge-core` (steam.rs's registry lookup and
      `launch_quietly`, vlc.rs's registry lookup, system.rs's entire Wi-Fi/power/priority/battery surface,
      updater.rs's PowerShell script generation and WMI launch) — all type-check and clippy-clean cross-
      compiled for `x86_64-pc-windows-gnu`, none have actually **run**
- [ ] `app/src/lib.rs`'s `file_url` — only handles Unix-style paths so far; needs backslash-to-forward-
      slash conversion and the `file:///C:/...` drive-letter form for real Windows paths
- [ ] `get_state`'s `platform` field — currently reports `std::env::consts::OS` (would say `"linux"` on
      this machine); needs to actually report `"win32"` the way the renderer expects, or the renderer's
      several `platform === 'win32'` gates need re-checking against whatever this reports
- [ ] WebView2 itself — confirm it's present/installable the way `tauri-conf.json`/the installer expects
      on a clean Windows machine, since Electron currently bundles its own runtime and this won't

### Build & release pipeline

- [ ] `scripts/build-fse.ps1` reworked for whatever `cargo`/Tauri's bundler produces instead of
      `electron-builder`'s output layout
- [ ] `.github/workflows/build.yml` reworked: Rust toolchain setup instead of Node, `cargo build`/Tauri's
      bundler instead of `electron-builder`, and re-checking every step that currently assumes an
      `electron-builder`-shaped `dist/` (the NSIS/zip/FSE artifact naming `updater.rs` and
      `ASSET_PATTERNS` depend on)
- [ ] Decide how `lounge-core`'s 71 `#[test]`s and any future `app`-level tests fit into that CI run
      alongside (or instead of) `npm test`

### Testing

- [ ] Retire or keep `test/*.test.js` running against `src/*.js` for as long as Electron ships alongside
      Tauri; decide what happens to them at cutover (delete with `src/*.js`, presumably, once
      `lounge-core`'s coverage is confirmed to be a superset)
- [ ] Some kind of test coverage for `app`'s own command-wiring code (`app/src/lib.rs` currently has none
      beyond "it compiles and I ran it once by hand" — worth at least a few `#[test]`s once there's more
      logic in there than thin wiring)

## Ground rule

No version bump, no GitHub Release tag, and **no deletion of Electron/`main.js`/`preload.js`** happens
until every box above is checked, the result has actually run on a Windows handheld side by side with the
Electron build, and playback/game-launching/transfers/updates have all been exercised for real — not just
"the window opens." The tag this eventually produces is what the app's own auto-updater offers to real
users; shipping this before then would replace a working launcher with one that can't play media or
launch games.
