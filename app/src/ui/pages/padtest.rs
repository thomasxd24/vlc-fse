//! The controller tester (the renderer's VIEWS.padtest / startPadTester / openPadTester, with the stick
//! maths of gamepad.js: describePad, stick, circularity, pollRate, parseLegionStatus).
//!
//! While the page is open it captures the controller ([`input::set_capture`]): pads stop navigating and
//! every snapshot comes here, so every button can be tried — B included: holding it for a second goes
//! back. The keyboard still works (Esc leaves, arrows reach the buttons). The button hints are hidden.
//!
//! For screenshots without a controller, the pseudo-event `pad-snapshot` feeds a snapshot by hand:
//! `event pad-snapshot {"name":"Xbox Controller","buttons":[…17 values],"axes":[lx,ly,rx,ry]}`.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::{t, tv};
use crate::ui::input::{self, PadSnapshot};
use crate::ui::model::{self, s};
use crate::ui::router;
use crate::{Backdrop, Hints, PadHalf, PadRawByte, PadTestPage};
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const STICK_R: f32 = 38.0;
/// Stick centres on the drawing (viewbox units): left, right.
const STICKS: [(f32, f32); 2] = [(160.0, 150.0), (408.0, 222.0)];
const NAMES: [&str; 17] = ["A", "B", "X", "Y", "LB", "RB", "LT", "RT", "⧉", "☰", "L3", "R3", "↑", "↓", "←", "→", "⌂"];
/// USB ids of the Legion Go family's built-in controllers.
const LEGION_IDS: [(u16, &[u16]); 2] = [(0x17ef, &[0x6182, 0x6183, 0x6184, 0x6185, 0x61eb, 0x61ec, 0x61ed, 0x61ee]), (0x1a86, &[0xe310, 0xe311])];
/// Report 0x04 offsets (the report id counts as byte 0): batteries, then attach states.
const OFFSETS: [usize; 4] = [5, 7, 12, 13];

#[derive(Clone, Copy, PartialEq)]
enum HalfState {
    Attached,
    Detached,
    Off,
}

#[derive(Default)]
struct Tester {
    active: bool,
    trails: [Vec<(f32, f32)>; 2],
    rest: [f32; 2],
    times: VecDeque<Instant>,
    last: Option<([f32; 17], [f32; 4])>,
    last_seen: Option<Instant>,
    /// A hand-fed snapshot (`pad-snapshot`): don't time out back to "Press any button".
    sticky: bool,
    shown: String,
    hold_start: Option<Instant>,
    combo_down: bool,
    timer: Option<slint::Timer>,
    // The Legion Go halves, from the `legion-report` events (kept even while the page is closed).
    legion: Option<[(HalfState, u8); 2]>,
    raw: Option<Vec<u8>>,
    raw_open: bool,
}

thread_local! {
    static T: RefCell<Tester> = RefCell::new(Tester::default());
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<PadTestPage>();
    page.on_action(|a| action(&a));
    ctx.on_event("route", |_, r| {
        if s(r, "name") == "padtest" {
            start();
        } else if T.with(|t| t.borrow().active) {
            stop();
        }
    });
    ctx.on_event("legion-report", |_, r| legion_report(r));
    ctx.on_event("legion-state", |_, r| {
        if !r.get("connected").and_then(Value::as_bool).unwrap_or(false) {
            T.with(|t| t.borrow_mut().legion = None);
            show_legion(None);
        }
    });
    ctx.on_event("pad-snapshot", |_, v| {
        let f = |key: &str, len: usize| -> Vec<f32> {
            let mut out: Vec<f32> = model::arr(v, key).iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
            out.resize(len, 0.0);
            out
        };
        let mut snap = PadSnapshot {
            name: s(v, "name").to_string(),
            vendor: v.get("vendor").and_then(Value::as_u64).map(|x| x as u16),
            product: v.get("product").and_then(Value::as_u64).map(|x| x as u16),
            connected_count: v.get("count").and_then(Value::as_u64).unwrap_or(1) as usize,
            ..Default::default()
        };
        snap.buttons.copy_from_slice(&f("buttons", 17));
        snap.axes.copy_from_slice(&f("axes", 4));
        T.with(|t| t.borrow_mut().sticky = true);
        on_snapshot(&snap);
    });
}

/// Open the tester (Settings › Controller, the quick menu): `openPadTester`.
pub fn open() {
    if router::current().0 == "padtest" {
        return;
    }
    router::go("padtest", "");
}

fn start() {
    let ui = cx().ui();
    ui.global::<Backdrop>().set_src("".into());
    ui.global::<Hints>().set_hidden(true);
    let page = ui.global::<PadTestPage>();
    page.set_waiting(true);
    page.set_hold(0.0);
    page.set_pressed(model::model(vec![false; 17]));
    page.set_stats(model::model(vec![slint::SharedString::new(); 9]));
    T.with(|t| {
        let mut t = t.borrow_mut();
        t.active = true;
        t.sticky = false;
        t.last = None;
        t.last_seen = None;
        t.shown.clear();
        t.hold_start = None;
        t.times.clear();
        // No snapshot for a while: the pad went away.
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), || {
            let gone = T.with(|t| {
                let t = t.borrow();
                !t.sticky && t.last_seen.map(|at| at.elapsed() > Duration::from_millis(600)).unwrap_or(true)
            });
            if gone {
                let ui = cx().ui();
                ui.global::<PadTestPage>().set_waiting(true);
                T.with(|t| t.borrow_mut().shown.clear());
            }
        });
        t.timer = Some(timer);
    });
    reset();
    show_legion(None);
    input::set_capture(Some(Box::new(|snap: &PadSnapshot| on_snapshot(snap))));
}

fn stop() {
    input::set_capture(None);
    T.with(|t| {
        let mut t = t.borrow_mut();
        t.active = false;
        t.timer = None;
    });
    let ui = cx().ui();
    ui.global::<Hints>().set_hidden(false);
}

fn action(a: &str) {
    match a {
        "weak" => input::rumble(0.0, 1.0, 500),
        "strong" => input::rumble(1.0, 0.3, 500),
        "reset" => reset(),
        "raw" => {
            let open = T.with(|t| {
                let mut t = t.borrow_mut();
                t.raw_open = !t.raw_open;
                t.raw_open
            });
            cx().ui().global::<PadTestPage>().set_raw_open(open);
            show_legion(None);
        }
        _ => {}
    }
}

fn reset() {
    T.with(|t| {
        let mut t = t.borrow_mut();
        t.trails = Default::default();
        t.rest = [0.0; 2];
    });
    let ui = cx().ui();
    let page = ui.global::<PadTestPage>();
    page.set_trail_l("".into());
    page.set_trail_r("".into());
}

// ---------------------------------------------------------------- gamepad.js

struct PadInfo {
    name: String,
    legion: bool,
}

fn describe_pad(snap: &PadSnapshot) -> PadInfo {
    let name = snap.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if name.is_empty() { "Controller".to_string() } else { name };
    let by_id = match (snap.vendor, snap.product) {
        (Some(v), Some(p)) => LEGION_IDS.iter().any(|(vendor, products)| *vendor == v && products.contains(&p)),
        _ => false,
    };
    let by_name = name.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == "legion");
    PadInfo { legion: by_id || by_name, name }
}

fn magnitude(x: f32, y: f32) -> f32 {
    x.hypot(y)
}

/// How round a stick's outer range is: for each of 36 directions the furthest reach, then the average
/// distance of those from the unit circle (%). None until most directions have been covered.
fn circularity(samples: &[(f32, f32)]) -> Option<f64> {
    const BINS: usize = 36;
    let mut reach = [0f32; BINS];
    for &(x, y) in samples {
        let mag = magnitude(x, y);
        if mag < 0.6 {
            continue;
        }
        let angle = y.atan2(x);
        let bin = (((angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI)) * BINS as f32).floor() as usize % BINS;
        reach[bin] = reach[bin].max(mag);
    }
    let hit: Vec<f32> = reach.into_iter().filter(|r| *r > 0.0).collect();
    if (hit.len() as f32 / BINS as f32) < 0.75 {
        return None;
    }
    let error = hit.iter().map(|r| (1.0 - r.min(1.5)).abs() as f64).sum::<f64>() / hit.len() as f64;
    Some((error * 1000.0).round() / 10.0)
}

/// Updates per second, from the times at which the pad reported new data.
fn poll_rate(times: &VecDeque<Instant>) -> i64 {
    match (times.front(), times.back()) {
        (Some(a), Some(b)) if times.len() >= 2 => {
            let span = b.duration_since(*a).as_secs_f64();
            if span > 0.0 {
                ((times.len() - 1) as f64 / span).round() as i64
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Report 0x04 from the Legion controllers' vendor interface: each half's state and battery. None for
/// other reports and for misaligned ones (state bytes out of range). `data` excludes the report id.
fn parse_legion(report_id: u8, data: &[u8]) -> Option<[(HalfState, u8); 2]> {
    if report_id != 0x04 || data.len() < OFFSETS[3] {
        return None;
    }
    let at = |i: usize| data[i - 1];
    let (ls, rs) = (at(OFFSETS[2]), at(OFFSETS[3]));
    if !(1..=3).contains(&ls) || !(1..=3).contains(&rs) {
        return None;
    }
    let half = |state: u8, battery: u8| {
        if state == 2 {
            (HalfState::Attached, battery)
        } else if battery == 0 {
            (HalfState::Off, 0)
        } else {
            (HalfState::Detached, battery)
        }
    };
    Some([half(ls, at(OFFSETS[0]).min(100)), half(rs, at(OFFSETS[1]).min(100))])
}

fn legion_report(r: &Value) {
    let id = r.get("reportId").and_then(Value::as_u64).unwrap_or(0) as u8;
    let bytes: Vec<u8> = model::arr(r, "bytes").iter().map(|x| x.as_u64().unwrap_or(0) as u8).collect();
    let active = T.with(|t| {
        let mut t = t.borrow_mut();
        if id == 0x04 || t.raw.is_none() {
            let mut raw = vec![id];
            raw.extend(bytes.iter().take(20));
            t.raw = Some(raw);
        }
        if let Some(st) = parse_legion(id, &bytes) {
            t.legion = Some(st);
        }
        t.active
    });
    if active {
        show_legion(None);
    }
}

/// The Legion section, the no-HID note and the raw bytes. `legion_pad`: whether the shown pad is a
/// Legion Go's (None: keep what's shown).
fn show_legion(legion_pad: Option<bool>) {
    let ui = cx().ui();
    let page = ui.global::<PadTestPage>();
    let legion_pad = legion_pad.unwrap_or_else(|| page.get_legion());
    T.with(|t| {
        let t = t.borrow();
        let any = t.legion.is_some() || t.raw.is_some();
        page.set_show_legion(any);
        page.set_no_hid(!any && legion_pad);
        let halves = ["pad.left", "pad.right"]
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let half = t.legion.map(|l| l[i]);
                let (state, text) = match half.map(|h| h.0) {
                    Some(HalfState::Attached) => ("attached", t_("pad.stAttached")),
                    Some(HalfState::Detached) => ("detached", t_("pad.stDetached")),
                    Some(HalfState::Off) => ("off", t_("pad.stOff")),
                    None => ("unknown", t_("pad.stUnknown")),
                };
                PadHalf {
                    name: t_(key).into(),
                    state: state.into(),
                    text: text.into(),
                    battery: half.map(|h| h.1 as f32 / 100.0).unwrap_or(0.0),
                    pct: match half {
                        Some((s, b)) if s != HalfState::Off => format!("{b} %"),
                        _ => "–".into(),
                    }
                    .into(),
                }
            })
            .collect();
        page.set_halves(model::model(halves));
        page.set_raw_open(t.raw_open);
        let raw = t
            .raw
            .as_ref()
            .map(|r| r.iter().take(16).enumerate().map(|(i, b)| PadRawByte { index: i as i32, hex: format!("{b:02x}").into(), hl: OFFSETS.contains(&i) }).collect())
            .unwrap_or_default();
        page.set_raw(model::model(raw));
    });
}

/// `t`, for closures where `t` is the borrowed tester.
fn t_(key: &str) -> String {
    t(key)
}

// ---------------------------------------------------------------- Every frame

fn on_snapshot(snap: &PadSnapshot) {
    if !T.with(|t| t.borrow().active) {
        return;
    }
    let ui = cx().ui();
    let page = ui.global::<PadTestPage>();
    let now = Instant::now();
    page.set_waiting(false);

    // A different pad than last time: name it and start over.
    let info = describe_pad(snap);
    let key = format!("{}|{:?}|{:?}|{}", snap.name, snap.vendor, snap.product, snap.connected_count);
    if T.with(|t| t.borrow().shown != key) {
        T.with(|t| t.borrow_mut().shown = key);
        page.set_name(
            if snap.connected_count > 1 { format!("{} · {}", info.name, tv("pad.more", &[("n", snap.connected_count.into())])) } else { info.name.clone() }.into(),
        );
        page.set_legion(info.legion);
        let ids = match (snap.vendor, snap.product) {
            (Some(v), Some(p)) => format!("{v:04x}:{p:04x} · {}", t("pad.standard")),
            _ => t("pad.standard"),
        };
        page.set_ids(ids.into());
        reset();
        show_legion(Some(info.legion));
    }

    let pressed: Vec<bool> = snap.buttons.iter().map(|v| *v > 0.5).collect();
    let (changed, rate, trails, rest) = T.with(|t| {
        let mut t = t.borrow_mut();
        t.last_seen = Some(now);
        let changed = t.last != Some((snap.buttons, snap.axes));
        if changed {
            t.last = Some((snap.buttons, snap.axes));
            t.times.push_back(now);
            if t.times.len() > 120 {
                t.times.pop_front();
            }
        }
        for (k, axes) in [(0usize, (snap.axes[0], snap.axes[1])), (1, (snap.axes[2], snap.axes[3]))] {
            let mag = magnitude(axes.0, axes.1);
            if mag > 0.05 {
                if changed {
                    t.trails[k].push(axes);
                    if t.trails[k].len() > 1500 {
                        t.trails[k].remove(0);
                    }
                }
            } else {
                // Resting: the largest offset seen at rest is the drift.
                t.rest[k] = (t.rest[k] * 0.995).max(mag);
            }
        }
        (changed, poll_rate(&t.times), t.trails.clone(), t.rest)
    });

    if changed {
        page.set_pressed(model::model(pressed.clone()));
        page.set_lt(snap.buttons[6]);
        page.set_rt(snap.buttons[7]);
        page.set_lx(snap.axes[0]);
        page.set_ly(snap.axes[1]);
        page.set_rx(snap.axes[2]);
        page.set_ry(snap.axes[3]);
        let trail_cmd = |k: usize| -> String {
            let (cx, cy) = STICKS[k];
            trails[k]
                .iter()
                .enumerate()
                .map(|(i, (x, y))| format!("{}{:.1} {:.1}", if i == 0 { "M" } else { " L" }, cx + x * STICK_R, cy + y * STICK_R))
                .collect()
        };
        page.set_trail_l(if trails[0].len() > 1 { trail_cmd(0) } else { String::new() }.into());
        page.set_trail_r(if trails[1].len() > 1 { trail_cmd(1) } else { String::new() }.into());
    }

    let round = |k: usize| match circularity(&trails[k]) {
        Some(e) => tv("pad.roundError", &[("n", e.into())]),
        None => t("pad.rollStick"),
    };
    let down: Vec<&str> = pressed.iter().enumerate().filter(|(_, p)| **p).map(|(i, _)| NAMES[i]).collect();
    let stats: Vec<slint::SharedString> = vec![
        tv("pad.hz", &[("n", rate.into())]),
        format!("{:.2}, {:.2}", snap.axes[0], snap.axes[1]),
        format!("{:.2}, {:.2}", snap.axes[2], snap.axes[3]),
        format!("{:.1} %", rest[0] * 100.0),
        format!("{:.1} %", rest[1] * 100.0),
        round(0),
        round(1),
        format!("{} % · {} %", (snap.buttons[6] * 100.0).round(), (snap.buttons[7] * 100.0).round()),
        if down.is_empty() { "–".to_string() } else { down.join(" ") },
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    page.set_stats(model::model(stats));

    // L3 + R3 together: vibration test.
    let combo = pressed[10] && pressed[11];
    let fire = T.with(|t| {
        let mut t = t.borrow_mut();
        let fire = combo && !t.combo_down;
        t.combo_down = combo;
        fire
    });
    if fire {
        input::rumble(1.0, 0.3, 500);
    }

    // Hold B to leave.
    let hold = T.with(|t| {
        let mut t = t.borrow_mut();
        if pressed[1] {
            let start = *t.hold_start.get_or_insert(now);
            now.duration_since(start).as_secs_f32().min(1.0)
        } else {
            t.hold_start = None;
            0.0
        }
    });
    page.set_hold(hold);
    if hold >= 1.0 {
        T.with(|t| t.borrow_mut().hold_start = None);
        // Not from inside the capture callback: leaving releases the capture.
        slint::Timer::single_shot(Duration::ZERO, || {
            if router::current().0 == "padtest" {
                input::feedback("back");
                router::back();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundness_needs_coverage() {
        assert_eq!(circularity(&[(1.0, 0.0)]), None);
        let circle: Vec<(f32, f32)> = (0..72).map(|i| ((i as f32 * 5.0).to_radians().cos(), (i as f32 * 5.0).to_radians().sin())).collect();
        assert_eq!(circularity(&circle), Some(0.0));
    }

    #[test]
    fn legion_status() {
        let mut data = vec![0u8; 63];
        data[4] = 80; // byte 5: left battery
        data[6] = 0; // byte 7: right battery
        data[11] = 2; // byte 12: left attached
        data[12] = 1; // byte 13: right detached
        let st = parse_legion(4, &data).unwrap();
        assert!(st[0] == (HalfState::Attached, 80));
        assert!(st[1] == (HalfState::Off, 0));
        assert!(parse_legion(3, &data).is_none());
        data[11] = 9;
        assert!(parse_legion(4, &data).is_none());
    }

    #[test]
    fn legion_pads_by_id_or_name() {
        let pad = |name: &str, v, p| PadSnapshot { name: name.into(), vendor: v, product: p, ..Default::default() };
        assert!(describe_pad(&pad("Controller", Some(0x17ef), Some(0x6182))).legion);
        assert!(describe_pad(&pad("Legion Controller for Windows", None, None)).legion);
        assert!(!describe_pad(&pad("Xbox Controller", Some(0x045e), Some(0x028e))).legion);
    }
}
