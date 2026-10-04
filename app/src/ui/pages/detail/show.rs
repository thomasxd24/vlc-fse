//! A series' page (VIEWS.show), with its season tabs (selectSeason).

use super::{action, backdrop, current_id, first_of, meta_line, more, strings};
use crate::ui::actions;
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, arr, b, s};
use crate::ui::router;
use crate::{EpisodeTile, GridChip, ShowDetail, ShowFocus};
use serde_json::{json, Value};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::time::Duration;

thread_local! {
    /// The selected season of the show on screen: (show id, season number) — the renderer's `r.season`.
    static SEASON: RefCell<Option<(String, i64)>> = const { RefCell::new(None) };
    /// A season to open the next show page at (Go to show from an episode's options).
    static PENDING: Cell<Option<i64>> = const { Cell::new(None) };
    /// Debounces season switching while moving along the chips.
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Open a show's page, optionally at a given season.
pub fn open_show(id: &str, season: Option<i64>) {
    PENDING.with(|p| p.set(season));
    router::go("show", id);
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let sd = ui.global::<ShowDetail>();
    sd.on_act(|value| {
        let Some(id) = current_id("show") else { return };
        let Some(show) = cx().find("shows", &id) else { return };
        match value.as_str() {
            v if v.starts_with("play:") => actions::play_episode(&id, &v[5..], ""),
            v if v.starts_with("again:") => actions::play_episode(&id, &v[6..], "start"),
            "watched" => {
                let all = watched_count(&show) == arr(&show, "episodes").len();
                actions::set_watched(json!({ "kind": "show", "id": id, "watched": !all }));
            }
            _ => actions::options("show", &id, ""),
        }
    });
    sd.on_options(|| {
        if let Some(id) = current_id("show") {
            actions::options("show", &id, "");
        }
    });
    sd.on_season_focused(|i| {
        TIMER.with(|t| t.start(slint::TimerMode::SingleShot, Duration::from_millis(180), move || select_season(i)));
    });
    sd.on_season_pressed(|i| {
        TIMER.with(|t| t.stop());
        select_season(i);
    });
    sd.on_season_options(|i| {
        let Some(id) = current_id("show") else { return };
        let Some(show) = cx().find("shows", &id) else { return };
        if let Some(n) = seasons(&show).get(i as usize) {
            actions::options("season", &n.to_string(), &id);
        }
    });
    sd.on_play(|i| {
        if let Some((id, e)) = episode_at(i) {
            actions::play_episode(&id, s(&e, "id"), "");
        }
    });
    sd.on_episode_options(|i| {
        if let Some((id, e)) = episode_at(i) {
            actions::options("episode", s(&e, "id"), &id);
        }
    });
    ctx.on_event("route", |_, r| {
        if r.get("name").and_then(Value::as_str) != Some("show") {
            return;
        }
        let id = r.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let forward = r.get("forward").and_then(Value::as_bool).unwrap_or(false);
        let pending = PENDING.with(|p| p.take());
        let same = SEASON.with(|x| x.borrow().as_ref().map(|(sid, _)| *sid == id).unwrap_or(false));
        if forward || !same {
            SEASON.with(|x| *x.borrow_mut() = pending.map(|n| (id.clone(), n)));
            let ui = cx().ui();
            ui.global::<ShowDetail>().set_focus(ShowFocus { zone: 0, action: 0, chip: 0, ep: 0 });
        }
        rebuild(true);
    });
    ctx.on_event("state", |_, _| rebuild(false));
}

fn watched_count(show: &Value) -> usize {
    show.get("watchedCount").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn seasons(show: &Value) -> Vec<i64> {
    arr(show, "seasons").iter().filter_map(Value::as_i64).collect()
}

fn season_of(e: &Value) -> i64 {
    e.get("season").and_then(Value::as_i64).unwrap_or(0)
}

/// The selected season: kept while it exists, else the next episode's, else the first real season.
fn current_season(show: &Value) -> i64 {
    let id = s(show, "id");
    let list = seasons(show);
    let kept = SEASON.with(|x| x.borrow().as_ref().filter(|(sid, _)| sid == id).map(|(_, n)| *n));
    if let Some(n) = kept.filter(|n| list.contains(n)) {
        return n;
    }
    let next = arr(show, "episodes").iter().find(|e| s(e, "id") == s(show, "nextUp")).map(season_of);
    let n = next.or_else(|| list.iter().copied().find(|n| *n > 0)).or_else(|| list.first().copied()).unwrap_or(0);
    SEASON.with(|x| *x.borrow_mut() = Some((id.to_string(), n)));
    n
}

fn episode_at(i: i32) -> Option<(String, Value)> {
    let id = current_id("show")?;
    let show = cx().find("shows", &id)?;
    let season = current_season(&show);
    let e = arr(&show, "episodes").iter().filter(|e| season_of(e) == season).nth(i as usize)?.clone();
    Some((id, e))
}

fn select_season(i: i32) {
    let Some(id) = current_id("show") else { return };
    let Some(show) = cx().find("shows", &id) else { return };
    let Some(n) = seasons(&show).get(i as usize).copied() else { return };
    if n == current_season(&show) {
        return;
    }
    SEASON.with(|x| *x.borrow_mut() = Some((id, n)));
    {
        let ui = cx().ui();
        let sd = ui.global::<ShowDetail>();
        let mut f = sd.get_focus();
        f.ep = 0;
        sd.set_focus(f);
    }
    rebuild(false);
}

fn episode_tile(e: &Value, show: &Value) -> EpisodeTile {
    let pr = e.get("progress").cloned().unwrap_or(Value::Null);
    let episode = e.get("episode").and_then(Value::as_i64).unwrap_or(0);
    let end = e.get("episodeEnd").and_then(Value::as_i64).map(|x| format!("–{x}")).unwrap_or_default();
    let title = match s(e, "title") {
        "" => tv("ep.n", &[("n", episode.into())]),
        x => x.to_string(),
    };
    let runtime = e.get("runtime").and_then(Value::as_i64).unwrap_or(0);
    let year = s(e, "airDate").get(0..4).unwrap_or("").to_string();
    let meta = [fmt::runtime(runtime), year].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let resumable = b(&pr, "resumable");
    EpisodeTile {
        num: format!("{episode}{end}").into(),
        title: title.into(),
        subtitle: if meta.is_empty() && resumable { fmt::remaining(&pr) } else { meta }.into(),
        overview: s(e, "overview").into(),
        art: s(e, "thumb").into(),
        placeholder: format!("E{episode}").into(),
        tint: fmt::tint_of(s(show, "title")),
        progress: if resumable { fmt::pct(&pr) } else { -1.0 },
        watched: b(&pr, "watched"),
    }
}

fn rebuild(entering: bool) {
    let Some(id) = current_id("show") else { return };
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<ShowDetail>();
    let Some(sh) = ctx.find("shows", &id) else {
        page.set_found(false);
        return;
    };
    if entering {
        backdrop(&first_of(&sh, &["backdrop", "poster"]));
    }
    let episodes = arr(&sh, "episodes");
    let season = current_season(&sh);
    let next = episodes.iter().find(|e| !s(&sh, "nextUp").is_empty() && s(e, "id") == s(&sh, "nextUp"));
    let all_watched = watched_count(&sh) == episodes.len();

    let mut acts = Vec::new();
    if let Some(next) = next {
        let ep = fmt::ep_code(next);
        let key = if next.pointer("/progress/resumable").and_then(Value::as_bool).unwrap_or(false) {
            "show.resumeEp"
        } else if watched_count(&sh) > 0 {
            "show.playNext"
        } else {
            "show.playEp"
        };
        acts.push(action(&tv(key, &[("ep", ep.into())]), "play", &format!("play:{}", s(next, "id")), true));
    } else if let Some(first) = episodes.first() {
        acts.push(action(&tv("show.watchAgain", &[("ep", fmt::ep_code(first).into())]), "restart", &format!("again:{}", s(first, "id")), true));
    }
    acts.push(action(&t(if all_watched { "opt.showUnwatched" } else { "opt.showWatched" }), "check", "watched", false));
    acts.push(more());

    let list = seasons(&sh);
    let chips: Vec<GridChip> = list
        .iter()
        .map(|n| {
            let done = episodes.iter().filter(|e| season_of(e) == *n).all(|e| e.pointer("/progress/watched").and_then(Value::as_bool).unwrap_or(false));
            GridChip {
                label: format!("{}{}", fmt::season_name(*n), if done { " ✓" } else { "" }).into(),
                value: n.to_string().into(),
                on: *n == season,
                ..Default::default()
            }
        })
        .collect();
    let tiles: Vec<EpisodeTile> = episodes.iter().filter(|e| season_of(e) == season).map(|e| episode_tile(e, &sh)).collect();

    page.set_found(true);
    page.set_title(s(&sh, "title").into());
    page.set_poster(s(&sh, "poster").into());
    page.set_tint(fmt::tint_of(s(&sh, "title")));
    page.set_meta(model::model(meta_line(&sh)));
    page.set_overview(s(&sh, "overview").into());
    page.set_genres(strings(&sh, "genres").join(" · ").into());
    page.set_actions(model::model(acts));
    page.set_seasons(model::model(chips));
    page.set_episodes(model::model(tiles));
    page.set_season(list.iter().position(|n| *n == season).unwrap_or(0) as i32);
}
