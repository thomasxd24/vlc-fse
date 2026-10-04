//! Home: the minimal launcher (the renderer's VIEWS.home with greeting / resumeTarget / homeClockText).
//! The clock and date, a greeting, the one thing to pick back up, and doors into the tabs. The
//! backdrop shows the resume target's artwork.
//!
//! With nothing in the library (and no scan running) the renderer drew the Welcome page in Home's
//! place; here that's the `welcome` route, so Home hands over to it.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, arr, b, n, s};
use crate::ui::{actions, fmt, router};
use crate::{Backdrop, HomeDoor, HomePage, HomeResume, Nav};
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    static CLOCK: slint::Timer = slint::Timer::default();
    /// The current resume target's background art (set as the backdrop while Home is shown).
    static BG: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let home = ui.global::<HomePage>();
    home.on_play(|| {
        let r = cx().ui().global::<HomePage>().get_resume();
        match r.kind.as_str() {
            "game" => actions::play_game(&r.id),
            "movie" => actions::play_movie(&r.id, ""),
            "episode" => actions::play_episode(&r.show_id, &r.id, ""),
            _ => {}
        }
    });
    home.on_options(|| {
        let ui = cx().ui();
        let home = ui.global::<HomePage>();
        if home.get_has_resume() {
            let r = home.get_resume();
            actions::options(&r.kind, &r.id, &r.show_id);
        }
    });
    home.on_open_door(|tab| {
        router::switch_tab(&tab);
        cx().ui().global::<Nav>().invoke_focus_page();
    });

    tick_clock();
    // The renderer refreshed every 15 s; once a second keeps the minute exact (it only sets on change).
    CLOCK.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(1), tick_clock));

    ctx.on_event("state", |_, _| {
        rebuild();
        if on_home() {
            show();
        }
    });
    ctx.on_event("route", |_, r| {
        if r.get("name").and_then(Value::as_str) != Some("home") {
            return;
        }
        let forward = r.get("forward").and_then(Value::as_bool).unwrap_or(true);
        rebuild();
        if forward {
            // `data-autofocus`: the resume card, else the first door.
            let ui = cx().ui();
            let home = ui.global::<HomePage>();
            home.set_focus(if home.get_has_resume() { 0 } else { 1 });
            home.set_last_door(0);
        }
        show();
    });
}

fn on_home() -> bool {
    router::current().0 == "home"
}

/// Home is (or stays) on screen: set its backdrop, or hand over to Welcome if there's nothing to show.
fn show() {
    let ctx = cx();
    let visible = |k: &str| ctx.library(k).iter().any(|x| !b(x, "hidden"));
    let scanning = ctx.state.borrow().get("scanning").and_then(Value::as_bool).unwrap_or(false);
    if !visible("games") && !visible("movies") && !visible("shows") && !scanning {
        router::reset("welcome");
        return;
    }
    let ui = ctx.ui();
    ui.global::<Backdrop>().set_src(BG.with(|b| b.borrow().clone()).into());
}

fn tick_clock() {
    let ui = cx().ui();
    let home = ui.global::<HomePage>();
    let clock = fmt::clock(false);
    if home.get_clock() != clock.as_str() {
        home.set_clock(clock.into());
        home.set_date(fmt::long_date().into());
        home.set_greeting(greeting().into());
    }
}

fn greeting() -> String {
    let hr = fmt::hour();
    let part = match hr {
        0..=4 => "night",
        5..=11 => "morning",
        12..=17 => "afternoon",
        18..=21 => "evening",
        _ => "night",
    };
    let name = cx().state.borrow().pointer("/library/steamUser").and_then(Value::as_str).unwrap_or("").to_string();
    let text = if name.is_empty() { t(&format!("greet.{part}")) } else { tv(&format!("greet.{part}Name"), &[("name", name.into())]) };
    // `.mh-greet { text-transform: uppercase }`
    text.to_uppercase()
}

fn first_of<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().map(|k| s(v, k)).find(|x| !x.is_empty()).unwrap_or("")
}

/// The one thing Home offers to pick up: the most recently played game or watched item, with its
/// background art.
fn resume_target() -> Option<(HomeResume, String)> {
    let ctx = cx();
    let games = ctx.library("games");
    let movies = ctx.library("movies");
    let shows = ctx.library("shows");
    let cont = ctx.library("continueWatching");
    enum Pick<'a> {
        Game(&'a Value),
        Watch(&'a Value),
    }
    let mut picks: Vec<(f64, Pick)> = games.iter().filter(|g| !b(g, "hidden") && n(g, "lastPlayed") > 0.0).map(|g| (n(g, "lastPlayed"), Pick::Game(g))).collect();
    picks.extend(cont.iter().map(|c| (n(c, "at"), Pick::Watch(c))));
    // Stable, newest first (games before Continue watching on a tie, as the renderer's sort).
    picks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, p) in picks {
        match p {
            Pick::Game(g) => {
                let sub = [fmt::ago(n(g, "lastPlayed") as i64), fmt::playtime(n(g, "playtime") as i64)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
                let r = HomeResume { kind: "game".into(), id: s(g, "id").into(), title: s(g, "title").into(), logo: s(g, "logo").into(), sub: sub.into(), progress: -1.0, ..Default::default() };
                return Some((r, first_of(g, &["hero", "header", "poster"]).into()));
            }
            Pick::Watch(c) if s(c, "kind") == "movie" => {
                let Some(m) = movies.iter().find(|m| s(m, "id") == s(c, "id")) else { continue };
                let pr = m.get("progress").cloned().unwrap_or(Value::Null);
                let sub = match fmt::remaining(&pr) {
                    x if x.is_empty() => t("media.resume"),
                    x => x,
                };
                let r = HomeResume { kind: "movie".into(), id: s(m, "id").into(), title: s(m, "title").into(), sub: sub.into(), progress: fmt::pct(&pr), ..Default::default() };
                return Some((r, first_of(m, &["backdrop", "poster"]).into()));
            }
            Pick::Watch(c) => {
                let found = shows.iter().find_map(|sh| arr(sh, "episodes").iter().find(|e| s(e, "id") == s(c, "id")).map(|e| (sh, e)));
                let Some((show, e)) = found else { continue };
                let pr = e.get("progress").cloned().unwrap_or(Value::Null);
                let mut sub = fmt::ep_code(e);
                if !s(e, "title").is_empty() {
                    sub += &format!(" · {}", s(e, "title"));
                }
                let resumable = b(&pr, "resumable");
                if resumable {
                    sub += &format!(" · {}", fmt::remaining(&pr));
                }
                let r = HomeResume {
                    kind: "episode".into(),
                    id: s(e, "id").into(),
                    show_id: s(show, "id").into(),
                    title: s(show, "title").into(),
                    sub: sub.into(),
                    progress: if resumable { fmt::pct(&pr) } else { -1.0 },
                    ..Default::default()
                };
                let bg = [s(show, "backdrop"), s(e, "thumb"), s(show, "poster")].into_iter().find(|x| !x.is_empty()).unwrap_or("");
                return Some((r, bg.into()));
            }
        }
    }
    None
}

fn rebuild() {
    let ctx = cx();
    let ui = ctx.ui();
    let home = ui.global::<HomePage>();
    let visible = |k: &str| ctx.library(k).iter().any(|x| !b(x, "hidden"));
    let apps = ctx.state.borrow().get("apps").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    let doors: Vec<HomeDoor> = [("games", "tab.games", visible("games")), ("movies", "tab.movies", visible("movies")), ("shows", "tab.shows", visible("shows")), ("apps", "tab.apps", apps)]
        .into_iter()
        .filter(|(_, _, on)| *on)
        .map(|(tab, key, _)| HomeDoor { tab: tab.into(), label: t(key).into() })
        .collect();
    home.set_doors(model::model(doors));
    match resume_target() {
        Some((r, bg)) => {
            home.set_resume(r);
            home.set_has_resume(true);
            BG.with(|b| *b.borrow_mut() = bg);
        }
        None => {
            home.set_has_resume(false);
            BG.with(|b| b.borrow_mut().clear());
        }
    }
    // The greeting follows the language and the Steam user.
    home.set_greeting(greeting().into());
    home.set_clock(fmt::clock(false).into());
    home.set_date(fmt::long_date().into());
}
