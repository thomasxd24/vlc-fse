//! The Bluetooth panel (Bluetooth in the quick menu): AirPods battery, where sound plays and the paired
//! Bluetooth audio devices. Re-read every 2 s while it's open; the AirPods listener runs only then too.
//! All Bluetooth and audio calls run off the event loop (`crate::bluetooth`).

use crate::bluetooth::{self, AirPods, Status};
use crate::ui::ctx::{cx, Ctx};
use crate::ui::i18n::{t, tv};
use crate::{BtPanel, BtRow, Hints, PodLevel};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::time::Duration;

const REFRESH: Duration = Duration::from_secs(2);
/// How long a headset gets to (dis)connect before the panel says how it went.
const SETTLE: Duration = Duration::from_secs(7);

thread_local! {
    static POLL: slint::Timer = slint::Timer::default();
    static SAVED_HINTS: RefCell<Option<ModelRc<crate::Hint>>> = const { RefCell::new(None) };
    /// The status as shown (rows are its outputs, then its devices).
    static STATUS: RefCell<Status> = RefCell::new(Status::default());
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// The demo's own state, so its actions show a result.
    static DEMO: RefCell<Option<Status>> = const { RefCell::new(None) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let p = ui.global::<BtPanel>();
    p.on_close(close);
    p.on_activate(|i| activate(i as usize));
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
    cx().ui().global::<BtPanel>().get_open()
}

pub fn open() {
    if is_open() {
        return;
    }
    let ui = cx().ui();
    let hints = ui.global::<Hints>();
    SAVED_HINTS.with(|h| *h.borrow_mut() = Some(hints.get_items()));
    hints.set_items(ModelRc::new(VecModel::from(vec![
        crate::Hint { button: "a".into(), label: t("hint.select").into() },
        crate::Hint { button: "b".into(), label: t("hint.back").into() },
    ])));
    let p = ui.global::<BtPanel>();
    p.set_index(0);
    p.set_message("".into());
    p.set_open(true);
    if !cx().demo() {
        std::thread::spawn(|| bluetooth::watch_airpods(true));
    }
    refresh();
    POLL.with(|p| p.start(slint::TimerMode::Repeated, REFRESH, refresh));
}

pub fn close() {
    POLL.with(|p| p.stop());
    if !cx().demo() {
        std::thread::spawn(|| bluetooth::watch_airpods(false));
    }
    let ui = cx().ui();
    ui.global::<BtPanel>().set_open(false);
    if let Some(items) = SAVED_HINTS.with(|h| h.borrow_mut().take()) {
        ui.global::<Hints>().set_items(items);
    }
    ui.global::<crate::Nav>().invoke_focus_page();
}

fn refresh() {
    if cx().demo() {
        let st = DEMO.with(|d| d.borrow_mut().get_or_insert_with(bluetooth::demo).clone());
        let pods = st.devices.iter().any(|d| d.connected && d.name.contains("AirPods")).then(bluetooth::demo_airpods);
        show(st, pods);
        return;
    }
    std::thread::spawn(|| {
        let st = bluetooth::read();
        let pods = bluetooth::latest_airpods();
        let _ = slint::invoke_from_event_loop(move || show(st, pods));
    });
}

fn level(label: &str, v: Option<u8>, charging: bool) -> PodLevel {
    PodLevel {
        label: t(label).into(),
        value: v.map(|v| format!("{v}%")).unwrap_or_else(|| "—".into()).into(),
        charging,
        low: v.is_some_and(|v| v <= 20) && !charging,
    }
}

/// Rows per list: enough for a handheld (speakers, headphones, a monitor…) without the panel running off
/// screen when Windows also lists virtual outputs.
const MAX_ROWS: usize = 4;

fn show(mut st: Status, pods: Option<AirPods>) {
    if !is_open() {
        return;
    }
    // Keep the default output and connected devices when trimming (both lists put those first).
    st.outputs.sort_by_key(|o| !o.default);
    st.outputs.truncate(MAX_ROWS);
    st.devices.truncate(MAX_ROWS);
    let ui = cx().ui();
    let p = ui.global::<BtPanel>();
    p.set_available(st.available);
    match &pods {
        Some(a) => {
            p.set_pods_model(a.model.into());
            p.set_pods(ModelRc::new(VecModel::from(vec![
                level("bt.left", a.left, a.charging_left),
                level("bt.right", a.right, a.charging_right),
                level("bt.case", a.case, a.charging_case),
            ])));
        }
        None => p.set_pods_model("".into()),
    }
    let mut rows = Vec::new();
    for (i, o) in st.outputs.iter().enumerate() {
        rows.push(BtRow {
            kind: "output".into(),
            title: o.name.clone().into(),
            detail: if o.default { t("bt.default").into() } else { Default::default() },
            active: o.default,
            header: if i == 0 { t("bt.output").into() } else { Default::default() },
        });
    }
    for (i, d) in st.devices.iter().enumerate() {
        rows.push(BtRow {
            kind: "device".into(),
            title: d.name.clone().into(),
            detail: t(if d.connected { "bt.connected" } else { "bt.notConnected" }).into(),
            active: d.connected,
            header: if i == 0 { t("bt.devices").into() } else { Default::default() },
        });
    }
    if st.available && st.devices.is_empty() && p.get_message().is_empty() {
        p.set_message(t("bt.noDevices").into());
    }
    let n = rows.len();
    p.set_rows(ModelRc::new(VecModel::from(rows)));
    if p.get_index() as usize >= n {
        p.set_index(n.saturating_sub(1) as i32);
    }
    STATUS.with(|s| *s.borrow_mut() = st);
}

fn activate(i: usize) {
    if BUSY.with(Cell::get) {
        return;
    }
    let (outputs, devices) = STATUS.with(|s| {
        let s = s.borrow();
        (s.outputs.clone(), s.devices.clone())
    });
    let ui = cx().ui();
    let p = ui.global::<BtPanel>();
    if let Some(o) = outputs.get(i) {
        if o.default {
            return;
        }
        let (id, name) = (o.id.clone(), o.name.clone());
        run(
            move || bluetooth::set_default_output(&id),
            move |ok| tv(if ok { "bt.outputSet" } else { "bt.outputFailed" }, &[("name", name.into())]),
            |st| {
                for o in &mut st.outputs {
                    o.default = false;
                }
            },
            i,
        );
        return;
    }
    let Some(d) = devices.get(i - outputs.len()).cloned() else { return };
    let on = !d.connected;
    p.set_message(tv(if on { "bt.connecting" } else { "bt.disconnecting" }, &[("name", d.name.clone().into())]).into());
    BUSY.with(|b| b.set(true));
    let name = d.name.clone();
    let check = move || {
        // Give the headset time to (dis)connect, then report what actually happened.
        slint::Timer::single_shot(SETTLE, move || {
            let name2 = name.clone();
            let report = move |connected: bool| {
                BUSY.with(|b| b.set(false));
                let key = match (on, connected) {
                    (true, true) => "bt.connectedTo",
                    (true, false) => "bt.connectFailed",
                    (false, false) => "bt.disconnected",
                    (false, true) => "bt.disconnectFailed",
                };
                if is_open() {
                    cx().ui().global::<BtPanel>().set_message(tv(key, &[("name", name2.into())]).into());
                }
                refresh();
            };
            if cx().demo() {
                report(on);
                return;
            }
            let name = name.clone();
            std::thread::spawn(move || {
                let connected = bluetooth::read().devices.iter().any(|d| d.name == name && d.connected);
                let _ = slint::invoke_from_event_loop(move || report(connected));
            });
        });
    };
    if cx().demo() {
        DEMO.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                if let Some(x) = st.devices.iter_mut().find(|x| x.name == d.name) {
                    x.connected = on;
                }
            }
        });
        check();
        return;
    }
    std::thread::spawn(move || {
        bluetooth::connect(&d, on);
        let _ = slint::invoke_from_event_loop(check);
    });
}

/// Run `action` off the event loop, then show `message(result)` and refresh. In the demo, `demo`
/// resets its state and output `row` becomes the default instead.
fn run(action: impl FnOnce() -> bool + Send + 'static, message: impl FnOnce(bool) -> String + Send + 'static, demo: impl FnOnce(&mut Status), row: usize) {
    BUSY.with(|b| b.set(true));
    let done = move |ok: bool| {
        BUSY.with(|b| b.set(false));
        if is_open() {
            cx().ui().global::<BtPanel>().set_message(message(ok).into());
        }
        refresh();
    };
    if cx().demo() {
        DEMO.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                demo(st);
                if let Some(o) = st.outputs.get_mut(row) {
                    o.default = true;
                }
            }
        });
        done(true);
        return;
    }
    std::thread::spawn(move || {
        let ok = action();
        let _ = slint::invoke_from_event_loop(move || done(ok));
    });
}
