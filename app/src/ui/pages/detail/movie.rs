//! A film's page (VIEWS.movie).

use super::{action, backdrop, current_id, first_of, meta_line, more, strings};
use crate::ui::actions;
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, b, s};
use crate::MovieDetail;
use serde_json::{json, Value};
use slint::ComponentHandle;

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let m = ui.global::<MovieDetail>();
    m.on_act(|value| {
        let Some(id) = current_id("movie") else { return };
        let Some(movie) = cx().find("movies", &id) else { return };
        match value.as_str() {
            "resume" => actions::play_movie(&id, "resume"),
            "start" => actions::play_movie(&id, "start"),
            "watched" => {
                let watched = movie.pointer("/progress/watched").and_then(Value::as_bool).unwrap_or(false);
                actions::set_watched(json!({ "kind": "movie", "id": id, "watched": !watched }));
            }
            _ => actions::options("movie", &id, ""),
        }
    });
    m.on_options(|| {
        if let Some(id) = current_id("movie") {
            actions::options("movie", &id, "");
        }
    });
    ctx.on_event("route", |_, r| {
        if r.get("name").and_then(Value::as_str) != Some("movie") {
            return;
        }
        if r.get("forward").and_then(Value::as_bool).unwrap_or(false) {
            let ui = cx().ui();
            ui.global::<MovieDetail>().set_focus(0);
        }
        rebuild(true);
    });
    ctx.on_event("state", |_, _| rebuild(false));
}

fn rebuild(entering: bool) {
    let Some(id) = current_id("movie") else { return };
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<MovieDetail>();
    let Some(m) = ctx.find("movies", &id) else {
        page.set_found(false);
        return;
    };
    if entering {
        backdrop(&first_of(&m, &["backdrop", "poster"]));
    }
    let pr = m.get("progress").cloned().unwrap_or(Value::Null);
    let resumable = b(&pr, "resumable");
    let watched = b(&pr, "watched");
    let time = pr.get("time").and_then(Value::as_f64).unwrap_or(0.0);
    let length = pr.get("length").and_then(Value::as_f64).unwrap_or(0.0);
    let mut actions = if resumable {
        vec![
            action(&tv("media.resumeFrom", &[("time", fmt::time(time).into())]), "play", "resume", true),
            action(&t("media.playFromStart"), "restart", "start", false),
        ]
    } else {
        vec![action(&t("media.play"), "play", "start", true)]
    };
    actions.push(action(&t(if watched { "opt.markUnwatched" } else { "opt.markWatched" }), "check", "watched", false));
    actions.push(more());
    page.set_found(true);
    page.set_title(s(&m, "title").into());
    page.set_poster(s(&m, "poster").into());
    page.set_tint(fmt::tint_of(s(&m, "title")));
    page.set_meta(model::model(meta_line(&m)));
    page.set_tagline(s(&m, "tagline").into());
    page.set_overview(s(&m, "overview").into());
    page.set_genres(strings(&m, "genres").join(" · ").into());
    page.set_progress(if resumable { fmt::pct(&pr) } else { -1.0 });
    page.set_progress_text(tv("media.timeOf", &[("time", fmt::time(time).into()), ("total", fmt::time(length).into())]).into());
    page.set_actions(model::model(actions));
    page.set_path(s(&m, "path").into());
}
