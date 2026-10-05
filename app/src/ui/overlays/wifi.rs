//! The Wi-Fi panel (tap the top bar's Wi-Fi icon, or Wi-Fi in the quick menu): the IP address and the
//! networks in range, re-read every 4 s while it's open (a fresh scan on opening and every 20 s).
//! Picking a network joins it: saved and open networks straight away, new secured ones after a password
//! prompt. All Wi-Fi calls run off the event loop (`crate::wifi`).

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, Prompt};
use crate::ui::i18n::{t, tv};
use crate::wifi::{self, Network, Status};
use crate::{Hints, Sys, WifiNet, WifiPanel};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::time::Duration;

const REFRESH: Duration = Duration::from_secs(4);
/// Rescan every this many refreshes.
const SCAN_EVERY: u32 = 5;

thread_local! {
    static POLL: slint::Timer = slint::Timer::default();
    static TICKS: Cell<u32> = const { Cell::new(0) };
    static SAVED_HINTS: RefCell<Option<ModelRc<crate::Hint>>> = const { RefCell::new(None) };
    /// The list as shown (row i = networks[i]).
    static NETWORKS: RefCell<Vec<Network>> = const { RefCell::new(Vec::new()) };
    static CONNECTING: Cell<bool> = const { Cell::new(false) };
    /// The demo's own Wi-Fi, so joining a network there shows the result.
    static DEMO: RefCell<Option<Status>> = const { RefCell::new(None) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    ui.global::<Sys>().on_wifi_clicked(open);
    let p = ui.global::<WifiPanel>();
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
    cx().ui().global::<WifiPanel>().get_open()
}

pub fn open() {
    if is_open() {
        return;
    }
    let ui = cx().ui();
    let hints = ui.global::<Hints>();
    SAVED_HINTS.with(|h| *h.borrow_mut() = Some(hints.get_items()));
    set_hints();
    let p = ui.global::<WifiPanel>();
    p.set_index(0);
    p.set_message("".into());
    p.set_open(true);
    TICKS.with(|c| c.set(0));
    refresh(true);
    POLL.with(|p| {
        p.start(slint::TimerMode::Repeated, REFRESH, || {
            let n = TICKS.with(|c| {
                c.set(c.get() + 1);
                c.get()
            });
            refresh(n.is_multiple_of(SCAN_EVERY));
        })
    });
}

fn set_hints() {
    cx().ui().global::<Hints>().set_items(ModelRc::new(VecModel::from(vec![
        crate::Hint { button: "a".into(), label: t("wifi.join").into() },
        crate::Hint { button: "b".into(), label: t("hint.back").into() },
    ])));
}

pub fn close() {
    POLL.with(|p| p.stop());
    let ui = cx().ui();
    ui.global::<WifiPanel>().set_open(false);
    if let Some(items) = SAVED_HINTS.with(|h| h.borrow_mut().take()) {
        ui.global::<Hints>().set_items(items);
    }
    ui.global::<crate::Nav>().invoke_focus_page();
}

fn refresh(scan: bool) {
    if cx().demo() {
        let st = DEMO.with(|d| d.borrow_mut().get_or_insert_with(wifi::demo).clone());
        show(st, Some("192.168.1.42".into()));
        return;
    }
    std::thread::spawn(move || {
        if scan {
            wifi::scan();
        }
        let st = wifi::read();
        let ip = wifi::ip_address();
        let _ = slint::invoke_from_event_loop(move || show(st, ip));
    });
}

fn show(st: Status, ip: Option<String>) {
    if !is_open() {
        return;
    }
    let ui = cx().ui();
    let p = ui.global::<WifiPanel>();
    // Keep the focus on the same network as the list reorders.
    let focused = NETWORKS.with(|n| n.borrow().get(p.get_index() as usize).map(|n| n.ssid.clone()));
    p.set_available(st.available);
    p.set_ip(ip.unwrap_or_default().into());
    p.set_current(st.connected().map(|n| n.ssid.clone()).unwrap_or_default().into());
    let rows: Vec<WifiNet> = st
        .networks
        .iter()
        .map(|n| WifiNet { ssid: n.ssid.clone().into(), signal: n.signal as i32, secured: n.secured, connected: n.connected, saved: n.profile.is_some() })
        .collect();
    let index = focused.and_then(|s| st.networks.iter().position(|n| n.ssid == s)).unwrap_or(0);
    p.set_networks(ModelRc::new(VecModel::from(rows)));
    p.set_index(index.min(st.networks.len().saturating_sub(1)) as i32);
    NETWORKS.with(|n| *n.borrow_mut() = st.networks);
}

fn activate(i: usize) {
    if CONNECTING.with(Cell::get) {
        return;
    }
    let Some(net) = NETWORKS.with(|n| n.borrow().get(i).cloned()) else { return };
    let ui = cx().ui();
    let p = ui.global::<WifiPanel>();
    if net.connected {
        p.set_message(tv("wifi.already", &[("name", net.ssid.clone().into())]).into());
        return;
    }
    if net.profile.is_some() || !net.secured {
        if net.profile.is_none() && net.auth.is_none() {
            p.set_message(tv("wifi.unsupported", &[("name", net.ssid.clone().into())]).into());
            return;
        }
        join(net, None);
        return;
    }
    if net.auth.is_none() {
        p.set_message(tv("wifi.unsupported", &[("name", net.ssid.clone().into())]).into());
        return;
    }
    let title = tv("wifi.password", &[("name", net.ssid.clone().into())]);
    dialogs::prompt(Prompt { title, ok: t("wifi.join"), secret: true, symbols: true, ..Default::default() }, move |pw| {
        if let Some(pw) = pw.filter(|p| !p.is_empty()) {
            join(net, Some(pw));
        }
        // Closing the prompt refocuses the page, which puts up its own hints; the panel takes the focus
        // back (wifi.slint) and its hints here.
        slint::Timer::single_shot(Duration::from_millis(120), || {
            if is_open() {
                set_hints();
            }
        });
    });
}

fn join(net: Network, password: Option<String>) {
    CONNECTING.with(|c| c.set(true));
    let ui = cx().ui();
    let p = ui.global::<WifiPanel>();
    p.set_busy(true);
    p.set_message(tv("wifi.connecting", &[("name", net.ssid.clone().into())]).into());
    let done = |ssid: String, ok: bool| {
        CONNECTING.with(|c| c.set(false));
        let ui = cx().ui();
        let p = ui.global::<WifiPanel>();
        p.set_busy(false);
        p.set_message(tv(if ok { "wifi.joined" } else { "wifi.failed" }, &[("name", ssid.into())]).into());
        refresh(false);
        // The top bar's icon and network name.
        crate::ui::sys::poll_status();
    };
    if cx().demo() {
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            DEMO.with(|d| {
                if let Some(st) = d.borrow_mut().as_mut() {
                    for n in &mut st.networks {
                        n.connected = n.ssid == net.ssid;
                        if n.connected {
                            n.profile.get_or_insert_with(|| n.ssid.clone());
                        }
                    }
                    st.networks = wifi::merge(std::mem::take(&mut st.networks));
                }
            });
            done(net.ssid, true);
        });
        return;
    }
    std::thread::spawn(move || {
        let ok = wifi::connect(&net, password.as_deref()).is_ok();
        let _ = slint::invoke_from_event_loop(move || done(net.ssid, ok));
    });
}
