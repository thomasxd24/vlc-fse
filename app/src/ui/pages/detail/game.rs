//! A game's page (VIEWS.game).

use super::{action, backdrop, current_id, first_of, game_meta_line, more, strings};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::t;
use crate::ui::model::{self, arr, b, s};
use crate::ui::actions;
use crate::GameDetail;
use serde_json::Value;
use slint::{ComponentHandle, SharedString};

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let g = ui.global::<GameDetail>();
    g.on_act(|value| {
        let Some(id) = current_id("game") else { return };
        let Some(game) = cx().find("games", &id) else { return };
        match value.as_str() {
            "play" => actions::play_game(&id),
            "fav" => actions::set_pref(&id, "favorites", !b(&game, "favorite")),
            "edit" => actions::edit_game(&id),
            _ => actions::options("game", &id, ""),
        }
    });
    g.on_options(|| {
        if let Some(id) = current_id("game") {
            actions::options("game", &id, "");
        }
    });
    g.on_open_shot(|i| {
        if let Some(id) = current_id("game") {
            actions::show_screenshot(&id, i.max(0) as usize);
        }
    });
    ctx.on_event("route", |_, r| {
        if r.get("name").and_then(Value::as_str) != Some("game") {
            return;
        }
        if r.get("forward").and_then(Value::as_bool).unwrap_or(false) {
            // A new page: Play has the focus (data-autofocus).
            let ui = cx().ui();
            let g = ui.global::<GameDetail>();
            g.set_zone(0);
            g.set_action(0);
            g.set_shot(0);
        }
        rebuild(true);
    });
    ctx.on_event("state", |_, _| rebuild(false));
}

fn rebuild(entering: bool) {
    let Some(id) = current_id("game") else { return };
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<GameDetail>();
    let Some(g) = ctx.find("games", &id) else {
        page.set_found(false);
        return;
    };
    if entering {
        backdrop(&first_of(&g, &["hero", "header", "poster"]));
    }
    let devs = strings(&g, "developers");
    let pubs: Vec<String> = strings(&g, "publishers").into_iter().filter(|p| !devs.contains(p)).collect();
    let credits = [devs.join(", "), pubs.join(", ")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let genres = [strings(&g, "genres").join(" · "), credits].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("  —  ");
    let fav = b(&g, "favorite");
    let actions = vec![
        action(&t("game.play"), "play", "play", true),
        action(&t(if fav { "opt.unfavorite" } else { "opt.favorite" }), if fav { "star-on" } else { "star" }, "fav", false),
        action(&t("game.edit"), "edit", "edit", false),
        more(),
    ];
    let shots: Vec<SharedString> = arr(&g, "screenshots").iter().map(|x| s(x, "thumb").into()).collect();
    page.set_found(true);
    page.set_title(s(&g, "title").into());
    page.set_poster(s(&g, "poster").into());
    page.set_icon(s(&g, "icon").into());
    page.set_logo(s(&g, "logo").into());
    page.set_tint(fmt::tint_of(s(&g, "title")));
    page.set_meta(model::model(game_meta_line(&g)));
    page.set_overview(s(&g, "overview").into());
    page.set_genres(genres.into());
    page.set_actions(model::model(actions));
    page.set_shots(model::model(shots));
}
