# Lounge's Slint UI — how it's put together

Lounge 3 draws its interface natively with [Slint](https://slint.dev) (Skia renderer) instead of a
webview. The screens are ports of the web renderer that shipped with 2.x and the Tauri alphas
(`renderer/*.js`, `renderer/styles.css` on branch `tauri-migration`): same layout, same strings, same
controller behaviour.

```
lounge-core/          business logic (library, Steam, VLC, transfers, updater…) — no UI
app/src/backend/      application state + every command; emits events through a Host
app/src/ui/           the Slint side in Rust: context, router, dialogs, artwork, input, screens
app/ui/               the .slint files
app/i18n/{en,fr}.json UI strings (from the renderer's i18n.js)
app/src/demo.rs       a made-up library so every screen can be run and photographed anywhere
```

## Data flow

1. The backend emits events (`state`, `now-playing`, `game`, `toast`, `update`, `transfers`,
   `legion-report`, `legion-state`) — the same names and JSON payloads the web renderer consumed.
2. `ui::ctx` delivers them on the event loop; screens subscribe with `ctx.on_event("state", |ctx, st| …)`.
   The latest state is in `ctx.state`; `ctx.library("games")`, `ctx.find("movies", id)`, `ctx.setting(k)`.
3. A screen turns JSON into plain Slint structs/models and sets them on its **global**. Slint never
   reads JSON.
4. Slint calls back through the global's callbacks; Rust runs backend commands with
   `ctx.call("what", |b| b.play(req), |result| …)` (worker thread → result on the event loop) or
   `ctx.send("what", |b| b.stop())`. In demo mode backend calls are skipped (logged).

Get the context anywhere on the event loop with `cx()` (`use crate::ui::ctx::cx`). Screen `install`
functions receive `ctx: &Ctx`; inside callbacks call `cx()` again.

Rust gotcha: a Slint global borrows the window handle, so bind the window first:
`let ui = cx().ui(); let nav = ui.global::<Nav>();` (not `cx().ui().global::<Nav>()` in a `let`).

## Screens

Each screen is a pair: `app/ui/pages/<name>.slint` (exports a global `XxxPage`/`XxxDetail` with its
data, focus state and callbacks, and a component `XxxView`) and `app/src/ui/pages/<name>.rs` (an
`install(ctx)` that wires the callbacks and subscribes to events). Overlays live in `app/ui/overlays/`
and `app/src/ui/overlays/`. `app/ui/app.slint` instantiates the view for `Nav.route.name` and
re-exports every global. **`pages/grid.slint` + `pages/grid.rs` are the reference implementation**:
read them before writing a screen.

State that must survive navigating away (focus index, scroll, filters) lives in the global, not the
component — components are destroyed when you leave the page.

Routes: `router::go(name, id)` pushes, `router::switch_tab(name)` replaces, `router::back()`. Subscribe
to `ctx.on_event("route", |ctx, r| …)` (`{ name, id, forward }`) to load a page's data when it opens.

## Focus and keys

Screens own their focus as indices in their global and pass `focused: …` to widgets — widgets never
hold keyboard focus. Each screen has one `PageScope` (a FocusScope that grabs focus when the page opens
and on `Nav.focus-page()`) whose `key-pressed` handles:

| key               | from the pad | do                                      |
|-------------------|--------------|-----------------------------------------|
| arrows            | D-pad/stick  | move focus; Up from the top row → `Nav.focus-top()` |
| Return / Space    | A            | activate                                |
| Menu              | X            | options for the focused item            |
| PageUp / PageDown | LT / RT      | jump a page                              |

Return `reject` for anything else: the window handles Escape/Backspace (Back), Tab/Backtab (LB/RB),
F1 (☰ quick menu — `ctx.on_event("key:menu", …)`), F2 (View — `key:view`), F3 (Y, search), F11.
Overlays and dialogs take focus with their own FocusScope and `accept` everything while open. Call
`Input.feedback("move" | "select" | "back" | "edge" | "open")` on navigation for sounds and rumble.

Pointer: widgets use `Pointer` (widgets.slint): hover moves focus in mouse mode (`hovered`), click
activates, right-click / touch long-press is `options`. Horizontal and vertical lists are Flickables
(swipeable), scrolled by the screen to keep the focused item in view.

Set the hint bar with `Hints.items = [{ button: "a", label: T.tr("hint.select") }, …]` when the page
opens (and when context changes). Glyphs switch between pad buttons and key caps automatically.

## Strings

`T.tr("key")`, `T.trv("key", ["name", value, "n", count])` in Slint; `i18n::t("key")`,
`i18n::tv("key", &[("n", 3.into())])` in Rust. Keys are the renderer's (`app/i18n/en.json`). Add new
strings to both en.json and fr.json. Numbers and relative times: `ui::fmt` (`ago`, `playtime`,
`runtime`, `remaining`, `ep_code`, `size`, `clock`, `long_date`…).

## Artwork

`Artwork { path: …; }` (placeholder, shimmer, fade-in) or `Image { source: Art.get(path, self.width); }`.
Paths are plain filesystem paths from the state payload. Decoding and downscaling happen off the
event loop (`ui::art`).

## Dialogs and toasts

`dialogs::choose(title, text, vec![choice(label, value), …], |answer: Option<String>| …)` for menus
and confirmations (an answer may open the next dialog directly — focus stays with it) (`dialogs::open(ChooseSpec{…})` for wide/art/initial focus), `dialogs::prompt(Prompt{…},
|text| …)` for text entry with the on-screen keyboard, `toasts::toast(text, "info"|"error"|"success")`.
Screens with a custom modal (edit a game, pick artwork…) draw it inside `DialogFrame` (shell.slint)
with their own FocusScope, and register `ui::on_close_overlay(|| …)` so Back closes it.

## Motion

Durations come from `Motion` (`fast`, `normal`, `slow`, `fade`) and collapse to zero when Settings ›
Animations is Reduced; easing is `Motion.ease`. Focus zoom is `Motion.card-zoom` / `btn-zoom`.

## Running and checking

```sh
~/.cargo/bin/slint-compiler --style fluent-dark app/ui/app.slint > /dev/null   # .slint check, ~0.2 s
cargo build -p lounge                     # full build
cargo run -p lounge -- --demo             # the demo library in a window
scripts/snap.sh "go games; key Right; key Menu; shot snaps/menu.png"   # headless screenshots (Xvfb)
scripts/snap.sh "go home; shot snaps/home-fr.png" --lang fr --resume show
scripts/snap.sh "wait 600; shot /tmp/intro.png" --intro   # scripted runs skip the boot intro unless asked
```

Item actions shared by every screen (play, the options menu, languages, editing games) are in
`ui::actions` (+ `actions/edit.rs`); a screen calls `actions::options(kind, id, show_id)` rather than
building its own menu. Sounds and rumble (`overlays/feedback.rs`, cpal + gilrs) are Windows-only, like
controller input.

Script steps: `go NAME [ID]`, `key Up|Down|Left|Right|Return|Escape|Menu|Tab|Backtab|PageUp|PageDown|F1|F2|F3`,
`type TEXT`, `mode pad|keyboard|mouse|touch`, `event NAME JSON|demo`, `wait MS`, `shot PATH`, `quit`.

## Slint limitations met so far (and what we do instead)

- No flex-wrap: toolbars scroll sideways (`ChipBar`); grids compute positions from a column count.
- No backdrop blur: modals use a dark scrim.
- No string slicing: `Util.drop-last`, `Util.mask` (Rust-backed pure callbacks) — add more there.
- Every element has a `focus()` function, so a property can't be called `focus`.
- `Flickable.viewport-*` is deprecated: use `content-x/y/width/height`.
- `changed prop => { … }` handlers are the way to react to a property changing (e.g. scroll to focus).
- Globals can't convert logical ↔ physical pixels; `Art.get` takes logical px.
