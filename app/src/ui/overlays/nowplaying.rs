//! "Playing in VLC" — the renderer's `renderNowPlaying`, driven by the backend's `now-playing` event
//! (null when playback ends). The controls map to `np_command` (pause, seek back / forward, next,
//! audio / subtitle track) and `stop`.

use super::quickmenu::{layer_closed, layer_opened};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{b, n, s};
use crate::NowPlaying;
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
        if name == "stop" {
            ctx.send("stop", |b| b.stop());
        } else {
            ctx.send("np_command", move |b| b.np_command(&name));
        }
    });
    ctx.on_event("now-playing", |_, v| apply(v));
    // Already playing when the UI starts (or is rebuilt): the first state carries it.
    ctx.on_event("state", |_, st| {
        if !SEEN_STATE.with(|c| c.replace(true)) {
            if let Some(v) = st.get("nowPlaying").filter(|v| !v.is_null()) {
                apply(v);
            }
        }
    });
    // Back does nothing while VLC plays.
    crate::ui::on_close_overlay(|| cx().ui().global::<NowPlaying>().get_open());
}

fn apply(v: &Value) {
    let ui = cx().ui();
    let np = ui.global::<NowPlaying>();
    if v.is_null() {
        if np.get_open() {
            np.set_open(false);
            layer_closed();
        }
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
    if !np.get_open() {
        // Focus starts on play / pause.
        np.set_row(0);
        np.set_index(1);
        np.set_open(true);
        layer_opened();
    }
}
