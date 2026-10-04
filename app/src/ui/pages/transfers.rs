//! Transfers: the download queue and saved servers (the renderer's VIEWS.transfers, jobRow, jobStatus,
//! remoteErrorText, jobPct, onTransfers, plus jobMenu from app.js). The server menu and editor
//! (serverMenu / editServer) live with the Remote screen: `pages::remote::{server_menu, edit_server}`.
//!
//! Jobs come from the state's `transfers` and from the `transfers` event (progress ticks). A tick that
//! doesn't add, finish or remove a job updates the rows in place; anything else rebuilds the list,
//! keeping focus on the same job or server.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, choice, Choice};
use crate::ui::fmt;
use crate::ui::i18n::{t, tv};
use crate::ui::model::{self, b, n, s};
use crate::ui::pages::remote;
use crate::ui::router;
use crate::{Backdrop, TransfersPage, XferRow};
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static JOBS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    static SIG: RefCell<String> = const { RefCell::new(String::new()) };
    static JOB_ROWS: Rc<VecModel<XferRow>> = Rc::new(VecModel::default());
    /// The focus keys of the page's items, in order ("job:t1", "clear", "srv:x", "add").
    static KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<TransfersPage>();
    JOB_ROWS.with(|m| page.set_jobs(ModelRc::from(m.clone())));
    page.on_activate(|kind, i| match kind.as_str() {
        "job" => {
            if let Some(id) = job_id(i) {
                job_menu(&id);
            }
        }
        "clear" => cx().send("clear_transfers", |b| b.clear_transfers(None)),
        "server" => {
            if let Some(srv) = servers().get(i as usize) {
                router::go("remote", s(srv, "id"));
            }
        }
        "add" => remote::edit_server(None),
        _ => {}
    });
    page.on_options(|kind, i| match kind.as_str() {
        "job" => {
            if let Some(id) = job_id(i) {
                job_menu(&id);
            }
        }
        "server" => {
            if let Some(srv) = servers().get(i as usize) {
                remote::server_menu(s(srv, "id"));
            }
        }
        _ => {}
    });
    ctx.on_event("state", |_, st| {
        let jobs = model::arr(st, "transfers").to_vec();
        on_transfers(jobs, true);
    });
    ctx.on_event("transfers", |_, list| {
        on_transfers(list.as_array().cloned().unwrap_or_default(), false);
    });
    ctx.on_event("route", |ctx, r| {
        if s(r, "name") != "transfers" {
            return;
        }
        ctx.ui().global::<Backdrop>().set_src("".into());
        if b(r, "forward") {
            // A fresh visit starts on the first row (which is "Add a server" when the page is empty,
            // the renderer's data-autofocus).
            ctx.ui().global::<TransfersPage>().set_focus(0);
        }
    });
}

fn servers() -> Vec<Value> {
    cx().state.borrow().get("servers").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn job_id(i: i32) -> Option<String> {
    JOBS.with(|j| j.borrow().get(i as usize).map(|x| s(x, "id").to_string()))
}

fn active(j: &Value) -> bool {
    matches!(s(j, "status"), "running" | "queued")
}

/// `onTransfers`: progress ticks update rows in place; a job appearing, finishing or disappearing (or
/// the server list changing, on `state`) rebuilds the page.
fn on_transfers(jobs: Vec<Value>, from_state: bool) {
    let sig = jobs.iter().map(|j| format!("{}:{}", s(j, "id"), s(j, "status"))).collect::<Vec<_>>().join(",");
    let changed = SIG.with(|x| std::mem::replace(&mut *x.borrow_mut(), sig.clone()) != sig);
    let rows: Vec<XferRow> = jobs.iter().map(job_row).collect();
    JOBS.with(|j| *j.borrow_mut() = jobs);
    if changed || from_state {
        rebuild(rows);
    } else {
        JOB_ROWS.with(|m| {
            for (i, r) in rows.into_iter().enumerate() {
                if i < m.row_count() {
                    m.set_row_data(i, r);
                }
            }
        });
    }
}

fn rebuild(rows: Vec<XferRow>) {
    let ui = cx().ui();
    let page = ui.global::<TransfersPage>();
    let old_key = KEYS.with(|k| k.borrow().get(page.get_focus().max(0) as usize).cloned());

    let jobs = JOBS.with(|j| j.borrow().clone());
    let servers = servers();
    let finished = jobs.iter().any(|j| !active(j));
    let mut keys: Vec<String> = jobs.iter().map(|j| format!("job:{}", s(j, "id"))).collect();
    if finished {
        keys.push("clear".into());
    }
    keys.extend(servers.iter().map(|x| format!("srv:{}", s(x, "id"))));
    keys.push("add".into());

    let server_rows = servers
        .iter()
        .map(|x| {
            let user = match s(x, "username") {
                "" => String::new(),
                u => format!("{u}@"),
            };
            let root = match s(x, "root") {
                "" => String::new(),
                r => format!("  ·  {r}"),
            };
            XferRow {
                id: s(x, "id").into(),
                badge: s(x, "protocol").to_uppercase().into(),
                name: s(x, "name").into(),
                desc: format!("{user}{}:{}{root}", s(x, "host"), port_of(x)).into(),
                bar: -1.0,
                icon: "server".into(),
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();

    JOB_ROWS.with(|m| m.set_vec(rows));
    page.set_servers(model::model(server_rows));
    page.set_finished(finished);
    // Keep focus on the same job / server / button; otherwise stay at the same place in the list.
    if let Some(key) = old_key {
        if let Some(i) = keys.iter().position(|k| *k == key) {
            page.set_focus(i as i32);
        } else {
            page.set_focus(page.get_focus().min(keys.len() as i32 - 1));
        }
    }
    KEYS.with(|k| *k.borrow_mut() = keys);
}

fn port_of(x: &Value) -> String {
    match x.get("port") {
        Some(Value::Number(p)) => p.to_string(),
        Some(Value::String(p)) => p.clone(),
        _ => String::new(),
    }
}

/// `jobStatus`.
fn job_status(j: &Value) -> String {
    match s(j, "status") {
        "running" => {
            let mut bits = vec![
                tv("xfer.running", &[("n", (n(j, "fileIndex") + 1.0).into()), ("total", n(j, "files").into())]),
                tv("xfer.of", &[("done", fmt::size(n(j, "bytesDone")).into()), ("total", fmt::size(n(j, "bytesTotal")).into())]),
            ];
            if n(j, "rate") > 0.0 {
                bits.push(tv("xfer.rate", &[("rate", fmt::size(n(j, "rate")).into())]));
            }
            bits.join(" · ")
        }
        "queued" => format!("{} · {}", t("xfer.queued"), fmt::size(n(j, "bytesTotal"))),
        "done" if n(j, "skipped") > 0.0 => tv("xfer.doneSkipped", &[("n", n(j, "skipped").into())]),
        "done" => format!("{} · {}", t("xfer.done"), fmt::size(n(j, "bytesTotal"))),
        "error" => tv("xfer.error", &[("message", remote_error_text(j.get("error").unwrap_or(&Value::Null)).into())]),
        _ => t("xfer.cancelled"),
    }
}

/// `remoteErrorText`: error codes (e.g. "auth") read as sentences; anything else is shown as is.
fn remote_error_text(code: &Value) -> String {
    let code = match code {
        Value::String(c) => c.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let key = format!("err.remote.{code}");
    match t(&key) {
        s if s == key => code,
        s => s,
    }
}

/// `jobPct`.
fn job_pct(j: &Value) -> i64 {
    let total = n(j, "bytesTotal");
    if total > 0.0 {
        ((n(j, "bytesDone") / total) * 100.0).floor().min(100.0) as i64
    } else if s(j, "status") == "done" {
        100
    } else {
        0
    }
}

/// `jobRow`.
fn job_row(j: &Value) -> XferRow {
    let status = s(j, "status");
    let act = active(j);
    XferRow {
        id: s(j, "id").into(),
        badge: if s(j, "kind") == "tv" { t("lib.tv") } else { t("tab.movies") }.to_uppercase().into(),
        name: s(j, "title").into(),
        desc: job_status(j).into(),
        error: status == "error",
        bar: if act || status == "done" { job_pct(j) as f32 / 100.0 } else { -1.0 },
        done: status == "done",
        value: if act { format!("{}%", job_pct(j)) } else { String::new() }.into(),
        icon: if status == "done" { "check" } else { "" }.into(),
    }
}

/// `jobMenu`: cancel a running / queued job; retry or remove a finished one.
fn job_menu(id: &str) {
    let Some(j) = JOBS.with(|x| x.borrow().iter().find(|j| s(j, "id") == id).cloned()) else { return };
    let mut text = vec![job_status(&j)];
    text.extend(model::arr(&j, "folders").iter().take(3).filter_map(Value::as_str).map(String::from));
    let choices = if active(&j) {
        vec![Choice { icon: "stop".into(), danger: true, ..choice(&t("xfer.cancel"), "cancel") }, choice(&t("common.cancel"), "")]
    } else {
        let mut c = Vec::new();
        if matches!(s(&j, "status"), "error" | "cancelled") {
            c.push(Choice { icon: "refresh".into(), primary: true, ..choice(&t("xfer.retry"), "retry") });
        }
        c.push(Choice { icon: "trash".into(), ..choice(&t("xfer.remove"), "remove") });
        c
    };
    let id = id.to_string();
    dialogs::choose(s(&j, "title"), &text.join("\n"), choices, move |v| match v.as_deref() {
        Some("cancel") => cx().send("cancel_transfer", move |b| b.cancel_transfer(&id)),
        Some("retry") => cx().send("retry_transfer", move |b| {
            b.retry_transfer(&id);
        }),
        Some("remove") => cx().send("clear_transfers", move |b| b.clear_transfers(Some(&id))),
        _ => {}
    });
}
