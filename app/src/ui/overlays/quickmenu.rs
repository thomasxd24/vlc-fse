//! The quick menu (☰ / F1, or the menu button in mouse / touch mode) — the renderer's `QuickMenu`:
//! clock and status, volume / brightness sliders through the system helper (`system_get` /
//! `system_set`), and Go to desktop, Sleep, Restart, Shut down, Statistics, Settings, Exit Lounge.
//!
//! Also home to what the three layers (quick menu, now playing, running game) share: while any is open
//! the hint bar shows Select / Back, and when the last one closes the page gets its hints and keyboard
//! focus back ([`layer_opened`] / [`layer_closed`]).

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, choice, Choice};
use crate::ui::i18n::{t, tv};
use crate::ui::{fmt, input, router};
use crate::{App, GameLayer, Hint, Hints, Nav, NowPlaying, QmAction, QmSlider, QuickMenu, Sys, TopBar};
use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

thread_local! {
    /// The last `system_get` answer (kept between openings, like the renderer's `sys`).
    static SYS: RefCell<Value> = RefCell::new(json!({ "supported": false }));
    static SLIDERS: Rc<VecModel<QmSlider>> = Rc::new(VecModel::default());
    /// One pending `system_set` per key: rapid changes (holding the stick) coalesce into one call.
    static SEND: RefCell<HashMap<String, slint::Timer>> = RefCell::new(HashMap::new());
    /// The page's hints and focus zone from before the first layer opened.
    static SAVED: RefCell<Option<(Vec<Hint>, String)>> = const { RefCell::new(None) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let qm = ui.global::<QuickMenu>();
    SLIDERS.with(|m| qm.set_sliders(ModelRc::from(m.clone())));
    qm.on_nudge(|key, dir| {
        let step = if key == "volume" { 4.0 } else { 10.0 };
        set_value(&key, sys_num(&key) + dir as f64 * step);
        input::feedback("move");
    });
    qm.on_set(|key, v| set_value(&key, v as f64));
    qm.on_toggle_mute(toggle_mute);
    qm.on_action(|a| action(&a));
    qm.on_close(close);
    ui.global::<TopBar>().on_open_quick_menu(show);
    // ☰ and ⧉ both open it (the renderer's Nav.onAction 'menu' / 'view').
    ctx.on_event("key:menu", |_, _| show());
    ctx.on_event("key:view", |_, _| show());
    crate::ui::on_close_overlay(|| {
        if is_open() {
            close();
            true
        } else {
            false
        }
    });
}

pub fn is_open() -> bool {
    cx().ui().global::<QuickMenu>().get_open()
}

/// Open the menu, or close it if it's open. Not over a dialog, VLC or a running game.
pub fn show() {
    let ui = cx().ui();
    if ui.global::<NowPlaying>().get_open() || ui.global::<GameLayer>().get_open() || dialogs::is_open() {
        return;
    }
    let qm = ui.global::<QuickMenu>();
    if qm.get_open() {
        close();
        return;
    }
    qm.set_zone(1);
    qm.set_index(0);
    qm.set_date(capitalize_words(&fmt::long_date()).into());
    qm.set_fact_list(crate::ui::model::model(facts()));
    qm.set_actions(crate::ui::model::model(actions()));
    rebuild_sliders();
    qm.set_open(true);
    layer_opened();
    input::feedback("open");

    let ctx = cx();
    if ctx.demo() {
        SYS.with(|s| *s.borrow_mut() = json!({ "supported": true, "volume": 62, "muted": false, "brightness": 80 }));
        rebuild_sliders();
    } else if ctx.state.borrow().get("systemControls").and_then(Value::as_bool).unwrap_or(false) {
        ctx.call("system_get", |b| b.system_get(), |v| {
            SYS.with(|s| *s.borrow_mut() = v);
            if is_open() {
                rebuild_sliders();
            }
        });
    }
}

pub fn close() {
    let ui = cx().ui();
    let qm = ui.global::<QuickMenu>();
    if !qm.get_open() {
        return;
    }
    qm.set_open(false);
    layer_closed();
}

/// Battery and Wi-Fi under the date (`.qm-facts`).
/// The tile grid: Now playing (while VLC plays behind Lounge), then power, Battery / Wi-Fi when the
/// machine has them, Stats, Settings and Quit.
fn actions() -> Vec<QmAction> {
    let ui = cx().ui();
    let sys = ui.global::<Sys>();
    let win = ui.global::<App>().get_platform() == "win32";
    let a = |value: &str, icon: &str, label: String| QmAction { value: value.into(), icon: icon.into(), label: label.into(), danger: false };
    let mut out = Vec::new();
    if super::nowplaying::is_active() {
        out.push(a("nowplaying", "play", t("np.nowPlaying")));
    }
    out.push(a("desktop", "desktop", t("qm.desktop")));
    if win {
        out.push(a("sleep", "moon", t("qm.sleep")));
        out.push(a("restart", "restart", t("qm.restart")));
        out.push(a("shutdown", "power", t("qm.shutdown")));
        if sys.get_has_battery() {
            out.push(a("battery", "battery", t("bat.title")));
        }
        if sys.get_has_wifi() {
            out.push(a("wifi", "wifi", t("wifi.title")));
        }
    }
    out.push(a("stats", "chart", t("stats.title")));
    out.push(a("settings", "gamepad", t("tab.settings")));
    out.push(QmAction { danger: true, ..a("quit", "exit", t("qm.quit")) });
    out
}

fn facts() -> Vec<slint::SharedString> {
    let ui = cx().ui();
    let sys = ui.global::<Sys>();
    let mut out = Vec::new();
    if sys.get_has_battery() {
        let charging = if sys.get_charging() { format!(" · {}", t("status.charging")) } else { String::new() };
        out.push(format!("{}%{charging}", sys.get_battery()).into());
    }
    if sys.get_wifi() >= 0 && !sys.get_wifi_name().is_empty() {
        out.push(sys.get_wifi_name());
    }
    out
}

/// `text-transform: capitalize` ("mardi 29 septembre" → "Mardi 29 Septembre").
fn capitalize_words(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sys_num(key: &str) -> f64 {
    SYS.with(|s| s.borrow().get(key).and_then(Value::as_f64).unwrap_or(0.0))
}

fn rebuild_sliders() {
    let (supported, volume, muted, brightness) = SYS.with(|s| {
        let s = s.borrow();
        (
            s.get("supported").and_then(Value::as_bool).unwrap_or(false),
            s.get("volume").and_then(Value::as_f64),
            s.get("muted").and_then(Value::as_bool).unwrap_or(false),
            s.get("brightness").and_then(Value::as_f64),
        )
    });
    let mut rows = Vec::new();
    if supported {
        rows.push(QmSlider {
            key: "volume".into(),
            icon: if muted { "mute" } else { "volume" }.into(),
            label: t("qm.volume").into(),
            value: volume.unwrap_or(0.0).round() as i32,
            known: volume.is_some(),
        });
        if let Some(b) = brightness {
            rows.push(QmSlider { key: "brightness".into(), icon: "sun".into(), label: t("qm.brightness").into(), value: b.round() as i32, known: true });
        }
    }
    let n = rows.len();
    SLIDERS.with(|m| {
        if m.row_count() == n {
            for (i, r) in rows.into_iter().enumerate() {
                if m.row_data(i).as_ref() != Some(&r) {
                    m.set_row_data(i, r);
                }
            }
        } else {
            m.set_vec(rows);
        }
    });
    let ui = cx().ui();
    let qm = ui.global::<QuickMenu>();
    if qm.get_zone() == 0 && qm.get_index() as usize >= n {
        if n == 0 {
            qm.set_zone(1);
            qm.set_index(0);
        } else {
            qm.set_index(n as i32 - 1);
        }
    }
}

/// The renderer's `setValue`: clamp, show, and send after 120 ms of quiet.
fn set_value(key: &str, v: f64) {
    let v = v.round().clamp(if key == "brightness" { 1.0 } else { 0.0 }, 100.0) as i64;
    SYS.with(|s| {
        let mut s = s.borrow_mut();
        s[key] = json!(v);
        if key == "volume" && v > 0 {
            s["muted"] = json!(false);
        }
    });
    rebuild_sliders();
    let key = key.to_string();
    SEND.with(|timers| {
        let mut timers = timers.borrow_mut();
        let timer = timers.entry(key.clone()).or_default();
        timer.start(slint::TimerMode::SingleShot, Duration::from_millis(120), move || {
            let key = key.clone();
            cx().send("system_set", move |b| {
                b.system_set(&key, json!(v));
            });
        });
    });
}

fn toggle_mute() {
    let muted = SYS.with(|s| {
        let mut s = s.borrow_mut();
        let m = !s.get("muted").and_then(Value::as_bool).unwrap_or(false);
        s["muted"] = json!(m);
        m
    });
    rebuild_sliders();
    cx().send("system_set", move |b| {
        b.system_set("muted", json!(muted));
    });
}

fn action(a: &str) {
    let ctx = cx();
    match a {
        "settings" => {
            close();
            router::switch_tab("settings");
            ctx.ui().global::<Nav>().invoke_focus_page();
        }
        "battery" => {
            close();
            super::battery::open();
        }
        "wifi" => {
            close();
            super::wifi::open();
        }
        "nowplaying" => {
            close();
            super::nowplaying::reopen();
        }
        "stats" => {
            close();
            if router::current().0 != "stats" {
                router::go("stats", "");
            }
        }
        "desktop" => {
            close();
            ctx.send("power", |b| {
                b.power("desktop");
            });
        }
        "quit" => {
            close();
            if ctx.demo() {
                let _ = slint::quit_event_loop();
            } else {
                ctx.send("quit", |b| b.quit());
            }
        }
        "sleep" | "restart" | "shutdown" => {
            let label = t(&format!("qm.{a}"));
            let action = a.to_string();
            dialogs::choose(
                &tv("qm.confirm", &[("action", label.as_str().into())]),
                "",
                vec![Choice { primary: true, ..choice(&label, "yes") }, choice(&t("common.cancel"), "")],
                move |v| {
                    if v.as_deref() == Some("yes") {
                        close();
                        cx().send("power", move |b| {
                            b.power(&action);
                        });
                    }
                },
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- Shared by the three layers

fn modal_hints() -> ModelRc<Hint> {
    crate::ui::model::model(vec![
        Hint { button: "a".into(), label: t("hint.select").into() },
        Hint { button: "b".into(), label: t("hint.back").into() },
    ])
}

/// A layer (quick menu, now playing, game) has opened: remember the page's hints and focus zone (the
/// first time), and show Select / Back.
pub fn layer_opened() {
    let ui = cx().ui();
    let hints = ui.global::<Hints>();
    SAVED.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            *s = Some((hints.get_items().iter().collect(), ui.global::<Nav>().get_zone().to_string()));
        }
    });
    hints.set_items(modal_hints());
}

/// A layer has closed: hand focus to the next layer still open, or give the page its hints and focus back.
pub fn layer_closed() {
    let ui = cx().ui();
    let qm = ui.global::<QuickMenu>();
    let gl = ui.global::<GameLayer>();
    let np = ui.global::<NowPlaying>();
    if qm.get_open() {
        qm.set_focus_rev(qm.get_focus_rev() + 1);
        return;
    }
    if gl.get_open() {
        gl.set_focus_rev(gl.get_focus_rev() + 1);
        return;
    }
    if np.get_open() {
        np.set_focus_rev(np.get_focus_rev() + 1);
        return;
    }
    let Some((items, zone)) = SAVED.with(|s| s.borrow_mut().take()) else { return };
    ui.global::<Hints>().set_items(crate::ui::model::model(items));
    if dialogs::is_open() {
        return;
    }
    let nav = ui.global::<Nav>();
    if zone == "top" {
        nav.invoke_focus_top();
    } else {
        nav.invoke_focus_page();
    }
}
