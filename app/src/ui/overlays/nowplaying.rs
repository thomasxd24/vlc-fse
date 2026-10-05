//! "Playing in VLC" — the renderer's `renderNowPlaying`, driven by the backend's `now-playing` event
//! (null when playback ends). The controls map to `np_command` (pause, seek back / forward, next,
//! audio / subtitle track) and `stop`. Back / "Back to Lounge" hides it so Lounge can be used while VLC
//! plays; [`reopen`] (the top bar's Now playing button, the quick menu) shows it again.

use super::quickmenu::{layer_closed, layer_opened};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{b, n, s};
use crate::{NowPlaying, Sys};
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::Cell;

thread_local! {
    static SEEN_STATE: Cell<bool> = const { Cell::new(false) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let np = ui.global::<NowPlaying>();
    np.on_command(|name| {
        let ctx = cx();
        if name == "hide" {
            hide();
        } else if name == "stop" {
            ctx.send("stop", |b| b.stop());
        } else {
            ctx.send("np_command", move |b| b.np_command(&name));
        }
    });
    np.on_reopen(reopen);
    ui.global::<Sys>().on_now_playing_clicked(reopen);
    ctx.on_event("now-playing", |_, v| apply(v));
    // Already playing when the UI starts (or is rebuilt): the first state carries it.
    ctx.on_event("state", |_, st| {
        if !SEEN_STATE.with(|c| c.replace(true)) {
            if let Some(v) = st.get("nowPlaying").filter(|v| !v.is_null()) {
                apply(v);
            }
        }
    });
    // Back hides the layer; VLC keeps playing.
    crate::ui::on_close_overlay(|| {
        if cx().ui().global::<NowPlaying>().get_open() {
            hide();
            true
        } else {
            false
        }
    });
}

pub fn is_active() -> bool {
    cx().ui().global::<NowPlaying>().get_active()
}

fn hide() {
    let ui = cx().ui();
    let np = ui.global::<NowPlaying>();
    if np.get_open() {
        np.set_open(false);
        layer_closed();
    }
    sync_top_bar();
}

/// The top bar's Now playing button: there while VLC plays with this layer hidden.
fn sync_top_bar() {
    let ui = cx().ui();
    let np = ui.global::<NowPlaying>();
    let title = if np.get_active() && !np.get_open() { np.get_title() } else { Default::default() };
    ui.global::<Sys>().set_now_playing(title);
}

/// Show the layer again while VLC plays.
pub fn reopen() {
    let ui = cx().ui();
    let np = ui.global::<NowPlaying>();
    if np.get_active() && !np.get_open() {
        np.set_row(0);
        np.set_index(1);
        np.set_open(true);
        layer_opened();
    }
    sync_top_bar();
}

fn apply(v: &Value) {
    let ui = cx().ui();
    let np = ui.global::<NowPlaying>();
    if v.is_null() {
        np.set_active(false);
        hide();
        return;
    }
    let file = s(v, "current").rsplit(['/', '\\']).next().unwrap_or("").to_string();
    let queue = n(v, "queueSize") as i64;
    let index = n(v, "index") as i64;
    let (time, length, paused) = (n(v, "time"), n(v, "length"), b(v, "paused"));
    np.set_title(s(v, "title").into());
    np.set_file(if queue > 1 { format!("{file} · {}", tv("np.queue", &[("n", (index + 1).into()), ("total", queue.into())])) } else { file }.into());
    np.set_progress(if length > 0.0 { (time / length) as f32 } else { 0.0 });
    np.set_time(
        if length > 0.0 {
            format!("{} / {}{}", fmt::time(time), fmt::time(length), if paused { format!(" · {}", t("np.paused")) } else { String::new() })
        } else {
            t("np.starting")
        }
        .into(),
    );
    np.set_paused(paused);
    np.set_has_next(queue > 1 && index < queue - 1);
    // Playback starting shows the layer; once hidden it stays hidden until reopened.
    if !np.get_active() {
        np.set_active(true);
        reopen();
    }
}
