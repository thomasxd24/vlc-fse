//! The running-game layer — the renderer's `renderGameLayer`, driven by the backend's `game` event
//! (`{ id, title, phase: launching | running | untracked }`, null when the game is over). "Back to the
//! game" is `back_to_game` (the UI unloads), "I'm done playing" / "Cancel" is `end_game`.

use super::quickmenu::{layer_closed, layer_opened};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::model::s;
use crate::GameLayer;
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::Cell;

thread_local! {
    static SEEN_STATE: Cell<bool> = const { Cell::new(false) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let gl = ui.global::<GameLayer>();
    gl.on_back_to_game(|| cx().send("back_to_game", |b| b.back_to_game()));
    gl.on_done(|| cx().send("end_game", |b| b.end_game()));
    ctx.on_event("game", |_, g| apply(g));
    // A game already running when the UI starts: the first state carries it.
    ctx.on_event("state", |_, st| {
        if !SEEN_STATE.with(|c| c.replace(true)) {
            if let Some(g) = st.get("game").filter(|g| !g.is_null()) {
                apply(g);
            }
        }
    });
    // Back does nothing while a game runs.
    crate::ui::on_close_overlay(|| cx().ui().global::<GameLayer>().get_open());
}

fn apply(g: &Value) {
    let ctx = cx();
    let ui = ctx.ui();
    let gl = ui.global::<GameLayer>();
    if g.is_null() {
        if gl.get_open() {
            gl.set_open(false);
            layer_closed();
        }
        return;
    }
    let game = ctx.find("games", s(g, "id"));
    let pick = |keys: &[&str]| -> String {
        game.as_ref()
            .and_then(|x| keys.iter().map(|k| s(x, k)).find(|v| !v.is_empty()))
            .unwrap_or("")
            .to_string()
    };
    gl.set_title(s(g, "title").into());
    gl.set_art(pick(&["hero", "header", "poster"]).into());
    gl.set_logo(pick(&["logo"]).into());
    gl.set_phase(s(g, "phase").into());
    // Every update lands on the first button again (the renderer re-renders and refocuses).
    gl.set_index(0);
    gl.set_focus_rev(gl.get_focus_rev() + 1);
    if !gl.get_open() {
        gl.set_open(true);
        layer_opened();
    }
}
