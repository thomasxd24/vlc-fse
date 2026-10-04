//! Apps (the renderer's VIEWS.apps / appTile, and openApp plus the `app:` options menu from app.js).

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, choice, Choice};
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, b, n, s};
use crate::ui::pages::remote::err_text;
use crate::ui::toasts::toast;
use crate::ui::{fmt, prefs};
use crate::{AppTile, AppsPage, Backdrop, GridChip};
use serde_json::Value;
use slint::ComponentHandle;

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<AppsPage>();
    page.on_chip_clicked(|value| chip(&value));
    page.on_open(|i| {
        if let Some(a) = items().get(i as usize) {
            open_app(s(a, "id"));
        }
    });
    page.on_options(|i| {
        if let Some(a) = items().get(i as usize) {
            options(s(a, "id"));
        }
    });
    ctx.on_event("state", |_, _| rebuild());
    ctx.on_event("route", |ctx, r| {
        if s(r, "name") == "apps" {
            ctx.ui().global::<Backdrop>().set_src("".into());
        }
    });
}

fn apps() -> Vec<Value> {
    cx().state.borrow().get("apps").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// The apps as shown (filtered and sorted), so indices from Slint map straight back.
fn items() -> Vec<Value> {
    let hidden = prefs::get("appFilter") == "hidden";
    let mut list: Vec<Value> = apps().into_iter().filter(|a| b(a, "hidden") == hidden).collect();
    if prefs::get("appSort") == "recent" {
        list.sort_by(|a, c| {
            n(c, "lastLaunched").partial_cmp(&n(a, "lastLaunched")).unwrap_or(std::cmp::Ordering::Equal).then_with(|| model::title_cmp(s(a, "name"), s(c, "name")))
        });
    } else {
        list.sort_by(|a, c| model::title_cmp(s(a, "name"), s(c, "name")));
    }
    list
}

fn chip(value: &str) {
    match value.split_once(':') {
        Some(("act", "rescan")) => return cx().send("rescan_apps", |b| b.rescan_apps()),
        Some((group @ ("appSort" | "appFilter"), v)) => prefs::set(group, v),
        _ => return,
    }
    rebuild();
}

fn chips(group: &str, options: &[(&str, &str)], sep: bool) -> Vec<GridChip> {
    let current = prefs::get(group);
    options
        .iter()
        .enumerate()
        .map(|(i, (value, key))| GridChip { label: t(key).into(), value: format!("{group}:{value}").into(), on: *value == current, sep_before: sep && i == 0, ..Default::default() })
        .collect()
}

fn rebuild() {
    let ctx = cx();
    let ui = ctx.ui();
    let page = ui.global::<AppsPage>();
    let all = apps();
    let list = items();
    let scanning = ctx.state.borrow().get("appsScanning").and_then(Value::as_bool).unwrap_or(false);
    let platform = ctx.state.borrow().get("platform").and_then(Value::as_str).unwrap_or("win32").to_string();
    let mut chip_list = chips("appSort", &[("recent", "sort.recentApps"), ("az", "sort.az")], false);
    chip_list.extend(chips("appFilter", &[("all", "filter.all"), ("hidden", "filter.hidden")], true));
    chip_list.push(GridChip {
        label: t(if scanning { "apps.scanning" } else { "apps.refresh" }).into(),
        icon: "refresh".into(),
        value: "act:rescan".into(),
        sep_before: true,
        ..Default::default()
    });
    let empty = if !list.is_empty() {
        String::new()
    } else if platform != "win32" {
        t("apps.windowsOnly")
    } else if scanning {
        t("apps.scanning")
    } else if !all.is_empty() {
        t("grid.empty")
    } else {
        t("apps.empty")
    };
    page.set_count(tv("apps.count", &[("n", list.len().into())]).into());
    page.set_chips(model::model(chip_list));
    page.set_empty(empty.into());
    page.set_apps(model::model(list.iter().map(tile).collect()));
}

fn tile(a: &Value) -> AppTile {
    let name = s(a, "name");
    let h = fmt::hue_of(name) as f32;
    AppTile {
        id: s(a, "id").into(),
        name: name.into(),
        icon: s(a, "icon").into(),
        letter: name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
        tint_a: fmt::hsl(h, 0.30, 0.24),
        tint_b: fmt::hsl(h + 30.0, 0.35, 0.11),
        hidden: b(a, "hidden"),
    }
}

/// Launch an app (the renderer's `openApp`): toast "Opening…", then the backend's launch_app.
/// Home's "your apps" row calls this too.
pub fn open_app(id: &str) {
    let Some(app) = apps().into_iter().find(|a| s(a, "id") == id) else { return };
    toast(&tv("apps.opening", &[("name", s(&app, "name").into())]), "info");
    let id = id.to_string();
    cx().call("launch_app", move |b| b.launch_app(&id), |r| {
        if !b(&r, "ok") {
            toast(&err_text(&r), "error");
        }
    });
}

/// The options menu for an app (X / right-click / long-press): Open, Hide / Unhide.
pub fn options(id: &str) {
    let Some(app) = apps().into_iter().find(|a| s(a, "id") == id) else { return };
    let hidden = b(&app, "hidden");
    let id = id.to_string();
    dialogs::choose(
        s(&app, "name"),
        "",
        vec![
            Choice { icon: "play".into(), primary: true, ..choice(&t("apps.open"), "open") },
            Choice { icon: if hidden { "show" } else { "hide" }.into(), ..choice(&t(if hidden { "opt.unhide" } else { "opt.hide" }), "hide") },
        ],
        move |v| match v.as_deref() {
            Some("open") => open_app(&id),
            Some("hide") => cx().call("hide_app", move |b| b.hide_app(&id, !hidden), move |_| toast(&t(if hidden { "apps.unhidden" } else { "apps.hidden" }), "info")),
            _ => {}
        },
    );
}
