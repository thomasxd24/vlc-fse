//! The game, movie and show pages (the renderer's VIEWS.game / movie / show) — one submodule each —
//! plus what they share: the `.meta` line (metaLine / gameMetaLine), the action buttons, and loading a
//! page's data when it's navigated to or when the state changes under it.

mod game;
mod movie;
mod show;

use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{arr, b, n, s};
use crate::ui::{actions, router};
use crate::{Backdrop, DetailAction, MetaBit};
use serde_json::Value;
use slint::ComponentHandle;

pub use show::open_show;

pub fn install(ctx: &Ctx) {
    actions::install();
    game::install(ctx);
    movie::install(ctx);
    show::install(ctx);
}

/// The route's id when the current page is `name`.
fn current_id(name: &str) -> Option<String> {
    let (n, id) = router::current();
    (n == name).then_some(id)
}

/// Show the page's artwork behind it right away (the renderer's `Backdrop.set(…, { immediate })`).
fn backdrop(path: &str) {
    let ui = cx().ui();
    let bd = ui.global::<Backdrop>();
    bd.set_dim(false);
    bd.set_src(path.into());
}

fn first_of(item: &Value, keys: &[&str]) -> String {
    keys.iter().map(|k| s(item, k)).find(|x| !x.is_empty()).unwrap_or("").to_string()
}

fn bit(text: impl Into<String>, kind: &str, icon: &str) -> MetaBit {
    MetaBit { text: text.into().into(), kind: kind.into(), icon: icon.into() }
}

fn action(label: &str, icon: &str, value: &str, primary: bool) -> DetailAction {
    DetailAction { label: label.into(), icon: icon.into(), value: value.into(), primary }
}

/// The ⋯ button (icon only).
fn more() -> DetailAction {
    action("", "more", "options", false)
}

/// A number as JS prints it: `7.8`, `2021`.
fn num_text(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// `metaLine(item)` for a movie or a show.
fn meta_line(item: &Value) -> Vec<MetaBit> {
    let kind = s(item, "type");
    let mut bits = Vec::new();
    if n(item, "year") > 0.0 {
        bits.push(bit(num_text(n(item, "year")), "", ""));
    }
    if kind == "movie" && n(item, "runtime") > 0.0 {
        bits.push(bit(fmt::runtime(n(item, "runtime") as i64), "", ""));
    }
    if kind == "show" {
        let seasons = tv("n.seasons", &[("n", arr(item, "seasons").len().into())]);
        let eps = tv("n.episodes", &[("n", arr(item, "episodes").len().into())]);
        bits.push(bit(format!("{seasons} · {eps}"), "", ""));
    }
    if n(item, "rating") > 0.0 {
        bits.push(bit(num_text(n(item, "rating")), "rating", ""));
    }
    if kind == "show" && !s(item, "status").is_empty() {
        bits.push(bit(s(item, "status"), "pill", ""));
    }
    if kind == "movie" && item.pointer("/progress/watched").and_then(Value::as_bool).unwrap_or(false) {
        bits.push(bit(t("media.watched"), "pill", ""));
    }
    if b(item, "favorite") {
        bits.push(bit(t("opt.favorite"), "fav", "star-on"));
    }
    bits
}

/// `gameMetaLine(g)`.
fn game_meta_line(g: &Value) -> Vec<MetaBit> {
    let mut bits = Vec::new();
    if n(g, "playtime") > 0.0 {
        bits.push(bit(fmt::playtime(n(g, "playtime") as i64), "", ""));
    }
    if n(g, "lastPlayed") > 0.0 {
        bits.push(bit(tv("game.lastPlayed", &[("when", fmt::ago(n(g, "lastPlayed") as i64).into())]), "", ""));
    }
    if !s(g, "releaseDate").is_empty() {
        bits.push(bit(s(g, "releaseDate"), "", ""));
    }
    if n(g, "metacritic") > 0.0 {
        bits.push(bit(num_text(n(g, "metacritic")), "score", ""));
    }
    if s(g, "controller") == "full" {
        bits.push(bit(t("game.controllerFull"), "pill", "gamepad"));
    }
    if s(g, "source") == "steam" {
        bits.push(bit("Steam", "pill", "steam"));
    } else {
        bits.push(bit(t("game.manual"), "pill", ""));
    }
    if b(g, "favorite") {
        bits.push(bit(t("opt.favorite"), "fav", "star-on"));
    }
    bits
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    arr(v, key).iter().filter_map(Value::as_str).map(String::from).collect()
}
