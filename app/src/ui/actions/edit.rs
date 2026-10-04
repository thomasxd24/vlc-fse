//! Adding, editing and removing games — the renderer's `addGame`, `editGame`, `matchSteam`,
//! `pickArtwork` (with its artwork picker modal), `removeGame` — and the screenshot lightbox
//! (`showScreenshot`). The two modals are drawn by `GameModals` (app/ui/pages/game.slint).

use super::{ch, choose, error_toast, ok, primary, prompt};
use crate::ui::ctx::cx;
use crate::ui::dialogs::{choice, Choice, Prompt};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, arr, s};
use crate::ui::router;
use crate::ui::toasts::toast;
use crate::{ArtPicker, GameDetail, Lightbox, Nav};
use serde_json::{json, Map, Value};
use slint::ComponentHandle;
use std::cell::RefCell;

type Done = Box<dyn FnOnce()>;

struct PickerState {
    id: String,
    key: &'static str,
    urls: Vec<String>,
    then: Option<Done>,
}

struct LightboxState {
    game: String,
    index: usize,
    shots: Vec<(String, String)>, // (thumb, full)
}

thread_local! {
    static PICKER: RefCell<Option<PickerState>> = const { RefCell::new(None) };
    static LIGHTBOX: RefCell<Option<LightboxState>> = const { RefCell::new(None) };
    // Where keyboard focus was when a modal opened ("page" | "top"), to hand it back on close.
    static ZONE: RefCell<String> = RefCell::new("page".into());
}

pub fn install() {
    let ctx = cx();
    let ui = ctx.ui();
    let g = ui.global::<GameDetail>();
    g.on_picker_done(|i| picker_done(usize::try_from(i).ok()));
    g.on_lightbox_step(|d| lightbox_step(d as isize));
    g.on_lightbox_close(close_lightbox);
    // Back (pointer back button, or the window's Back when something else had focus) closes them.
    crate::ui::on_close_overlay(|| {
        if PICKER.with(|p| p.borrow().is_some()) {
            picker_done(None);
            return true;
        }
        if LIGHTBOX.with(|l| l.borrow().is_some()) {
            close_lightbox();
            return true;
        }
        false
    });
}

fn remember_zone() {
    let ui = cx().ui();
    let zone = ui.global::<Nav>().get_zone().to_string();
    ZONE.with(|z| *z.borrow_mut() = zone);
}

fn restore_zone() {
    let ui = cx().ui();
    let nav = ui.global::<Nav>();
    if ZONE.with(|z| z.borrow().clone()) == "top" {
        nav.invoke_focus_top();
    } else {
        nav.invoke_focus_page();
    }
}

// ---------------------------------------------------------------------------------------- Add / remove

/// "Add a game": pick an executable, then open its page.
pub fn add_game() {
    cx().call("add_game", |b| b.add_game(), |r| {
        if !ok(&r) {
            error_toast(&r);
            return;
        }
        toast(&t("games.added"), "info");
        if let Some(id) = r.get("id").and_then(Value::as_str) {
            router::go("game", id);
        }
    });
}

/// Remove a manually added game, after a confirmation.
pub fn remove_game(id: &str) {
    let Some(g) = cx().find("games", id) else { return };
    let id = id.to_string();
    choose(
        &tv("opt.removeConfirm", &[("name", s(&g, "title").into())]),
        &t("opt.removeText"),
        vec![Choice { danger: true, ..ch(&t("opt.remove"), "remove", "trash") }, choice(&t("common.cancel"), "cancel")],
        move |v| {
            if v.as_deref() != Some("remove") {
                return;
            }
            cx().call("remove_game", move |b| b.remove_game(&id), |_| {
                if router::current().0 == "game" {
                    router::back();
                }
            });
        },
    );
}

// ---------------------------------------------------------------------------------------- Edit

/// The "Edit info" menu: rename, launch options (manual games), Steam match, artwork. Reopens after
/// each change until Done or Back.
pub fn edit_game(id: &str) {
    let Some(g) = cx().find("games", id) else { return };
    let mut choices = vec![
        ch(&t("edit.rename"), "rename", "edit"),
        ch(&t("edit.matchSteam"), "match", "steam"),
        ch(&t("edit.cover"), "grids", "image"),
        ch(&t("edit.background"), "heroes", "image"),
        ch(&t("edit.logo"), "logos", "image"),
    ];
    if s(&g, "source") == "manual" {
        choices.insert(1, ch(&t("edit.args"), "args", "gamepad"));
    }
    choices.push(primary(choice(&t("common.done"), "done")));
    let steam_id = match g.get("steamAppId") {
        Some(Value::String(x)) if !x.is_empty() => Some(x.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let text = match steam_id {
        Some(x) => tv("edit.matched", &[("id", x.into())]),
        None => t("edit.unmatched"),
    };
    let id = id.to_string();
    choose(&tv("edit.title", &[("name", s(&g, "title").into())]), &text, choices, move |v| {
        let Some(v) = v.filter(|v| v != "done") else { return };
        let again = {
            let id = id.clone();
            Box::new(move || edit_game(&id)) as Done
        };
        match v.as_str() {
            "rename" => prompt(Prompt { title: t("edit.rename"), value: s(&g, "title").to_string(), ..Default::default() }, move |name| {
                match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                    Some(name) => patch_game(&id, json!({ "title": name }), again),
                    None => again(),
                }
            }),
            "args" => prompt(
                Prompt { title: t("edit.args"), value: s(&g, "args").to_string(), placeholder: "-fullscreen".into(), symbols: true, ..Default::default() },
                move |args| match args {
                    Some(args) => patch_game(&id, json!({ "args": args }), again),
                    None => again(),
                },
            ),
            "match" => match_steam(&id, again),
            kind => pick_artwork(&id, kind, again),
        }
    });
}

fn patch_game(id: &str, patch: Value, then: Done) {
    let id = id.to_string();
    cx().call("edit_game", move |b| b.edit_game(&id, patch), move |_| then());
}

/// Search the Steam store and pick which game this is (or none).
fn match_steam(id: &str, then: Done) {
    let Some(g) = cx().find("games", id) else { return };
    let id = id.to_string();
    prompt(Prompt { title: t("edit.searchSteam"), value: s(&g, "title").to_string(), ok: t("common.search"), ..Default::default() }, move |term| {
        let Some(term) = term.map(|x| x.trim().to_string()).filter(|x| !x.is_empty()) else { return then() };
        toast(&t("edit.searching"), "info");
        cx().call("search_steam", move |b| b.search_steam(&term), move |hits| {
            let hits = hits.as_array().cloned().unwrap_or_default();
            let mut choices: Vec<Choice> = hits
                .iter()
                .take(8)
                .map(|x| {
                    let app = match x.get("steamAppId") {
                        Some(Value::String(a)) => a.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    choice(&format!("{}  ·  #{app}", s(x, "title")), &app)
                })
                .collect();
            choices.push(choice(&t("edit.noMatch"), "none"));
            let text = if hits.is_empty() { t("search.none") } else { String::new() };
            choose(&t("edit.pickMatch"), &text, choices, move |v| {
                let Some(v) = v else { return then() };
                let app = if v == "none" { Value::Null } else { json!(v) };
                patch_game(&id, json!({ "steamAppId": app }), then);
                toast(&t("edit.updating"), "info");
            });
        });
    });
}

/// Choose a cover / background / logo from SteamGridDB. kind: "grids" | "heroes" | "logos".
fn pick_artwork(id: &str, kind: &str, then: Done) {
    let ctx = cx();
    if ctx.setting("sgdbKey").as_str().unwrap_or("").is_empty() {
        choose(
            &t("edit.needSgdb"),
            &t("edit.needSgdbText"),
            vec![primary(choice(&t("edit.addKey"), "key")), choice(&t("edit.getKey"), "site"), choice(&t("common.cancel"), "cancel")],
            move |v| match v.as_deref() {
                Some("key") => edit_key("sgdbKey", then),
                Some("site") => {
                    cx().send("open_external", |b| b.open_external("https://www.steamgriddb.com/profile/preferences/api"));
                    then();
                }
                _ => then(),
            },
        );
        return;
    }
    let (shape, key, title) = match kind {
        "grids" => ("tall", "poster", t("edit.cover")),
        "heroes" => ("wide", "hero", t("edit.background")),
        _ => ("logo", "logo", t("edit.logo")),
    };
    toast(&t("edit.searching"), "info");
    let (id, kind) = (id.to_string(), kind.to_string());
    let id2 = id.clone();
    ctx.call("sgdb_images", move |b| b.sgdb_images(&kind, &id2), move |imgs| {
        let imgs = imgs.as_array().cloned().unwrap_or_default();
        if imgs.is_empty() {
            toast(&t("edit.noImages"), "error");
            return then();
        }
        open_picker(title, shape, key, id, &imgs, then);
    });
}

fn open_picker(title: String, shape: &str, key: &'static str, id: String, imgs: &[Value], then: Done) {
    remember_zone();
    let thumbs: Vec<slint::SharedString> = imgs.iter().map(|i| s(i, "thumb").into()).collect();
    let urls = imgs.iter().map(|i| s(i, "url").to_string()).collect();
    PICKER.with(|p| *p.borrow_mut() = Some(PickerState { id, key, urls, then: Some(then) }));
    let ui = cx().ui();
    let g = ui.global::<GameDetail>();
    g.set_picker_focus(0);
    g.set_picker(ArtPicker { open: true, title: title.into(), shape: shape.into(), images: model::model(thumbs) });
}

fn picker_done(index: Option<usize>) {
    let Some(st) = PICKER.with(|p| p.borrow_mut().take()) else { return };
    {
        let ui = cx().ui();
        ui.global::<GameDetail>().set_picker(ArtPicker::default());
    }
    restore_zone();
    let then = st.then.unwrap_or_else(|| Box::new(|| {}));
    let Some(url) = index.and_then(|i| st.urls.get(i).cloned()) else { return super::later(then) };
    let (id, key) = (st.id, st.key);
    cx().call("set_game_art", move |b| b.set_game_art(&id, key, &url), move |r| {
        if ok(&r) {
            toast(&t("edit.saved"), "info");
        } else {
            toast(&t("edit.downloadFailed"), "error");
        }
        then();
    });
}

/// Enter an API key (the renderer's editKey, used here for the SteamGridDB key).
fn edit_key(setting: &'static str, then: Done) {
    let current = cx().setting(setting).as_str().unwrap_or("").to_string();
    let title = t(if setting == "sgdbKey" { "set.sgdb" } else { "set.tmdb" });
    prompt(Prompt { title, value: current, placeholder: t("set.pasteKey"), symbols: true, ..Default::default() }, move |v| {
        let Some(v) = v else { return then() };
        let mut patch = Map::new();
        patch.insert(setting.to_string(), json!(v.trim()));
        cx().call("save_settings", move |b| b.save_settings(patch), move |_| {
            toast(&t("toast.saved"), "info");
            then();
        });
    });
}

// ---------------------------------------------------------------------------------------- Lightbox

/// Show a game's screenshots full size, starting at `index`.
pub fn show_screenshot(game_id: &str, index: usize) {
    let Some(g) = cx().find("games", game_id) else { return };
    let shots: Vec<(String, String)> = arr(&g, "screenshots").iter().map(|x| (s(x, "thumb").to_string(), s(x, "full").to_string())).collect();
    if shots.is_empty() {
        return;
    }
    remember_zone();
    let index = index.min(shots.len() - 1);
    LIGHTBOX.with(|l| *l.borrow_mut() = Some(LightboxState { game: game_id.to_string(), index, shots }));
    {
        let ui = cx().ui();
        ui.global::<GameDetail>().set_lightbox_focus(1);
    }
    paint_lightbox();
}

fn lightbox_step(d: isize) {
    LIGHTBOX.with(|l| {
        if let Some(st) = l.borrow_mut().as_mut() {
            let n = st.shots.len() as isize;
            st.index = ((st.index as isize + d).rem_euclid(n)) as usize;
        }
    });
    paint_lightbox();
}

fn close_lightbox() {
    if LIGHTBOX.with(|l| l.borrow_mut().take()).is_none() {
        return;
    }
    {
        let ui = cx().ui();
        ui.global::<GameDetail>().set_lightbox(Lightbox::default());
    }
    restore_zone();
}

/// The thumbnail at once, then the full-size image once it's downloaded (if still on that one).
fn paint_lightbox() {
    let Some((game, index, total, thumb, full)) = LIGHTBOX.with(|l| {
        l.borrow().as_ref().map(|st| {
            let (thumb, full) = st.shots[st.index].clone();
            (st.game.clone(), st.index, st.shots.len(), thumb, full)
        })
    }) else {
        return;
    };
    let set = move |image: &str| {
        let ui = cx().ui();
        ui.global::<GameDetail>().set_lightbox(Lightbox { open: true, image: image.into(), count: format!("{} / {total}", index + 1).into() });
    };
    set(&thumb);
    if full.is_empty() {
        return;
    }
    cx().call("screenshot", move |b| b.screenshot(&full), move |path| {
        let Some(path) = path.as_str().filter(|p| !p.is_empty()) else { return };
        let current = LIGHTBOX.with(|l| l.borrow().as_ref().map(|st| st.game == game && st.index == index).unwrap_or(false));
        if current {
            set(path);
        }
    });
}
