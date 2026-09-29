//! The status cluster (clock, battery, Wi-Fi, busy spinner) and app-wide facts pushed into Slint.

use super::ctx::{cx, Ctx};
use super::{fmt, i18n};
use crate::{App, AppWindow, MotionPrefs, Sys, Theme, T};
use serde_json::Value;
use slint::ComponentHandle;
use std::time::Duration;

thread_local! {
    static CLOCK: slint::Timer = slint::Timer::default();
    static POLL: slint::Timer = slint::Timer::default();
}

pub fn install(ctx: &Ctx) {
    tick_clock(&ctx.ui());
    CLOCK.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_secs(1), || tick_clock(&cx().ui()));
    });
    // Battery and Wi-Fi: the backend's system_get / wifi, every 20 s (and right away).
    poll_status();
    POLL.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(20), poll_status));
    ctx.on_event("state", |ctx, st| apply_state(ctx, st));
}

fn tick_clock(ui: &AppWindow) {
    let sys = ui.global::<Sys>();
    let clock = fmt::clock(true);
    if sys.get_clock() != clock {
        sys.set_clock(clock.into());
        sys.set_date(fmt::long_date().into());
    }
}

fn poll_status() {
    let ctx = cx();
    if ctx.demo() {
        let ui = ctx.ui();
        let sys = ui.global::<Sys>();
        sys.set_has_battery(true);
        sys.set_battery(100);
        sys.set_wifi(80);
        return;
    }
    // Battery level isn't a backend command (the web renderer read navigator.getBattery()), so it's
    // read here, off the event loop.
    let weak = ctx.ui.clone();
    std::thread::spawn(move || {
        let b = battery();
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(ui), Some((pct, charging))) = (weak.upgrade(), b) {
                let sys = ui.global::<Sys>();
                sys.set_battery(pct);
                sys.set_charging(charging);
            }
        });
    });
    ctx.call("wifi", |b| b.wifi(), |v| {
        let ui = cx().ui();
        let sys = ui.global::<Sys>();
        let connected = v.get("connected").and_then(Value::as_bool).unwrap_or(false);
        let signal = v.get("signal").and_then(Value::as_f64).unwrap_or(0.0);
        sys.set_wifi(if connected { signal.round() as i32 } else { -1 });
        sys.set_wifi_name(v.get("ssid").and_then(Value::as_str).unwrap_or("").into());
    });
}

fn apply_state(ctx: &Ctx, st: &Value) {
    let ui = ctx.ui();
    // Language: switching re-evaluates every T.tr binding through T.rev.
    let lang = st.get("lang").and_then(Value::as_str).unwrap_or("en");
    let t = ui.global::<T>();
    if t.get_lang() != lang {
        i18n::set_lang(lang);
        t.set_lang(lang.into());
        t.set_rev(t.get_rev() + 1);
        tick_clock(&ui);
        ui.global::<Sys>().set_date(fmt::long_date().into());
    }
    let settings = st.get("settings").cloned().unwrap_or(Value::Null);
    let scale = settings.get("uiScale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    ui.global::<Theme>().set_scale(scale);
    ui.global::<MotionPrefs>().set_reduced(settings.get("animations").and_then(Value::as_str) == Some("reduced"));

    let input = ui.global::<crate::Input>();
    input.set_sounds(settings.get("sounds").and_then(Value::as_bool).unwrap_or(true));
    input.set_haptics(settings.get("haptics").and_then(Value::as_bool).unwrap_or(true));

    let sys = ui.global::<Sys>();
    if let Some(b) = st.get("hasBattery").and_then(Value::as_bool) {
        sys.set_has_battery(b);
    }
    let scanning = st.get("scanning").and_then(Value::as_bool).unwrap_or(false);
    let meta = st.pointer("/metaStatus/running").and_then(Value::as_bool).unwrap_or(false);
    let info = st.pointer("/gameInfoStatus/running").and_then(Value::as_bool).unwrap_or(false);
    sys.set_busy(scanning || meta || info);
    sys.set_busy_text(
        if scanning {
            i18n::t("status.scanning")
        } else if meta {
            i18n::t("status.artwork")
        } else if info {
            i18n::t("status.gameInfo")
        } else {
            String::new()
        }
        .into(),
    );

    let app = ui.global::<App>();
    app.set_version(st.get("version").and_then(Value::as_str).unwrap_or("").into());
    app.set_platform(st.get("platform").and_then(Value::as_str).unwrap_or("win32").into());
    app.set_user(st.pointer("/library/steamUser").and_then(Value::as_str).unwrap_or("").into());
    app.set_has_servers(st.get("servers").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false));
    let active = st
        .get("transfers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|j| matches!(j.get("status").and_then(Value::as_str), Some("running") | Some("queued"))).count())
        .unwrap_or(0);
    app.set_active_transfers(active as i32);
    let lib = |k: &str| st.pointer(&format!("/library/{k}")).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let no_libraries = settings.get("libraries").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true);
    app.set_first_run(no_libraries && lib("games") == 0 && lib("movies") == 0 && lib("shows") == 0);
}

/// (percent, charging), or None without a battery.
#[cfg(windows)]
fn battery() -> Option<(i32, bool)> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 || s.BatteryLifePercent == 255 || s.BatteryFlag & 128 != 0 {
        return None;
    }
    Some((s.BatteryLifePercent as i32, s.ACLineStatus == 1))
}

#[cfg(not(windows))]
fn battery() -> Option<(i32, bool)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?.flatten().find(|e| e.file_name().to_string_lossy().starts_with("BAT"))?;
    let read = |f: &str| std::fs::read_to_string(dir.path().join(f)).ok().map(|s| s.trim().to_string());
    let pct = read("capacity")?.parse().ok()?;
    Some((pct, read("status").as_deref() == Some("Charging")))
}
