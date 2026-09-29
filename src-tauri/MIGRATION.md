# Electron → Tauri migration

Status: **the rewrite is complete.** Every `src/*.js` module is ported to `lounge-core` (134 tests,
`cargo clippy -D warnings` clean on Linux and cross-checked for `x86_64-pc-windows-gnu`), and the
`app` crate is a full 1:1 port of `main.js`'s orchestration: the same commands, the same payloads,
the same flows. `renderer/**` runs unmodified against either shell — `window.lounge` is recreated by
an initialization script, so not one line of UI code changed hands (one deliberate exception, below).

What is *not* done is Windows verification: this development machine is Linux. Everything below was
built, tested and (where noted) actually run here; the `cfg(windows)`-only surfaces compile and
lint-clean for the Windows target but have never **run** on one. That's what the 3.0.0-alpha release
is for: it ships the Tauri build for real Windows handhelds to exercise, side by side with the
working 2.2.0 Electron build, before any cutover.

## Layout

```
src-tauri/
  Cargo.toml          workspace root
  lounge-core/         <- ported business logic; no Tauri/WebView dependency (134 tests)
  app/                 <- the Tauri binary crate: window, commands, the full shell
```

## What each side owns

- **`lounge-core`** — `vdf`, `parse`, `store`, `migrate`, `steam`, `games` (helpers + `GameSession`),
  `library` + `locale`, `vlc` (locate/args + `VlcSession`), `metadata`, `gameinfo`, `system`
  (wifi/power/priority/battery + `SystemHelper`), `tailscale` (locate/parse + the CLI driver),
  `transfer_plan`, `transfers` (`TransferQueue`), `remote` (sort/walk + the real SFTP/FTP/FTPS
  clients over `ssh2`/`suppaftp`), `updater`, `apps`, and `viewmodel` — the view-model half of
  `main.js` (progress, prefs, languages, overrides, continue watching, next up, stats), which
  `main.js` had no tests for and `viewmodel.rs` carries 19 of them.
- **`app`** — everything `main.js` did around those modules: library/Steam scanning off the main
  thread, TMDB + game enrichment on background threads with status events, VLC playback (resume
  positions, per-item languages, watch-time accounting, the "previous item finished" rule), game
  launching (suspend/resume, stub launchers, Steam's `RunningAppID`, quiet launches), servers with
  DPAPI-sealed secrets and trust-on-first-use host keys, remote browsing/planning/downloads through
  the `TransferQueue`, Start-menu apps with Store logos and `ExtractAssociatedIcon` PNG icons,
  Tailscale with the sign-in QR, the PowerShell system helper behind the quick menu, the self-updater
  (auto-check cadence, NSIS/zip/FSE install flows), power-save blocking during playback,
  single-instance focus, and every `save-settings` side effect.

The Electron stack (`main.js`, `preload.js`, `src/*.js`, `test/*.test.js`,
`electron-builder`) is still in the tree and still green — it is the reference the port was verified
against, and it stays until the Windows side-by-side pass proves the Tauri build equivalent. At
cutover it gets deleted, along with this file.

## Known gaps, honestly

- **WebHID device permissions** (the Legion Go controllers' battery/attach toasts): Electron granted
  the controllers' HID interface silently (`setDevicePermissionHandler`); wry/WebView2 has no
  equivalent. `renderer/core.js` therefore only starts its HID listener under the Electron shell
  (it checks `window.lounge.kind`). Input itself uses the Gamepad API and works identically.
- **While a game runs** the page is blanked and the window minimised; Electron additionally unloaded
  the document itself, so peak renderer memory is somewhat higher under Tauri.
- **The system locale** (`uiLanguage: 'auto'`) reads `LC_ALL`/`LANG`/`LOUNGE_LOCALE`; on Windows
  these are usually unset, so auto resolves to English unless the env var is set. A
  `GetUserDefaultLocaleName` call is the proper fix on Windows.
- **Windows-only surfaces never ran**: Steam's registry lookup and quiet launches, Wi-Fi/power/
  priority/battery, the PowerShell helper, DPAPI sealing, `ExtractAssociatedIcon`, the FSE install
  flow (WMI launch + status file), WebView2 itself. All compile and lint for the Windows target;
  the alpha exists to exercise them.
- **`apps.rs`'s PowerShell listing** (`Get-StartApps` + `Get-AppxPackage`) is ported verbatim but,
  like the rest of this list, only verified to compile here.

## Build & release

`.github/workflows/build.yml` now builds the Tauri app on `windows-latest` (Node for the JS suite
against `src/*.js`, which still passes; Rust for `lounge-core`'s tests, clippy on both crates, and
`npx tauri-apps/cli build`). The output is staged into exactly the shapes the updater expects:
`Lounge-<v>-win-x64.zip` (a single self-contained `Lounge.exe` — the renderer is embedded and the
WebView2 loader is statically linked), `Lounge-Setup-<v>.exe` (Tauri's NSIS bundle, `installMode:
both` so silent installs stay per-machine like electron-builder's), and `Lounge-FSE-<v>.zip`
(`scripts/build-fse.ps1` unchanged, fed a `win-unpacked` holding the Tauri binary). Pre-release tags
(`v3.0.0-alpha`) produce pre-release GitHub Releases, which GitHub's `/releases/latest` excludes —
so the update channel never offers an alpha to users; they fetch it deliberately.

## Ground rule (unchanged in spirit)

No deletion of the Electron stack and no *stable* 3.0.0 happens until the Tauri build has actually
run on a Windows handheld side by side with the Electron build and playback, game-launching,
transfers and updates have all been exercised for real. The alpha tag is that test's starting gun,
not its finish line.
