//! The battery panel (tap the top bar's battery, or Battery in the quick menu): power draw, time left
//! or to full, charge, capacity, health, voltage and cycles, read every 2 s while it's open
//! (`crate::battery`, off the event loop).

use crate::battery::{self, Info};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{lang, t};
use crate::{BatteryPanel, BatteryRow, Hints, Sys};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::time::Duration;

thread_local! {
    static POLL: slint::Timer = slint::Timer::default();
    static SAVED_HINTS: RefCell<Option<ModelRc<crate::Hint>>> = const { RefCell::new(None) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    ui.global::<Sys>().on_battery_clicked(open);
    ui.global::<BatteryPanel>().on_close(close);
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
    cx().ui().global::<BatteryPanel>().get_open()
}

pub fn open() {
    if is_open() {
        return;
    }
    let ui = cx().ui();
    let hints = ui.global::<Hints>();
    SAVED_HINTS.with(|h| *h.borrow_mut() = Some(hints.get_items()));
    hints.set_items(ModelRc::new(VecModel::from(vec![crate::Hint { button: "b".into(), label: t("hint.back").into() }])));
    refresh();
    ui.global::<BatteryPanel>().set_open(true);
    POLL.with(|p| p.start(slint::TimerMode::Repeated, Duration::from_secs(2), refresh));
}

pub fn close() {
    POLL.with(|p| p.stop());
    let ui = cx().ui();
    ui.global::<BatteryPanel>().set_open(false);
    if let Some(items) = SAVED_HINTS.with(|h| h.borrow_mut().take()) {
        ui.global::<Hints>().set_items(items);
    }
    ui.global::<crate::Nav>().invoke_focus_page();
}

fn refresh() {
    if cx().demo() {
        show(Some(battery::demo()));
        return;
    }
    std::thread::spawn(|| {
        let info = battery::read();
        let _ = slint::invoke_from_event_loop(move || show(info));
    });
}

/// "14.3 W" / "14,3 W".
fn decimal(v: f64, unit: &str) -> String {
    let s = format!("{v:.1}");
    let s = if lang() == "fr" { s.replace('.', ",") } else { s };
    format!("{s} {unit}")
}

fn wh(mwh: u32) -> String {
    decimal(mwh as f64 / 1000.0, "Wh")
}

fn show(info: Option<Info>) {
    let ui = cx().ui();
    let p = ui.global::<BatteryPanel>();
    let Some(i) = info else {
        p.set_percent("".into());
        p.set_state(t("bat.none").into());
        p.set_power("".into());
        p.set_time("".into());
        p.set_rows(ModelRc::new(VecModel::from(Vec::<BatteryRow>::new())));
        return;
    };
    // The top bar's percentage (Windows' own figure) is what people compare against; fall back to ours.
    let sys = ui.global::<Sys>();
    let pct = if sys.get_has_battery() && !cx().demo() { sys.get_battery() as u32 } else { i.percent.unwrap_or(0) };
    p.set_percent(format!("{pct}%").into());
    p.set_charging(i.state == "charging");
    p.set_low(pct <= 15 && i.state != "charging");
    p.set_state(t(&format!("bat.{}", i.state)).into());
    p.set_name(i.name.clone().into());

    let charging = i.state == "charging";
    p.set_power_label(t(if charging { "bat.chargeRate" } else { "bat.draw" }).into());
    p.set_power(i.rate_mw.map(|mw| decimal(mw as f64 / 1000.0, "W")).unwrap_or_default().into());
    let left = i.seconds_left().filter(|s| *s > 0 && *s < 48 * 3600);
    p.set_time_label(t(if charging { "bat.toFull" } else { "bat.left" }).into());
    p.set_time(left.map(|s| fmt::runtime(s.div_ceil(60) as i64)).unwrap_or_default().into());

    let mut rows = Vec::new();
    let mut row = |label: &str, value: String| rows.push(BatteryRow { label: t(label).into(), value: value.into() });
    if let (Some(c), Some(f)) = (i.capacity_mwh, i.full_mwh) {
        row("bat.charge", format!("{} / {}", wh(c), wh(f)));
    }
    if let Some(d) = i.design_mwh {
        row("bat.design", wh(d));
    }
    if let Some(h) = i.health() {
        row("bat.health", format!("{h}%"));
    }
    if let Some(v) = i.voltage_mv {
        row("bat.voltage", decimal(v as f64 / 1000.0, "V"));
    }
    if let Some(c) = i.cycles {
        row("bat.cycles", c.to_string());
    }
    p.set_rows(ModelRc::new(VecModel::from(rows)));
}
