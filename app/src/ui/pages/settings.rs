//! Settings (the renderer's VIEWS.settings, toggleRow / valueRow / updateRows / controlsHelp, and the
//! settings ACTIONS: save, editKey, addLibrary, the VLC / Steam / language menus, text size, animations,
//! updates, FSE, system buttons).
//!
//! The page is a flat list of `SetItem`s rebuilt from the state payload. Rust keeps, next to each item,
//! what activating it (or each of its buttons) does, and owns the focus: the renderer's spatial
//! navigation on this page boils down to "Up / Down between lines, Left / Right between the buttons of an
//! actions line, and an actions line remembers the button focused last", which is what `move_focus` does.
//! Saving goes through the backend, which pushes a new `state`: `sys` re-applies language, scale, motion,
//! sounds and rumble from it, and this page rebuilds.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, choice, Choice, Prompt};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, arr, b, s};
use crate::ui::overlays::update;
use crate::ui::{actions, input, router, toasts};
use crate::{Nav, SetBtn, SetItem, SettingsPage};
use serde_json::{json, Map, Value};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;

/// One line of the page: what Slint draws, plus what activating it (or each of its buttons) does.
#[derive(Clone)]
struct Line {
    item: SetItem,
    key: String,
    /// Actions: the row's own (rows), or one per button (action lines). "" = not focusable / no-op.
    acts: Vec<String>,
}

#[derive(Default)]
struct Page {
    lines: Vec<Line>,
    /// The focused line's key and button, so focus survives rebuilds that shift lines around.
    focus_key: String,
    col: usize,
    /// Action lines remember their last focused button (the renderer's nav-group memory).
    memory: HashMap<String, usize>,
    /// The detected VLC (the renderer's S.vlcFound), the update state (S.update).
    vlc_found: Option<String>,
    update: Value,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<SettingsPage>();
    page.on_move(|dir| move_focus(&dir));
    page.on_activate(|| {
        let act = PAGE.with(|p| {
            let p = p.borrow();
            p.lines.iter().find(|l| l.key == p.focus_key).and_then(|l| l.acts.get(p.col).cloned())
        });
        if let Some(act) = act {
            run(&act);
        }
    });
    page.on_pointed(|line, col, click| {
        let (line, col) = (line.max(0) as usize, col.max(0) as usize);
        let act = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let l = p.lines.get(line)?.clone();
            if !stops_of(&l).contains(&col) {
                return None;
            }
            p.focus_key = l.key.clone();
            p.col = col;
            if l.acts.len() > 1 {
                p.memory.insert(l.key.clone(), col);
            }
            Some(l.acts[col].clone())
        });
        let Some(act) = act else { return };
        push_focus();
        if click {
            input::feedback("select");
            run(&act);
        }
    });

    ctx.on_event("state", |_, st| {
        if let Some(up) = st.get("update") {
            PAGE.with(|p| p.borrow_mut().update = up.clone());
        }
        rebuild();
    });
    ctx.on_event("update", |_, up| {
        let changed = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let changed = s(&p.update, "status") != s(up, "status") || p.update.get("progress") != up.get("progress");
            p.update = up.clone();
            changed
        });
        if changed {
            rebuild();
        }
    });
    ctx.on_event("route", |_, r| {
        if s(r, "name") == "settings" {
            detect_vlc(false);
        }
    });
    detect_vlc(false);
}

// ---------------------------------------------------------------- Building the page

fn section(name: &str) -> Line {
    Line { item: SetItem { kind: "section".into(), name: name.into(), ..Default::default() }, key: String::new(), acts: vec![] }
}

/// `toggleRow(name, desc, on, setting, key)`.
fn toggle_row(name: &str, desc: &str, on: bool, setting: &str) -> Line {
    Line {
        item: SetItem { kind: "toggle".into(), name: name.into(), desc: desc.into(), on, ..Default::default() },
        key: format!("t-{setting}"),
        acts: vec![format!("toggle:{setting}")],
    }
}

/// `valueRow(name, desc, value, act, key)`.
fn value_row(name: &str, desc: &str, value: &str, act: &str, key: &str) -> Line {
    Line {
        item: SetItem { kind: "value".into(), name: name.into(), desc: desc.into(), value: value.into(), ..Default::default() },
        key: key.into(),
        acts: vec![act.into()],
    }
}

struct B<'a> {
    label: String,
    icon: &'a str,
    act: &'a str,
    primary: bool,
    danger: bool,
    disabled: bool,
}

fn btn<'a>(label: String, icon: &'a str, act: &'a str) -> B<'a> {
    B { label, icon, act, primary: false, danger: false, disabled: false }
}

fn buttons(list: Vec<B>) -> (Vec<SetBtn>, Vec<String>) {
    let acts = list.iter().map(|x| if x.disabled { String::new() } else { x.act.to_string() }).collect();
    let btns = list
        .into_iter()
        .map(|x| SetBtn { label: x.label.into(), icon: x.icon.into(), primary: x.primary, danger: x.danger, disabled: x.disabled })
        .collect();
    (btns, acts)
}

fn action_line(key: &str, list: Vec<B>, note: &str) -> Line {
    let (btns, acts) = buttons(list);
    Line { item: SetItem { kind: "buttons".into(), buttons: model::model(btns), note: note.into(), ..Default::default() }, key: key.into(), acts }
}

fn text_line(kind: &str, text: &str) -> Line {
    Line { item: SetItem { kind: kind.into(), name: text.into(), ..Default::default() }, key: String::new(), acts: vec![] }
}

fn mask(v: &str) -> String {
    if v.is_empty() {
        t("set.notSet")
    } else {
        let tail: String = v.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("••••{tail}")
    }
}

fn build(st: &Value, vlc_found: Option<&str>, up: &Value) -> Vec<Line> {
    let empty = Value::Object(Map::new());
    let set = st.get("settings").unwrap_or(&empty);
    let flag = |k: &str| b(set, k);
    let version = s(st, "version");
    let fse_package = b(st, "fsePackage");
    let win = s(st, "platform") == "win32";
    let lib = st.get("library").unwrap_or(&empty);
    let games = arr(lib, "games");
    let status = |on: bool, x: Option<&Value>| -> String {
        let x = x.unwrap_or(&Value::Null);
        if !on {
            t("set.off")
        } else if b(x, "running") {
            t("set.fetching")
        } else if !s(x, "error").is_empty() {
            s(x, "error").to_string()
        } else {
            t("set.on")
        }
    };
    let mut out = Vec::new();

    // General
    out.push(section(&t("set.general")));
    let lang_label = match s(set, "uiLanguage") {
        "en" => "English".to_string(),
        "fr" => "Français".to_string(),
        _ => t("set.langAuto"),
    };
    out.push(value_row(&t("set.language"), &t("set.languageDesc"), &lang_label, "choose-language", "lang"));

    // Games
    out.push(section(&t("tab.games")));
    let steam_desc = if b(lib, "steamFound") {
        tv("set.steamFound", &[("n", games.iter().filter(|g| s(g, "source") == "steam").count().into())])
    } else {
        t("set.steamMissing")
    };
    out.push(toggle_row(&t("set.steam"), &steam_desc, flag("steamEnabled"), "steamEnabled"));
    let steam_path = s(set, "steamPath");
    out.push(value_row(&t("set.steamPath"), &t("set.steamPathDesc"), &if steam_path.is_empty() { t("set.auto") } else { steam_path.to_string() }, "pick-steam", "steam-path"));
    out.push(toggle_row(&t("set.quietSteam"), &t("set.quietSteamDesc"), set.get("quietSteam") != Some(&Value::Bool(false)), "quietSteam"));
    out.push(value_row(&t("set.sgdb"), &t("set.sgdbDesc"), &mask(s(set, "sgdbKey")), "edit-key:sgdbKey", "sgdb"));
    out.push(toggle_row(&t("set.freeWhilePlaying"), &t("set.freeWhilePlayingDesc"), flag("freeWhilePlaying"), "freeWhilePlaying"));
    out.push(action_line(
        "games-actions",
        vec![btn(t("games.add"), "plus", "add-game"), btn(t("set.refreshInfo"), "refresh", "clear-metadata")],
        &status(true, st.get("gameInfoStatus")),
    ));

    // Library
    out.push(section(&t("set.library")));
    let libs = arr(set, "libraries");
    for (i, l) in libs.iter().enumerate() {
        out.push(Line {
            item: SetItem {
                kind: "lib".into(),
                name: s(l, "path").into(),
                tag: (if s(l, "type") == "tv" { t("lib.tv") } else { t("tab.movies") }).into(),
                ..Default::default()
            },
            key: format!("lib-{i}"),
            acts: vec![format!("library-menu:{i}")],
        });
    }
    if libs.is_empty() {
        out.push(text_line("empty", &t("lib.noFolders")));
    }
    out.push(action_line(
        "lib-actions",
        vec![
            btn(t("lib.addMovies"), "plus", "add-library:movies"),
            btn(t("lib.addTv"), "plus", "add-library:tv"),
            btn(if b(st, "scanning") { t("status.scanning") } else { t("lib.rescan") }, "refresh", "rescan"),
        ],
        "",
    ));
    let shows = arr(lib, "shows");
    let episodes: usize = shows.iter().map(|x| arr(x, "episodes").len()).sum();
    out.push(text_line(
        "stats",
        &tv("lib.stats", &[("movies", arr(lib, "movies").len().into()), ("shows", shows.len().into()), ("episodes", episodes.into()), ("games", games.len().into())]),
    ));

    // Playback
    out.push(section(&t("set.playback")));
    let vlc_path = s(set, "vlcPath");
    let vlc_value = if !vlc_path.is_empty() {
        vlc_path.to_string()
    } else if let Some(p) = vlc_found {
        tv("set.autoPath", &[("path", p.into())])
    } else {
        t("set.notFound")
    };
    out.push(value_row(&t("set.vlc"), &t("set.vlcDesc"), &vlc_value, "vlc-menu", "vlc"));
    out.push(toggle_row(&t("set.vlcFullscreen"), &t("set.vlcFullscreenDesc"), flag("vlcFullscreen"), "vlcFullscreen"));
    out.push(toggle_row(&t("set.autoplay"), &t("set.autoplayDesc"), flag("autoplayNext"), "autoplayNext"));
    let or = |k: &str, d: &'static str| -> String {
        let v = s(set, k);
        if v.is_empty() {
            d.to_string()
        } else {
            v.to_string()
        }
    };
    out.push(value_row(&t("set.audioLang"), &t("set.audioLangDesc"), &t(&format!("lang.{}", or("audioLanguage", "original"))), "media-language:audio", "audio-lang"));
    out.push(value_row(&t("set.subLang"), &t("set.subLangDesc"), &t(&format!("lang.{}", or("subLanguage", "off"))), "media-language:subtitles", "sub-lang"));
    let args = s(set, "vlcExtraArgs");
    out.push(value_row(&t("set.vlcArgs"), &t("set.vlcArgsDesc"), &if args.is_empty() { t("set.none") } else { args.to_string() }, "edit-args", "vlc-args"));
    out.push(value_row(&t("set.tmdb"), &t("set.tmdbDesc"), &mask(s(set, "tmdbKey")), "edit-key:tmdbKey", "tmdb"));
    out.push(value_row(&t("set.refreshMedia"), &t("set.refreshMediaDesc"), &status(!s(set, "tmdbKey").is_empty(), st.get("metaStatus")), "clear-metadata", "meta-refresh"));

    // Controls
    out.push(section(&t("set.controls")));
    out.push(toggle_row(&t("set.haptics"), &t("set.hapticsDesc"), flag("haptics"), "haptics"));
    out.push(toggle_row(&t("set.sounds"), &t("set.soundsDesc"), flag("sounds"), "sounds"));
    let scale = set.get("uiScale").and_then(Value::as_f64).filter(|x| *x > 0.0).unwrap_or(1.0);
    out.push(value_row(&t("set.textSize"), &t("set.textSizeDesc"), &format!("{}%", (scale * 100.0).round() as i64), "cycle-scale", "scale"));
    let anim = if s(set, "animations") == "reduced" { t("set.animReduced") } else { t("set.animFull") };
    out.push(value_row(&t("set.animations"), &t("set.animationsDesc"), &anim, "cycle-animations", "anim"));
    out.push(value_row(&t("pad.title"), &t("pad.settingDesc"), "", "open-padtest", "padtest"));
    out.push(text_line("help", ""));

    // Fullscreen experience
    out.push(section(&t("set.fse")));
    let mut fse_btns = Vec::new();
    if win {
        fse_btns.push(btn(t("set.fseOpenSettings"), "gamepad", "open-gaming-settings"));
    }
    if !fse_package {
        fse_btns.push(btn(t("set.fseGetPackage"), "link", "open-releases"));
    }
    let (btns, acts) = buttons(fse_btns);
    out.push(Line {
        item: SetItem {
            kind: "fse".into(),
            name: (if fse_package { t("set.fseInstalled") } else { t("set.fseNotInstalled") }).into(),
            desc: (if fse_package { t("set.fseHowTo") } else { t("set.fseInstallHowTo") }).into(),
            on: fse_package,
            buttons: model::model(btns),
            ..Default::default()
        },
        key: "fse".into(),
        acts,
    });
    out.push(toggle_row(&t("set.fullscreen"), &t("set.fullscreenDesc"), flag("startFullscreen"), "startFullscreen"));
    if !fse_package {
        out.push(toggle_row(&t("set.login"), &t("set.loginDesc"), flag("launchAtLogin"), "launchAtLogin"));
    }

    // Updates
    out.push(section(&t("upd.section")));
    let up_status = s(up, "status");
    if up.is_null() || up_status == "unsupported" {
        out.push(text_line("card", &tv("upd.unsupported", &[("version", version.into())])));
    } else {
        let status_text = match up_status {
            "idle" => t("upd.idle"),
            "checking" => t("upd.checking"),
            "uptodate" => t("upd.upToDate"),
            "available" => tv("upd.availableShort", &[("version", s(up, "version").into())]),
            "downloading" => tv("upd.downloading", &[("n", up.get("progress").and_then(Value::as_f64).unwrap_or(0.0).into())]),
            "ready" | "installing" => t("upd.installing"),
            "elevating" => t("upd.elevating"),
            "error" => update::error_text(s(up, "error")),
            _ => String::new(),
        };
        let busy = matches!(up_status, "checking" | "downloading" | "elevating" | "installing" | "ready");
        let act = if busy { "" } else if up_status == "available" { "install-update" } else { "check-update" };
        out.push(value_row(&t("upd.version"), &status_text, &format!("Lounge {version}"), act, "upd-row"));
        let mut list = Vec::new();
        if up_status == "available" {
            list.push(B { primary: true, ..btn(t("upd.now"), "refresh", "install-update") });
        }
        list.push(B { disabled: busy, ..btn(t("upd.checkNow"), "search", "check-update") });
        out.push(action_line("upd-actions", list, ""));
        out.push(toggle_row(&t("upd.auto"), &t("upd.autoDesc"), flag("autoCheckUpdates"), "autoCheckUpdates"));
    }

    // System
    out.push(section(&t("set.system")));
    out.push(action_line(
        "system",
        vec![
            btn(t("qm.desktop"), "desktop", "minimize"),
            btn(t("set.toggleFullscreen"), "", "fullscreen"),
            B { danger: true, ..btn(t("qm.quit"), "exit", "quit") },
        ],
        "",
    ));
    let mut about = format!("Lounge {version}");
    if !s(set, "tmdbKey").is_empty() {
        about.push('\n');
        about.push_str(&t("about.tmdb"));
    }
    about.push('\n');
    about.push_str(&t("about.steam"));
    if !s(set, "sgdbKey").is_empty() {
        about.push(' ');
        about.push_str(&t("about.sgdb"));
    }
    out.push(text_line("about", &about));
    out
}

fn rebuild() {
    let ctx = cx();
    let lines = PAGE.with(|p| {
        let p = p.borrow();
        build(&ctx.state.borrow(), p.vlc_found.as_deref(), &p.update)
    });
    let ui = ctx.ui();
    let page = ui.global::<SettingsPage>();
    page.set_items(model::model(lines.iter().map(|l| l.item.clone()).collect()));
    PAGE.with(|p| p.borrow_mut().lines = lines);
    // Keep focus on the same line; if it's gone (a library removed), on the nearest one.
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.lines.iter().any(|l| l.key == p.focus_key && stops_of(l).contains(&p.col)) {
            return;
        }
        let prev_line = page.get_line().max(0) as usize;
        let stops: Vec<usize> = (0..p.lines.len()).filter(|&i| !stops_of(&p.lines[i]).is_empty()).collect();
        let target = stops.iter().copied().rfind(|&i| i <= prev_line).or_else(|| stops.first().copied());
        if let Some(i) = target {
            let same_line = p.lines[i].key == p.focus_key;
            let cols = stops_of(&p.lines[i]);
            let col = if same_line { cols.iter().copied().rfind(|&c| c <= p.col).unwrap_or(cols[0]) } else { cols[0] };
            p.focus_key = p.lines[i].key.clone();
            p.col = col;
        }
    });
    push_focus();
}

// ---------------------------------------------------------------- Focus

/// The focusable columns of a line: 0 for a row (even a busy one that does nothing, like the renderer's
/// `noop` row), the enabled buttons of an actions line.
fn stops_of(l: &Line) -> Vec<usize> {
    match l.item.kind.as_str() {
        "toggle" | "value" | "lib" => vec![0],
        _ => l.acts.iter().enumerate().filter(|(_, a)| !a.is_empty()).map(|(i, _)| i).collect(),
    }
}

fn push_focus() {
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<SettingsPage>();
    PAGE.with(|p| {
        let p = p.borrow();
        let line = p.lines.iter().position(|l| l.key == p.focus_key && !l.key.is_empty()).map(|i| i as i32).unwrap_or(-1);
        page.set_line(line);
        page.set_col(p.col as i32);
    });
}

fn move_focus(dir: &str) {
    enum Out {
        Moved,
        Edge,
        Top,
    }
    let out = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let stops: Vec<usize> = (0..p.lines.len()).filter(|&i| !stops_of(&p.lines[i]).is_empty()).collect();
        if stops.is_empty() {
            return Out::Edge;
        }
        let cur = p.lines.iter().position(|l| l.key == p.focus_key && !l.key.is_empty());
        let Some(cur) = cur else {
            let first = stops[0];
            p.focus_key = p.lines[first].key.clone();
            p.col = stops_of(&p.lines[first])[0];
            return Out::Moved;
        };
        let at = stops.iter().position(|&i| i == cur).unwrap_or(0);
        let enter = |p: &mut Page, i: usize| {
            let cols = stops_of(&p.lines[i]);
            let key = p.lines[i].key.clone();
            let col = p.memory.get(&key).copied().filter(|c| cols.contains(c)).unwrap_or(cols[0]);
            p.focus_key = key;
            p.col = col;
        };
        match dir {
            "up" | "down" => {
                let next = if dir == "up" { at.checked_sub(1) } else { Some(at + 1).filter(|&n| n < stops.len()) };
                match next {
                    Some(n) => {
                        enter(&mut p, stops[n]);
                        Out::Moved
                    }
                    None if dir == "up" => Out::Top,
                    None => Out::Edge,
                }
            }
            "pgup" | "pgdn" => {
                // About a screen's worth of lines.
                let n = if dir == "pgup" { at.saturating_sub(6) } else { (at + 6).min(stops.len() - 1) };
                if n == at {
                    return Out::Edge;
                }
                enter(&mut p, stops[n]);
                Out::Moved
            }
            _ => {
                let cols = stops_of(&p.lines[cur]);
                let i = cols.iter().position(|&c| c == p.col).unwrap_or(0);
                let next = if dir == "left" { i.checked_sub(1) } else { Some(i + 1).filter(|&n| n < cols.len()) };
                match next {
                    Some(n) => {
                        p.col = cols[n];
                        let key = p.focus_key.clone();
                        let col = p.col;
                        p.memory.insert(key, col);
                        Out::Moved
                    }
                    None => Out::Edge,
                }
            }
        }
    });
    match out {
        Out::Moved => {
            push_focus();
            input::feedback("move");
        }
        Out::Edge => input::feedback("edge"),
        Out::Top => {
            input::feedback("move");
            cx().ui().global::<Nav>().invoke_focus_top();
        }
    }
}

// ---------------------------------------------------------------- Actions

fn run(act: &str) {
    let (name, arg) = act.split_once(':').unwrap_or((act, ""));
    match name {
        "toggle" => toggle(arg),
        "choose-language" => choose_ui_language(),
        "pick-steam" => pick_steam(),
        "edit-key" => edit_key(arg),
        "edit-args" => edit_args(),
        "add-game" => actions::add_game(),
        "clear-metadata" => {
            cx().send("clear_metadata", |b| b.clear_metadata());
            toasts::toast(&t("set.refreshing"), "info");
        }
        "library-menu" => library_menu(arg.parse().unwrap_or(0)),
        "add-library" => add_library(arg),
        "rescan" => cx().send("rescan", |b| {
            b.rescan();
        }),
        "vlc-menu" => vlc_menu(),
        "media-language" => media_language(arg),
        "cycle-scale" => {
            const STEPS: [f64; 5] = [0.9, 1.0, 1.15, 1.3, 1.5];
            let cur = cx().setting("uiScale").as_f64().filter(|x| *x > 0.0).unwrap_or(1.0);
            let i = STEPS.iter().position(|x| (x - cur).abs() < 0.01).map(|i| i as i64).unwrap_or(-1);
            save(json!({ "uiScale": STEPS[((i + 1) as usize) % STEPS.len()] }));
        }
        "cycle-animations" => {
            let reduced = cx().setting("animations").as_str() == Some("reduced");
            save(json!({ "animations": if reduced { "full" } else { "reduced" } }));
        }
        "open-padtest" => router::go("padtest", ""),
        "check-update" => update::check(),
        "install-update" => update::start(),
        "open-gaming-settings" => cx().send("open_external", |b| b.open_external("ms-settings:gaming-gamebar")),
        "open-releases" => cx().send("open_external", |b| b.open_external("https://github.com/thomasxd24/vlc-fse/releases/latest")),
        "minimize" => cx().send("minimize", |b| b.minimize()),
        "fullscreen" => cx().send("toggle_fullscreen", |b| b.toggle_fullscreen()),
        "quit" => {
            let ctx = cx();
            if ctx.demo() {
                let _ = slint::quit_event_loop();
            } else {
                ctx.send("quit", |b| b.quit());
            }
        }
        _ => {}
    }
}

/// The renderer's `save(patch)`: the backend stores it and pushes a new `state`, which re-applies scale,
/// motion, sounds, rumble and language (`sys`) and rebuilds this page. In demo mode the patch is applied
/// to the demo state instead, so the page still reacts.
fn save(patch: Value) {
    let Value::Object(patch) = patch else { return };
    let ctx = cx();
    if ctx.demo() {
        {
            let mut st = ctx.state.borrow_mut();
            if let Some(lang) = patch.get("uiLanguage").and_then(Value::as_str) {
                st["lang"] = json!(if lang == "fr" { "fr" } else { "en" });
            }
            if let Some(Value::Object(set)) = st.get_mut("settings") {
                for (k, v) in &patch {
                    set.insert(k.clone(), v.clone());
                }
            }
        }
        ctx.refresh();
        return;
    }
    ctx.send("save_settings", move |b| {
        b.save_settings(patch);
    });
}

fn toggle(setting: &str) {
    let cur = cx().setting(setting);
    let on = match setting {
        "quietSteam" => cur != Value::Bool(false),
        _ => cur.as_bool().unwrap_or(false),
    };
    save(json!({ setting: !on }));
}

/// The renderer's `editKey(setting)`.
fn edit_key(setting: &str) {
    let setting = setting.to_string();
    let current = cx().setting(&setting).as_str().unwrap_or("").to_string();
    dialogs::prompt(
        Prompt { title: t(if setting == "sgdbKey" { "set.sgdb" } else { "set.tmdb" }), value: current, placeholder: t("set.pasteKey"), ..Default::default() },
        move |v| {
            let Some(v) = v else { return };
            save(json!({ setting: v.trim() }));
            toasts::toast(&t("toast.saved"), "info");
        },
    );
}

fn edit_args() {
    let current = cx().setting("vlcExtraArgs").as_str().unwrap_or("").to_string();
    dialogs::prompt(Prompt { title: t("set.vlcArgs"), value: current, placeholder: "--sub-language=fre".into(), symbols: true, ..Default::default() }, |v| {
        if let Some(v) = v {
            save(json!({ "vlcExtraArgs": v.trim() }));
        }
    });
}

/// The interface language (Automatic / English / Français). Strings switch live once the backend's new
/// `state` arrives (`sys::apply_state` bumps `T.rev`; this page rebuilds its Rust-made strings).
fn choose_ui_language() {
    let cur = cx().setting("uiLanguage").as_str().unwrap_or("auto").to_string();
    let opt = |label: &str, v: &str| Choice { checked: cur == v, ..choice(label, v) };
    dialogs::choose(&t("set.language"), "", vec![opt(&t("set.langAuto"), "auto"), opt("English", "en"), opt("Français", "fr")], |v| {
        if let Some(v) = v {
            save(json!({ "uiLanguage": v }));
        }
    });
}

fn media_language(kind: &str) {
    let setting = if kind == "audio" { "audioLanguage" } else { "subLanguage" };
    let current = cx().setting(setting).as_str().unwrap_or("").to_string();
    actions::choose_language(kind, &current, vec![], move |v| {
        if let Some(v) = v {
            save(json!({ setting: v }));
        }
    });
}

fn pick_steam() {
    let path = cx().setting("steamPath").as_str().unwrap_or("").to_string();
    dialogs::choose(
        &t("set.steamPath"),
        &if path.is_empty() { t("set.auto") } else { path },
        vec![Choice { primary: true, ..choice(&t("set.browse"), "browse") }, choice(&t("set.auto"), "auto"), choice(&t("common.cancel"), "")],
        |v| match v.as_deref() {
            Some("browse") => cx().call("pick_folder", |b| b.pick_folder(), |p| {
                if let Some(p) = p {
                    save(json!({ "steamPath": p }));
                }
            }),
            Some("auto") => save(json!({ "steamPath": "" })),
            _ => {}
        },
    );
}

fn vlc_menu() {
    let path = cx().setting("vlcPath").as_str().unwrap_or("").to_string();
    let found = PAGE.with(|p| p.borrow().vlc_found.clone());
    let text = if !path.is_empty() {
        path
    } else if let Some(f) = found {
        tv("set.autoPath", &[("path", f.into())])
    } else {
        t("welcome.vlcMissing")
    };
    dialogs::choose(
        &t("set.vlc"),
        &text,
        vec![Choice { primary: true, ..choice(&t("set.vlcBrowse"), "browse") }, choice(&t("set.vlcDetect"), "auto"), choice(&t("common.cancel"), "")],
        |v| match v.as_deref() {
            Some("browse") => cx().call("pick_vlc", |b| b.pick_vlc(), |p| {
                if let Some(p) = p {
                    save(json!({ "vlcPath": p }));
                }
            }),
            Some("auto") => {
                save(json!({ "vlcPath": "" }));
                detect_vlc(true);
            }
            _ => {}
        },
    );
}

/// The renderer's `S.vlcFound = await api.detectVlc()`; `announce` toasts the result (VLC › Detect).
fn detect_vlc(announce: bool) {
    let ctx = cx();
    if ctx.demo() {
        PAGE.with(|p| p.borrow_mut().vlc_found = Some(r"C:\Program Files\VideoLAN\VLC\vlc.exe".into()));
        return;
    }
    ctx.call("detect_vlc", |b| b.detect_vlc(), move |found| {
        if announce {
            match &found {
                Some(p) => toasts::toast(&tv("set.autoPath", &[("path", p.into())]), "info"),
                None => toasts::toast(&t("set.notFound"), "error"),
            }
        }
        let changed = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let changed = p.vlc_found != found;
            p.vlc_found = found;
            changed
        });
        if changed {
            rebuild();
        }
    });
}

/// The renderer's `addLibrary(type)`: pick a folder, add it unless it's already there.
pub fn add_library(kind: &str) {
    let kind = kind.to_string();
    cx().call("pick_folder", |b| b.pick_folder(), move |dir| {
        let Some(dir) = dir else { return };
        let mut libs: Vec<Value> = cx().setting("libraries").as_array().cloned().unwrap_or_default();
        if libs.iter().any(|l| s(l, "path") == dir) {
            toasts::toast(&t("lib.already"), "info");
            return;
        }
        libs.push(json!({ "path": dir, "type": kind }));
        save(json!({ "libraries": libs }));
        toasts::toast(&tv("lib.added", &[("path", dir.as_str().into())]), "info");
    });
}

fn library_menu(index: usize) {
    let libs: Vec<Value> = cx().setting("libraries").as_array().cloned().unwrap_or_default();
    let Some(lib) = libs.get(index).cloned() else { return };
    let tv_folder = s(&lib, "type") == "tv";
    dialogs::choose(
        s(&lib, "path"),
        &t(if tv_folder { "lib.tvFolder" } else { "lib.moviesFolder" }),
        vec![
            choice(&t(if tv_folder { "lib.treatAsMovies" } else { "lib.treatAsTv" }), "type"),
            Choice { danger: true, icon: "trash".into(), ..choice(&t("lib.remove"), "remove") },
            choice(&t("common.cancel"), ""),
        ],
        move |v| {
            let mut libs = libs;
            match v.as_deref() {
                Some("remove") => {
                    libs.remove(index);
                }
                Some("type") => {
                    libs[index]["type"] = json!(if tv_folder { "movies" } else { "tv" });
                }
                _ => return,
            }
            save(json!({ "libraries": libs }));
        },
    );
}
