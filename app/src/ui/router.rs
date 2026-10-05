//! Navigation: a stack of routes whose bottom is a tab (Home, Games, Movies, TV Shows, Apps) or a tool
//! page (Search, Transfers, Settings). Detail pages push onto it; Back pops, then goes Home, then asks
//! to exit — the renderer's `go` / `switchTab` / `back`.
//!
//! Screens that need to load data when they're shown subscribe to the pseudo-event `route` (payload
//! `{ name, id }`) with `ctx.on_event("route", …)`; it fires after `Nav.route` changes.

use super::ctx::{cx, Ctx};
use super::dialogs::{self, choice, Choice};
use super::i18n::t;
use crate::{Nav, Route};
use serde_json::json;
use slint::ComponentHandle;
use std::cell::RefCell;

pub const TABS: [&str; 5] = ["home", "games", "movies", "shows", "apps"];
pub const TOOLS: [&str; 3] = ["search", "transfers", "settings"];

thread_local! {
    static STACK: RefCell<Vec<(String, String)>> = RefCell::new(vec![("home".into(), String::new())]);
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let nav = ui.global::<Nav>();
    nav.on_go(|name, id| go(&name, &id));
    nav.on_back(back);
    nav.on_switch_tab(|name| switch_tab(&name));
    nav.on_cycle_tab(cycle_tab);
    nav.on_page_key(|k| page_key(&k));
}

pub fn current() -> (String, String) {
    STACK.with(|s| s.borrow().last().cloned().unwrap_or_default())
}

pub fn root() -> String {
    STACK.with(|s| s.borrow().first().map(|r| r.0.clone()).unwrap_or_default())
}

pub fn depth() -> usize {
    STACK.with(|s| s.borrow().len())
}

/// Push a page (a detail page, the stats page…).
pub fn go(name: &str, id: &str) {
    STACK.with(|s| s.borrow_mut().push((name.to_string(), id.to_string())));
    super::input::feedback("open");
    apply(true);
}

/// Replace the stack with a tab or tool page.
pub fn switch_tab(name: &str) {
    let same = STACK.with(|s| {
        let s = s.borrow();
        s.len() == 1 && s[0].0 == name
    });
    if same {
        return;
    }
    STACK.with(|s| *s.borrow_mut() = vec![(name.to_string(), String::new())]);
    apply(true);
}

/// Replace the whole stack (e.g. after a scan finds the first library, Welcome → Home).
pub fn reset(name: &str) {
    STACK.with(|s| *s.borrow_mut() = vec![(name.to_string(), String::new())]);
    apply(true);
}

/// LB / RB: the previous / next tab (tool pages count as sitting after the tabs).
pub fn cycle_tab(dir: i32) {
    let r = root();
    let i = TABS.iter().position(|t| *t == r).map(|i| i as i32).unwrap_or(if dir > 0 { -1 } else { TABS.len() as i32 });
    let next = i + dir;
    if next < 0 || next >= TABS.len() as i32 {
        return;
    }
    super::input::feedback("move");
    switch_tab(TABS[next as usize]);
    cx().ui().global::<Nav>().invoke_focus_page();
}

pub fn back() {
    let ctx = cx();
    // Modals and overlays first (pointer back buttons land here even while they're open).
    if dialogs::cancel_top() {
        return;
    }
    if super::overlays_close_top() {
        return;
    }
    if depth() > 1 {
        STACK.with(|s| {
            s.borrow_mut().pop();
        });
        super::input::feedback("back");
        apply(false);
        return;
    }
    let ui = ctx.ui();
    let nav = ui.global::<Nav>();
    if root() != "home" {
        // First Back from inside a tab jumps up to the tab bar, the second goes Home.
        if nav.get_zone() != "top" && super::input::mode() != "touch" {
            nav.invoke_focus_top();
            return;
        }
        switch_tab("home");
        nav.invoke_focus_page();
        return;
    }
    confirm_exit();
}

fn confirm_exit() {
    if dialogs::is_open() {
        return;
    }
    dialogs::choose(
        &t("exit.title"),
        "",
        vec![
            choice(&t("exit.stay"), ""),
            Choice { icon: "desktop".into(), ..choice(&t("qm.desktop"), "min") },
            Choice { icon: "exit".into(), danger: true, ..choice(&t("exit.quit"), "quit") },
        ],
        |v| match v.as_deref() {
            Some("min") => cx().send("minimize", |b| b.minimize()),
            Some("quit") => {
                let ctx = cx();
                if ctx.demo() {
                    let _ = slint::quit_event_loop();
                } else {
                    ctx.send("quit", |b| b.quit());
                }
            }
            _ => {}
        },
    );
}

fn page_key(k: &str) {
    match k {
        "search" => {
            switch_tab("search");
            cx().ui().global::<Nav>().invoke_focus_page();
        }
        "fullscreen" => cx().send("toggle_fullscreen", |b| b.toggle_fullscreen()),
        // "menu" (☰ / F1) and "view" are handled by whoever subscribed (the quick menu).
        other => cx().dispatch(&format!("key:{other}"), json!(null)),
    }
}

/// Push the stack's top to Slint and tell subscribers.
fn apply(forward: bool) {
    let ctx = cx();
    let ui = ctx.ui();
    let nav = ui.global::<Nav>();
    let (name, id) = current();
    let depth = depth();
    nav.set_tab(root().into());
    nav.set_detail(depth > 1);
    nav.set_restoring(!forward);
    nav.set_zone("page".into());
    nav.set_scrolled(false);
    nav.set_route(Route { name: name.clone().into(), id: id.clone().into() });
    ctx.dispatch("route", json!({ "name": name, "id": id, "forward": forward }));
}
