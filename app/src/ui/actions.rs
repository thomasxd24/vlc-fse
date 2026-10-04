//! What activating things does — opening, playing, the options menu (X / right-click / long-press),
//! adding and editing games. The renderer's `ACTIONS`, `openOptions`, `playMovie`, `playEpisode`,
//! `startMedia`, `playGame`, `setPref`, `chooseLanguage`, `chooseItemLanguages`; game editing
//! (`editGame`, `matchSteam`, `pickArtwork`, `removeGame`, `addGame`, `showScreenshot`) is in
//! [`edit`].

mod edit;

use super::ctx::cx;
use super::dialogs::{self, choice, Choice};
use super::fmt;
use super::i18n::{t, tv, Arg};
use super::model::{arr, b, s};
use super::router;
use super::toasts::toast;
use serde_json::{json, Value};

pub use edit::{add_game, edit_game, remove_game, show_screenshot};

/// Wire the game modals (artwork picker, screenshot lightbox). Called by the detail pages' install.
pub fn install() {
    edit::install();
}

/// `dialogs::choose`, with the answer handled on the next turn of the event loop. Closing a dialog hands
/// focus back to the page (asynchronously); a menu that opens another one from its answer would lose
/// the focus to the page without this.
pub(crate) fn choose(title: &str, text: &str, choices: Vec<Choice>, on_answer: impl FnOnce(Option<String>) + 'static) {
    dialogs::choose(title, text, choices, move |v| later(move || on_answer(v)));
}

/// `dialogs::prompt`, answered on the next turn of the event loop (see [`choose`]).
pub(crate) fn prompt(p: dialogs::Prompt, on_answer: impl FnOnce(Option<String>) + 'static) {
    dialogs::prompt(p, move |v| later(move || on_answer(v)));
}

pub(crate) fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(std::time::Duration::ZERO, f);
}

fn ch(label: &str, value: &str, icon: &str) -> Choice {
    Choice { icon: icon.into(), ..choice(label, value) }
}

fn primary(c: Choice) -> Choice {
    Choice { primary: true, ..c }
}

/// `t(r.errorKey, r.vars)` as an error toast, for a failed `{ ok: false, errorKey, vars }` result.
pub(crate) fn error_toast(r: &Value) {
    let Some(key) = r.get("errorKey").and_then(Value::as_str) else { return };
    let vars: Vec<(String, Arg)> = r
        .get("vars")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match v {
                            Value::Number(n) => Arg::N(n.as_f64().unwrap_or(0.0)),
                            Value::String(s) => Arg::S(s.clone()),
                            other => Arg::S(other.to_string()),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let refs: Vec<(&str, Arg)> = vars.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    toast(&tv(key, &refs), "error");
}

fn ok(r: &Value) -> bool {
    r.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

/// A show and one of its episodes. `show_id` may be empty (search every show).
pub(crate) fn find_episode(show_id: &str, id: &str) -> Option<(Value, Value)> {
    let shows = cx().library("shows");
    shows.into_iter().filter(|x| show_id.is_empty() || s(x, "id") == show_id).find_map(|show| {
        let e = arr(&show, "episodes").iter().find(|e| s(e, "id") == id).cloned()?;
        Some((show, e))
    })
}

fn progress(v: &Value) -> Value {
    v.get("progress").cloned().unwrap_or(Value::Null)
}

fn resume_label(pr: &Value) -> String {
    tv("media.resumeFrom", &[("time", fmt::time(pr.get("time").and_then(Value::as_f64).unwrap_or(0.0)).into())])
}

// ---------------------------------------------------------------------------------------- Options

/// The options menu for an item: kind is "movie" | "show" | "game" | "episode" | "app" | "season".
/// Episodes pass their show as `show_id`; a season passes its number as `id` and its show as `show_id`.
pub fn options(kind: &str, id: &str, show_id: &str) {
    let ctx = cx();
    let route = router::current().0;
    let fav = |item: &Value| {
        let on = b(item, "favorite");
        ch(&t(if on { "opt.unfavorite" } else { "opt.favorite" }), "fav", if on { "star-on" } else { "star" })
    };
    let hide = |item: &Value| {
        let on = b(item, "hidden");
        ch(&t(if on { "opt.unhide" } else { "opt.hide" }), "hide", if on { "show" } else { "hide" })
    };
    match kind {
        "game" => {
            let Some(g) = ctx.find("games", id) else { return };
            let mut choices = vec![primary(ch(&t("game.play"), "play", "play")), fav(&g), ch(&t("game.edit"), "edit", "edit")];
            if route != "game" {
                choices.push(ch(&t("opt.details"), "open", "more"));
            }
            choices.push(ch(&t("opt.showFolder"), "folder", "folder"));
            choices.push(hide(&g));
            if s(&g, "source") == "manual" {
                choices.push(Choice { danger: true, ..ch(&t("opt.remove"), "remove", "trash") });
            }
            let id = id.to_string();
            let title = s(&g, "title").to_string();
            choose(&title, "", choices, move |v| match v.as_deref() {
                Some("play") => play_game(&id),
                Some("fav") => set_pref(&id, "favorites", !b(&g, "favorite")),
                Some("edit") => edit_game(&id),
                Some("open") => router::go("game", &id),
                Some("folder") => cx().send("show_game_folder", move |b| b.show_game_folder(&id)),
                Some("hide") => set_pref(&id, "hidden", !b(&g, "hidden")),
                Some("remove") => remove_game(&id),
                _ => {}
            });
        }
        "movie" => {
            let Some(m) = ctx.find("movies", id) else { return };
            let pr = progress(&m);
            let resumable = b(&pr, "resumable");
            let watched = b(&pr, "watched");
            let choices = vec![
                primary(ch(&if resumable { resume_label(&pr) } else { t("media.play") }, "play", "play")),
                ch(&t(if watched { "opt.markUnwatched" } else { "opt.markWatched" }), "watched", "check"),
                fav(&m),
                ch(&t("opt.languages"), "lang", "subs"),
                hide(&m),
                ch(&t("opt.showFile"), "file", "folder"),
            ];
            let id = id.to_string();
            let title = s(&m, "title").to_string();
            choose(&title, "", choices, move |v| match v.as_deref() {
                Some("play") => play_movie(&id, if resumable { "resume" } else { "start" }),
                Some("lang") => choose_item_languages(&id),
                Some("watched") => {
                    let req = json!({ "kind": "movie", "id": id, "watched": !watched });
                    cx().send("set_watched", move |b| b.set_watched(req));
                }
                Some("fav") => set_pref(&id, "favorites", !b(&m, "favorite")),
                Some("hide") => set_pref(&id, "hidden", !b(&m, "hidden")),
                Some("file") => {
                    let path = s(&m, "path").to_string();
                    cx().send("show_in_folder", move |b| b.show_in_folder(&path));
                }
                _ => {}
            });
        }
        "show" => {
            let Some(sh) = ctx.find("shows", id) else { return };
            let all = n_watched(&sh) == arr(&sh, "episodes").len();
            let mut choices = vec![
                ch(&t(if all { "opt.showUnwatched" } else { "opt.showWatched" }), "watched", "check"),
                fav(&sh),
                ch(&t("opt.languages"), "lang", "subs"),
                hide(&sh),
            ];
            if route != "show" {
                choices.insert(0, primary(ch(&t("opt.details"), "open", "more")));
            }
            let id = id.to_string();
            let title = s(&sh, "title").to_string();
            choose(&title, "", choices, move |v| match v.as_deref() {
                Some("open") => router::go("show", &id),
                Some("lang") => choose_item_languages(&id),
                Some("watched") => {
                    let req = json!({ "kind": "show", "id": id, "watched": !all });
                    cx().send("set_watched", move |b| b.set_watched(req));
                }
                Some("fav") => set_pref(&id, "favorites", !b(&sh, "favorite")),
                Some("hide") => set_pref(&id, "hidden", !b(&sh, "hidden")),
                _ => {}
            });
        }
        "episode" => {
            let Some((show, e)) = find_episode(show_id, id) else { return };
            let show_id = s(&show, "id").to_string();
            let pr = progress(&e);
            let resumable = b(&pr, "resumable");
            let watched = b(&pr, "watched");
            let mut choices = vec![
                primary(ch(&if resumable { resume_label(&pr) } else { t("media.play") }, "play", "play")),
                ch(&t(if watched { "opt.markUnwatched" } else { "opt.markWatched" }), "watched", "check"),
            ];
            if route != "show" {
                choices.push(ch(&t("opt.goToShow"), "show", "more"));
            }
            let title = format!("{} · {}", s(&show, "title"), fmt::ep_code(&e));
            let id = id.to_string();
            let season = e.get("season").and_then(Value::as_i64).unwrap_or(0);
            choose(&title, s(&e, "title"), choices, move |v| match v.as_deref() {
                Some("play") => play_episode(&show_id, &id, if resumable { "resume" } else { "start" }),
                Some("watched") => {
                    let req = json!({ "kind": "episode", "showId": show_id, "id": id, "watched": !watched });
                    cx().send("set_watched", move |b| b.set_watched(req));
                }
                Some("show") => super::pages::detail::open_show(&show_id, Some(season)),
                _ => {}
            });
        }
        "season" => {
            let Some(sh) = ctx.find("shows", show_id) else { return };
            let n: i64 = id.parse().unwrap_or(0);
            let done = arr(&sh, "episodes")
                .iter()
                .filter(|e| e.get("season").and_then(Value::as_i64) == Some(n))
                .all(|e| b(&progress(e), "watched"));
            let title = format!("{} · {}", s(&sh, "title"), fmt::season_name(n));
            let show_id = show_id.to_string();
            choose(&title, "", vec![ch(&t(if done { "opt.markUnwatched" } else { "opt.markWatched" }), "watched", "check")], move |v| {
                if v.as_deref() == Some("watched") {
                    let req = json!({ "kind": "season", "showId": show_id, "id": n, "watched": !done });
                    cx().send("set_watched", move |b| b.set_watched(req));
                }
            });
        }
        "app" => {
            let apps = ctx.state.borrow().get("apps").and_then(Value::as_array).cloned().unwrap_or_default();
            let Some(app) = apps.into_iter().find(|a| s(a, "id") == id) else { return };
            let hidden = b(&app, "hidden");
            let choices = vec![primary(ch(&t("apps.open"), "open", "play")), ch(&t(if hidden { "opt.unhide" } else { "opt.hide" }), "hide", if hidden { "show" } else { "hide" })];
            let id = id.to_string();
            choose(s(&app, "name"), "", choices, move |v| match v.as_deref() {
                Some("open") => super::pages::apps::open_app(&id),
                Some("hide") => cx().call(
                    "hide_app",
                    move |b| b.hide_app(&id, !hidden),
                    move |_| toast(&t(if hidden { "apps.unhidden" } else { "apps.hidden" }), "info"),
                ),
                _ => {}
            });
        }
        _ => {}
    }
}

fn n_watched(show: &Value) -> usize {
    show.get("watchedCount").and_then(Value::as_u64).unwrap_or(0) as usize
}

/// Favourite / hide an item (`key`: "favorites" | "hidden"), with the renderer's toast.
pub fn set_pref(id: &str, key: &str, value: bool) {
    let (id, k) = (id.to_string(), key.to_string());
    cx().send("set_pref", move |b| b.set_pref(&id, &k, value));
    let msg = if key == "favorites" {
        if value { "toast.favAdded" } else { "toast.favRemoved" }
    } else if value {
        "toast.hidden"
    } else {
        "toast.unhidden"
    };
    toast(&t(msg), "info");
}

/// Mark a movie / show / episode / season watched or not (the detail pages' toggle-watched button).
pub fn set_watched(req: Value) {
    let watched = req.get("watched").and_then(Value::as_bool).unwrap_or(false);
    cx().send("set_watched", move |b| b.set_watched(req));
    toast(&t(if watched { "toast.markedWatched" } else { "toast.markedUnwatched" }), "info");
}

// ---------------------------------------------------------------------------------------- Playback

/// Play a movie. mode: "" (ask resume / start over when there's progress), "resume", "start".
pub fn play_movie(id: &str, mode: &str) {
    let Some(m) = cx().find("movies", id) else { return };
    let pr = progress(&m);
    let id = id.to_string();
    if mode.is_empty() && b(&pr, "resumable") {
        choose(s(&m, "title"), "", resume_choices(&pr), move |v| {
            if let Some(v) = v {
                start_media(json!({ "kind": "movie", "id": id, "resume": v == "resume" }));
            }
        });
        return;
    }
    start_media(json!({ "kind": "movie", "id": id, "resume": mode == "resume" }));
}

/// Play an episode of a show; mode as for `play_movie` (but no mode resumes by default).
pub fn play_episode(show_id: &str, id: &str, mode: &str) {
    let Some((show, e)) = find_episode(show_id, id) else { return };
    let show_id = s(&show, "id").to_string();
    let pr = progress(&e);
    let id = id.to_string();
    if mode.is_empty() && b(&pr, "resumable") {
        let title = format!("{} · {}", s(&show, "title"), fmt::ep_code(&e));
        choose(&title, s(&e, "title"), resume_choices(&pr), move |v| {
            if let Some(v) = v {
                start_media(json!({ "kind": "episode", "showId": show_id, "id": id, "resume": v == "resume" }));
            }
        });
        return;
    }
    start_media(json!({ "kind": "episode", "showId": show_id, "id": id, "resume": mode != "start" }));
}

fn resume_choices(pr: &Value) -> Vec<Choice> {
    vec![primary(ch(&resume_label(pr), "resume", "play")), ch(&t("media.playFromStart"), "start", "restart")]
}

fn start_media(req: Value) {
    cx().call("play", move |b| b.play(req), |r| {
        if !ok(&r) {
            error_toast(&r);
        }
    });
}

/// Launch a game.
pub fn play_game(id: &str) {
    let id = id.to_string();
    cx().call("play_game", move |b| b.play_game(&id), |r| {
        if !ok(&r) {
            error_toast(&r);
        }
    });
}

// ---------------------------------------------------------------------------------------- Languages

const LANGS: [&str; 6] = ["en", "fr", "de", "es", "it", "ja"];

fn lang_label(code: &str) -> String {
    t(&format!("lang.{code}"))
}

fn is_audio(kind: &str) -> bool {
    kind == "audio"
}

/// Pick a language for audio / subtitles. kind: "audio" | "subtitles" (or "subs"); extra: leading
/// (label, value) choices (e.g. "Default", "Off"). The entry whose value is `current` is checked. Calls
/// back with the chosen code, or None if cancelled.
pub fn choose_language(kind: &str, current: &str, extra: Vec<(String, String)>, done: impl FnOnce(Option<String>) + 'static) {
    let audio = is_audio(kind);
    let mut codes: Vec<&str> = LANGS.to_vec();
    codes.push(if audio { "original" } else { "off" });
    let mut choices: Vec<Choice> = extra.iter().map(|(label, value)| Choice { checked: value == current, ..choice(label, value) }).collect();
    choices.extend(codes.into_iter().map(|c| Choice { checked: c == current, ..choice(&lang_label(c), c) }));
    choose(&t(if audio { "set.audioLang" } else { "set.subLang" }), "", choices, done);
}

/// Per film / show languages, overriding the defaults from Settings (the renderer's
/// chooseItemLanguages): a small menu that reopens after each change until Done or Back.
pub fn choose_item_languages(id: &str) {
    let ctx = cx();
    let Some(item) = ctx.find("movies", id).or_else(|| ctx.find("shows", id)) else { return };
    let cur = item.get("languages").cloned().filter(Value::is_object);
    let default_of = |kind: &str| {
        let key = if is_audio(kind) { "audioLanguage" } else { "subLanguage" };
        let code = ctx.setting(key).as_str().unwrap_or("").to_string();
        tv("lang.default", &[("value", lang_label(&code).into())])
    };
    let set_code = |kind: &str| cur.as_ref().and_then(|c| c.get(kind)).and_then(Value::as_str).map(String::from);
    let shown = |kind: &str| set_code(kind).map(|c| lang_label(&c)).unwrap_or_else(|| default_of(kind));
    let title = s(&item, "title").to_string();
    let mut choices = vec![
        ch(&tv("lang.audio", &[("value", shown("audio").into())]), "audio", "volume"),
        ch(&tv("lang.subs", &[("value", shown("subs").into())]), "subs", "subs"),
    ];
    if cur.is_some() {
        choices.push(ch(&t("lang.reset"), "reset", "restart"));
    }
    choices.push(primary(choice(&t("common.done"), "done")));
    let id = id.to_string();
    let defaults = (default_of("audio"), default_of("subs"));
    let codes = (set_code("audio"), set_code("subs"));
    choose(
        &format!("{} · {title}", t("opt.languages")),
        &tv("lang.forThis", &[("name", title.clone().into())]),
        choices,
        move |v| {
            let Some(v) = v.filter(|v| v != "done") else { return };
            if v == "reset" {
                save_languages(id, None);
                return;
            }
            let (default_label, current) = if v == "audio" { (defaults.0, codes.0) } else { (defaults.1, codes.1) };
            let extra = vec![(default_label, "default".to_string())];
            let kind = v.clone();
            choose_language(&kind, current.as_deref().unwrap_or("default"), extra, move |picked| {
                let Some(picked) = picked else {
                    // Cancelled: back to the languages menu.
                    choose_item_languages(&id);
                    return;
                };
                let mut next = cur.and_then(|c| c.as_object().cloned()).unwrap_or_default();
                if picked == "default" {
                    next.remove(&v);
                } else {
                    next.insert(v.clone(), json!(picked));
                }
                save_languages(id, Some(Value::Object(next)));
            });
        },
    );
}

fn save_languages(id: String, languages: Option<Value>) {
    let id2 = id.clone();
    cx().call("set_languages", move |b| b.set_languages(&id2, languages), move |_| {
        toast(&t("lang.saved"), "info");
        choose_item_languages(&id);
    });
}
