//! Games / Movies / TV Shows grids (the renderer's VIEWS.games / movies / shows and gridPage).
//!
//! The pattern every screen follows: `install` wires the page global's callbacks and subscribes to
//! `state` (rebuild the page's data from the state payload) and, if needed, `route` (the page was
//! navigated to). Data goes to Slint as plain structs/models; nothing in Slint reads JSON.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, b, n, poster_card, s};
use crate::ui::{actions, prefs, router};
use crate::{Backdrop, GridChip, GridData, Grids};
use serde_json::Value;
use slint::ComponentHandle;

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let grids = ui.global::<Grids>();
    grids.on_chip(|page, value| chip(&page, &value));
    grids.on_open(|page, i| {
        if let Some(item) = items(&page).get(i as usize) {
            router::go(if page == "games" { "game" } else if page == "movies" { "movie" } else { "show" }, s(item, "id"));
        }
    });
    grids.on_options(|page, i| {
        if let Some(item) = items(&page).get(i as usize) {
            actions::options(s(item, "type"), s(item, "id"), "");
        }
    });
    grids.on_focused(|page, i| {
        if let Some(item) = items(&page).get(i as usize) {
            cx().ui().global::<Backdrop>().set_src(model::backdrop_of(item).into());
        }
    });
    ctx.on_event("state", |_, _| rebuild());
}

fn chip(page: &str, value: &str) {
    let (kind, v) = value.split_once(':').unwrap_or(("", value));
    let prefix = match page {
        "games" => "game",
        "movies" => "movie",
        _ => "show",
    };
    match kind {
        "sort" => prefs::set(&format!("{prefix}Sort"), v),
        "filter" => prefs::set(&format!("{prefix}Filter"), v),
        "act" if v == "add-game" => return actions::add_game(),
        "act" if v == "stats" => return router::go("stats", ""),
        _ => return,
    }
    rebuild();
}

/// The page's items, filtered and sorted as shown (so indices from Slint map straight back).
fn items(page: &str) -> Vec<Value> {
    let ctx = cx();
    let list = ctx.library(page);
    let (sort, filter) = match page {
        "games" => (prefs::get("gameSort"), prefs::get("gameFilter")),
        "movies" => (prefs::get("movieSort"), prefs::get("movieFilter")),
        _ => (prefs::get("showSort"), prefs::get("showFilter")),
    };
    let mut out: Vec<Value> = if filter == "hidden" {
        list.into_iter().filter(|x| b(x, "hidden")).collect()
    } else {
        list.into_iter()
            .filter(|x| !b(x, "hidden"))
            .filter(|x| match (page, filter.as_str()) {
                (_, "favorites") => b(x, "favorite"),
                ("games", "steam") => s(x, "source") == "steam",
                ("games", "other") => s(x, "source") != "steam",
                ("movies", "unwatched") => !x.pointer("/progress/watched").and_then(Value::as_bool).unwrap_or(false),
                ("movies", "progress") => x.pointer("/progress/resumable").and_then(Value::as_bool).unwrap_or(false),
                ("shows", "unwatched") => (n(x, "watchedCount") as usize) < model::arr(x, "episodes").len(),
                ("shows", "progress") => {
                    let w = n(x, "watchedCount") as usize;
                    w > 0 && w < model::arr(x, "episodes").len()
                }
                _ => true,
            })
            .collect()
    };
    sort_items(&mut out, &sort);
    out
}

pub fn sort_items(list: &mut [Value], sort: &str) {
    let by_title = |a: &Value, b: &Value| model::title_cmp(s(a, "title"), s(b, "title"));
    let desc = |key: &'static str| move |a: &Value, b: &Value| n(b, key).partial_cmp(&n(a, key)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| by_title(a, b));
    match sort {
        "added" => list.sort_by(|a, b| n(b, "addedAt").partial_cmp(&n(a, "addedAt")).unwrap_or(std::cmp::Ordering::Equal)),
        "year" => list.sort_by(desc("year")),
        "rating" => list.sort_by(desc("rating")),
        "recent" => list.sort_by(desc("lastPlayed")),
        "playtime" => list.sort_by(desc("playtime")),
        _ => list.sort_by(by_title),
    }
}

fn chips(group: &str, current: &str, options: &[(&str, &str)], first_sep: bool) -> Vec<GridChip> {
    options
        .iter()
        .enumerate()
        .map(|(i, (value, key))| GridChip {
            label: t(key).into(),
            value: format!("{group}:{value}").into(),
            on: *value == current,
            sep_before: first_sep && i == 0,
            ..Default::default()
        })
        .collect()
}

const MEDIA_SORTS: [(&str, &str); 4] = [("title", "sort.az"), ("added", "sort.added"), ("year", "sort.year"), ("rating", "sort.rating")];

fn rebuild() {
    let ctx = cx();
    let ui = ctx.ui();
    let grids = ui.global::<Grids>();
    for page in ["games", "movies", "shows"] {
        let all = ctx.library(page).len();
        let list = items(page);
        let mut chip_list = Vec::new();
        let (title, empty) = match page {
            "games" => {
                chip_list.push(GridChip { label: t("games.add").into(), icon: "plus".into(), value: "act:add-game".into(), accent: true, ..Default::default() });
                chip_list.push(GridChip { label: t("stats.title").into(), icon: "chart".into(), value: "act:stats".into(), ..Default::default() });
                chip_list.extend(chips("sort", &prefs::get("gameSort"), &[("recent", "sort.recent"), ("az", "sort.az"), ("playtime", "sort.playtime"), ("added", "sort.added")], true));
                chip_list.extend(chips("filter", &prefs::get("gameFilter"), &[("all", "filter.all"), ("steam", "filter.steam"), ("other", "filter.other"), ("favorites", "filter.favorites"), ("hidden", "filter.hidden")], true));
                let steam = ctx.state.borrow().pointer("/library/steamFound").and_then(Value::as_bool).unwrap_or(false);
                (t("tab.games"), if all == 0 { t(if steam { "games.emptySteam" } else { "games.empty" }) } else { String::new() })
            }
            "movies" => {
                chip_list.extend(chips("sort", &prefs::get("movieSort"), &MEDIA_SORTS, false));
                chip_list.extend(chips("filter", &prefs::get("movieFilter"), &[("all", "filter.all"), ("unwatched", "filter.unwatched"), ("progress", "filter.inProgress"), ("favorites", "filter.favorites"), ("hidden", "filter.hidden")], true));
                (t("tab.movies"), if all == 0 { t("lib.noMovies") } else { String::new() })
            }
            _ => {
                chip_list.extend(chips("sort", &prefs::get("showSort"), &MEDIA_SORTS, false));
                chip_list.extend(chips("filter", &prefs::get("showFilter"), &[("all", "filter.all"), ("unwatched", "filter.unwatched"), ("progress", "filter.watching"), ("favorites", "filter.favorites"), ("hidden", "filter.hidden")], true));
                (t("tab.shows"), if all == 0 { t("lib.noShows") } else { String::new() })
            }
        };
        let data = GridData {
            title: title.into(),
            count: tv("grid.count", &[("n", list.len().into()), ("total", all.into())]).into(),
            chips: model::model(chip_list),
            cards: model::model(list.iter().map(poster_card).collect()),
            empty: empty.into(),
        };
        match page {
            "games" => grids.set_games(data),
            "movies" => grids.set_movies(data),
            _ => grids.set_shows(data),
        }
    }
}
