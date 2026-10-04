//! Welcome (the renderer's VIEWS.welcome): the first-run page, plus `add_library` (the renderer's
//! `addLibrary`: pick a folder, add it to settings.libraries, toast) which Settings can reuse.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{arr, b, s};
use crate::ui::{actions, router, toasts};
use crate::WelcomePage;
use serde_json::{json, Map, Value};
use slint::ComponentHandle;

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<WelcomePage>();
    page.on_act(|a| match a.as_str() {
        "add-game" => actions::add_game(),
        "add-movies" => add_library("movies"),
        "add-tv" => add_library("tv"),
        "settings" => {
            router::switch_tab("settings");
            cx().ui().global::<crate::Nav>().invoke_focus_page();
        }
        _ => {}
    });
    ctx.on_event("state", |ctx, st| {
        let ui = ctx.ui();
        ui.global::<WelcomePage>().set_steam_found(st.pointer("/library/steamFound").and_then(Value::as_bool).unwrap_or(false));
        // The first scan found something (or a folder was just added): on to Home.
        if router::current().0 == "welcome" && has_items(st) {
            router::reset("home");
        }
    });
    ctx.on_event("route", |ctx, r| {
        if s(r, "name") != "welcome" {
            return;
        }
        let ui = ctx.ui();
        ui.global::<WelcomePage>().set_focus(0);
        // The renderer asks for VLC once at startup (`S.vlcFound = await api.detectVlc()`).
        if ctx.demo() {
            ui.global::<WelcomePage>().set_vlc_path("C:\\Program Files\\VideoLAN\\VLC\\vlc.exe".into());
        } else {
            ctx.call("detect_vlc", |b| b.detect_vlc(), |p| {
                cx().ui().global::<WelcomePage>().set_vlc_path(p.unwrap_or_default().into());
            });
        }
    });
}

/// Any visible game, movie or show in the library.
fn has_items(st: &Value) -> bool {
    let lib = st.get("library").cloned().unwrap_or(Value::Null);
    ["games", "movies", "shows"].iter().any(|k| arr(&lib, k).iter().any(|x| !b(x, "hidden")))
}

/// Pick a folder and add it as a library of `kind` ("movies" | "tv"), then rescan (the backend does that
/// when `libraries` changes).
pub fn add_library(kind: &'static str) {
    cx().call("pick_folder", |b| b.pick_folder(), move |dir| {
        let Some(dir) = dir else { return };
        let ctx = cx();
        let mut libs = ctx.setting("libraries").as_array().cloned().unwrap_or_default();
        if libs.iter().any(|l| s(l, "path") == dir) {
            toasts::toast(&t("lib.already"), "info");
            return;
        }
        libs.push(json!({ "path": dir, "type": kind }));
        let mut patch = Map::new();
        patch.insert("libraries".into(), Value::Array(libs));
        ctx.call("save_settings", move |b| b.save_settings(patch), move |_| {
            toasts::toast(&tv("lib.added", &[("path", dir.as_str().into())]), "info");
        });
    });
}
