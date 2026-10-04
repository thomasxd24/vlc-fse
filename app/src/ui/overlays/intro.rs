//! The startup intro — the renderer's `#splash`, `endIntro` / `INTRO_MS` (app.js) and boot.js.
//!
//! The splash covers the window from the first frame; the animation starts a moment after the window is
//! on screen, runs at least [`INTRO_MS`], then the splash lifts (0.6 s) and goes. A soft two-note chime
//! plays under it (the `boot` feedback). With Settings › Animations › Reduced it's a plain fade.
//!
//! Not played when coming back from a game (`uiState.stack` in the first state), nor in scripted runs
//! (`--script`, so headless snapshots aren't blocked) unless `--intro` asks for it.

use super::super::ctx::Ctx;
use super::super::input;
use crate::Intro;
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::Cell;
use std::time::Duration;

/// How long the intro runs before lifting (from when it starts playing).
pub const INTRO_MS: u64 = 2000;
/// The renderer waits a frame plus 250 ms after the window becomes visible.
const START_DELAY_MS: u64 = 250;
/// The lift-off fade (0.6 s) plus a little.
const OUT_MS: u64 = 700;

thread_local! {
    static FIRST_STATE: Cell<bool> = const { Cell::new(true) };
}

pub fn install(ctx: &Ctx) {
    let args: Vec<String> = std::env::args().collect();
    let scripted = args.iter().any(|a| a == "--script");
    let forced = args.iter().any(|a| a == "--intro");
    let ui = ctx.ui();
    let intro = ui.global::<Intro>();
    if scripted && !forced {
        intro.set_open(false);
        return;
    }
    intro.set_open(true);

    // The first state (dispatched right after the overlays install) says how to play it, or not at all.
    ctx.on_event("state", |ctx, st| {
        if !FIRST_STATE.replace(false) {
            return;
        }
        let ui = ctx.ui();
        let intro = ui.global::<Intro>();
        let resuming = st.pointer("/uiState/stack").and_then(Value::as_array).is_some_and(|s| !s.is_empty());
        if resuming {
            intro.set_open(false);
            return;
        }
        intro.set_calm(st.pointer("/settings/animations").and_then(Value::as_str) == Some("reduced"));
    });

    let weak = ctx.ui.clone();
    slint::Timer::single_shot(Duration::from_millis(START_DELAY_MS), move || {
        let Some(ui) = weak.upgrade() else { return };
        let intro = ui.global::<Intro>();
        if !intro.get_open() {
            return;
        }
        intro.set_playing(true);
        input::feedback("boot");
        let weak = weak.clone();
        slint::Timer::single_shot(Duration::from_millis(INTRO_MS), move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.global::<Intro>().set_leaving(true);
            let weak = weak.clone();
            slint::Timer::single_shot(Duration::from_millis(OUT_MS), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<Intro>().set_open(false);
                }
            });
        });
    });
}
