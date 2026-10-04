//! Servers and remote browsing: the renderer's VIEWS.remote / loadRemote / joinRemote, and from app.js
//! serverMenu, editServer (with PROTOCOL_LABELS), downloadRemote and the remote-* actions.
//!
//! Route: `remote` with id `<serverId>` (the server's start folder) or `<serverId><path>` (path starts
//! with `/`). Every folder visited is its own page on the stack, so Back climbs out; each keeps its
//! listing and focus here (keyed by route id) so coming back doesn't reload it. A forward visit reloads.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::dialogs::{self, choice, Choice, Prompt};
use crate::ui::i18n::{t, tv, Arg};
use crate::ui::model::{self, b, n, s};
use crate::ui::toasts::toast;
use crate::ui::{fmt, router};
use crate::{Backdrop, RemoteEntry, RemotePage};
use serde_json::{json, Map, Value};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;

const PROTOCOL_LABELS: [(&str, &str); 3] = [("sftp", "SFTP"), ("ftp", "FTP"), ("ftps", "FTPS")];

const VIDEO_EXT: [&str; 17] = ["mkv", "mp4", "m4v", "avi", "mov", "wmv", "mpg", "mpeg", "ts", "m2ts", "webm", "flv", "vob", "ogv", "3gp", "divx", "iso"];

#[derive(Default)]
struct Folder {
    server: String,
    path: String,
    entries: Option<Vec<Value>>,
    error: Option<String>,
    loading: bool,
    zone: i32,
    focus: i32,
}

thread_local! {
    static FOLDERS: RefCell<HashMap<String, Folder>> = RefCell::new(HashMap::new());
    static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<RemotePage>();
    page.on_open(|i| open(i as usize));
    page.on_options(|i| entry_options(i as usize));
    page.on_download_here(|| {
        if let Some((server, path)) = with_current(|f| (f.server.clone(), f.path.clone())) {
            download_remote(&server, &path, true);
        }
    });
    page.on_retry(|| {
        let Some(key) = current() else { return };
        FOLDERS.with(|m| {
            if let Some(f) = m.borrow_mut().get_mut(&key) {
                f.error = None;
                f.entries = None;
            }
        });
        load(&key);
    });
    ctx.on_event("route", |_, r| on_route(r));
    // The server's name can change (edit) while its folder is shown.
    ctx.on_event("state", |_, _| {
        if current().is_some() {
            show();
        }
    });
}

fn current() -> Option<String> {
    CURRENT.with(|c| c.borrow().clone())
}

fn with_current<R>(f: impl FnOnce(&Folder) -> R) -> Option<R> {
    let key = current()?;
    FOLDERS.with(|m| m.borrow().get(&key).map(f))
}

fn server_of(id: &str) -> Option<Value> {
    cx().state.borrow().get("servers").and_then(Value::as_array).and_then(|a| a.iter().find(|x| s(x, "id") == id).cloned())
}

/// `joinRemote`: a child path, with exactly one slash between.
fn join_remote(dir: &str, name: &str) -> String {
    let d = if dir.is_empty() { "/" } else { dir };
    format!("{}/{}", d.trim_end_matches('/'), name)
}

fn is_video(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.rsplit_once('.').map(|(_, ext)| VIDEO_EXT.contains(&ext)).unwrap_or(false)
}

fn on_route(r: &Value) {
    // Remember where the focus was in the folder we're leaving.
    if let Some(key) = current() {
        let ui = cx().ui();
        let page = ui.global::<RemotePage>();
        let (zone, focus) = (page.get_zone(), page.get_focus());
        FOLDERS.with(|m| {
            if let Some(f) = m.borrow_mut().get_mut(&key) {
                f.zone = zone;
                f.focus = focus;
            }
        });
    }
    if s(r, "name") != "remote" {
        CURRENT.with(|c| *c.borrow_mut() = None);
        return;
    }
    let id = s(r, "id").to_string();
    let (server, path) = match id.find('/') {
        Some(i) => (id[..i].to_string(), id[i..].to_string()),
        // The start folder: the server's root setting, else the top.
        None => (id.clone(), server_of(&id).map(|x| s(&x, "root").to_string()).filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into())),
    };
    let forward = b(r, "forward");
    FOLDERS.with(|m| {
        let mut m = m.borrow_mut();
        if forward || !m.contains_key(&id) {
            m.insert(id.clone(), Folder { server, path, zone: 1, ..Default::default() });
        }
    });
    CURRENT.with(|c| *c.borrow_mut() = Some(id.clone()));
    cx().ui().global::<Backdrop>().set_src("".into());
    let needs_load = FOLDERS.with(|m| m.borrow().get(&id).map(|f| f.entries.is_none() && f.error.is_none() && !f.loading).unwrap_or(false));
    show();
    if needs_load {
        load(&id);
    }
}

/// Push the current folder to Slint.
fn show() {
    let Some(key) = current() else { return };
    let ui = cx().ui();
    let page = ui.global::<RemotePage>();
    FOLDERS.with(|m| {
        let m = m.borrow();
        let Some(f) = m.get(&key) else { return };
        page.set_title(server_of(&f.server).map(|x| s(&x, "name").to_string()).unwrap_or_default().into());
        page.set_path(f.path.clone().into());
        let entries = f.entries.clone().unwrap_or_default();
        let status = if f.error.is_some() {
            "error"
        } else if f.entries.is_none() {
            "loading"
        } else if entries.is_empty() {
            "empty"
        } else {
            "list"
        };
        page.set_status(status.into());
        page.set_error(f.error.clone().unwrap_or_default().into());
        page.set_entries(model::model(
            entries
                .iter()
                .map(|e| {
                    let dir = b(e, "isDir");
                    let name = s(e, "name");
                    RemoteEntry {
                        name: name.into(),
                        kind: if dir { "dir" } else if is_video(name) { "video" } else { "other" }.into(),
                        size: if dir { String::new() } else { fmt::size(n(e, "size")) }.into(),
                    }
                })
                .collect(),
        ));
        let count = entries.len() as i32;
        page.set_zone(if count == 0 { 1 } else { f.zone });
        page.set_focus(f.focus.clamp(0, (count - 1).max(0)));
    });
    page.set_rev(page.get_rev() + 1);
}

fn load(key: &str) {
    let Some((server, path)) = FOLDERS.with(|m| {
        let mut m = m.borrow_mut();
        let f = m.get_mut(key)?;
        f.loading = true;
        Some((f.server.clone(), f.path.clone()))
    }) else {
        return;
    };
    show();
    let ctx = cx();
    let key = key.to_string();
    if ctx.demo() {
        // The demo listing is relative to the server's start folder.
        let root = server_of(&server).map(|x| s(&x, "root").to_string()).unwrap_or_default();
        let listing = crate::demo::remote_listing(path.strip_prefix(root.as_str()).filter(|_| !root.is_empty()).unwrap_or(&path));
        slint::Timer::single_shot(std::time::Duration::from_millis(250), move || loaded(&key, listing));
        return;
    }
    ctx.call("remote_list", move |b| b.remote_list(&server, Some(&path)), move |res| loaded(&key, res));
}

fn loaded(key: &str, res: Value) {
    FOLDERS.with(|m| {
        let mut m = m.borrow_mut();
        let Some(f) = m.get_mut(key) else { return };
        f.loading = false;
        match res {
            Value::Array(mut list) => {
                list.sort_by(|a, c| b(c, "isDir").cmp(&b(a, "isDir")).then_with(|| model::title_cmp(s(a, "name"), s(c, "name"))));
                f.entries = Some(list);
                f.error = None;
                f.zone = 1;
                f.focus = 0;
            }
            other => {
                f.error = Some(err_text(&other));
                f.entries = None;
            }
        }
    });
    if current().as_deref() == Some(key) {
        show();
    }
}

fn entry_at(i: usize) -> Option<(String, String, Value)> {
    let key = current()?;
    FOLDERS.with(|m| {
        let m = m.borrow();
        let f = m.get(&key)?;
        let e = f.entries.as_ref()?.get(i)?.clone();
        Some((f.server.clone(), f.path.clone(), e))
    })
}

/// `remote-open`: a folder opens as a new page, a file goes to the download flow.
fn open(i: usize) {
    let Some((server, path, e)) = entry_at(i) else { return };
    let child = join_remote(&path, s(&e, "name"));
    if b(&e, "isDir") {
        router::go("remote", &format!("{server}{child}"));
    } else {
        download_remote(&server, &child, false);
    }
}

/// X on an entry: Open (folders) and Download.
fn entry_options(i: usize) {
    let Some((server, path, e)) = entry_at(i) else { return };
    let dir = b(&e, "isDir");
    let mut choices = Vec::new();
    if dir {
        choices.push(Choice { icon: "folder".into(), primary: true, ..choice(&t("xfer.open"), "open") });
    }
    choices.push(Choice { icon: "download".into(), primary: !dir, ..choice(&t("xfer.download"), "download") });
    let child = join_remote(&path, s(&e, "name"));
    dialogs::choose(s(&e, "name"), "", choices, move |v| match v.as_deref() {
        Some("open") => router::go("remote", &format!("{server}{child}")),
        Some("download") => download_remote(&server, &child, dir),
        _ => {}
    });
}

// ---------------------------------------------------------------- Errors

/// A backend error code from the remote layer (`auth`, `ECONNREFUSED`…) as a sentence.
fn remote_error(code: &str) -> String {
    let key = format!("err.remote.{code}");
    let text = t(&key);
    if text == key {
        tv("err.remote.other", &[("message", code.into())])
    } else {
        text
    }
}

/// The text for a failed command result: `{ errorKey, vars }`, or the remote layer's `{ error: code }`.
pub(crate) fn err_text(v: &Value) -> String {
    if let Some(key) = v.get("errorKey").and_then(Value::as_str) {
        let vars: Vec<(String, Arg)> = v
            .get("vars")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, x)| {
                        let arg = match x {
                            Value::Number(num) => Arg::N(num.as_f64().unwrap_or(0.0)),
                            Value::String(st) => Arg::S(st.clone()),
                            other => Arg::S(other.to_string()),
                        };
                        (k.clone(), arg)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let refs: Vec<(&str, Arg)> = vars.iter().map(|(k, a)| (k.as_str(), a.clone())).collect();
        return tv(key, &refs);
    }
    remote_error(v.get("error").and_then(Value::as_str).unwrap_or("unknown"))
}

/// A listing came back (an array) rather than `{ error }`.
fn list_ok(v: &Value) -> bool {
    v.is_array()
}

// ---------------------------------------------------------------- Server menu

/// A saved server's menu (Settings › Servers, Transfers): Browse, Edit, Test, Forget host key, Remove.
pub fn server_menu(id: &str) {
    let Some(srv) = server_of(id) else { return };
    let id = id.to_string();
    let name = s(&srv, "name").to_string();
    let host_key = s(&srv, "hostKey").to_string();
    let text = if host_key.is_empty() {
        String::new()
    } else {
        let chunks: Vec<String> = host_key.chars().collect::<Vec<_>>().chunks(16).map(|c| c.iter().collect()).collect();
        tv("xfer.hostKey", &[("key", chunks.join(" ").into())])
    };
    let mut choices = vec![
        Choice { icon: "folder".into(), primary: true, ..choice(&t("xfer.browse"), "browse") },
        Choice { icon: "edit".into(), ..choice(&t("xfer.edit"), "edit") },
        Choice { icon: "refresh".into(), ..choice(&t("xfer.test"), "test") },
    ];
    if !host_key.is_empty() {
        choices.push(Choice { icon: "restart".into(), ..choice(&t("xfer.forgetKey"), "forget") });
    }
    choices.push(Choice { icon: "trash".into(), danger: true, ..choice(&t("xfer.removeServer"), "remove") });
    let title = name.clone();
    dialogs::choose(&title, &text, choices, move |v| match v.as_deref() {
        Some("browse") => router::go("remote", &id),
        Some("edit") => edit_server(Some(id)),
        Some("test") => {
            toast(&t("xfer.connecting"), "info");
            cx().call("test_server", move |b| b.test_server(&id), move |r| {
                if list_ok(&r) {
                    toast(&tv("xfer.connected", &[("name", name.into())]), "info");
                } else {
                    toast(&err_text(&r), "error");
                }
            });
        }
        Some("forget") => cx().call("forget_host_key", move |b| b.forget_host_key(&id), |_| toast(&t("xfer.keyForgotten"), "info")),
        Some("remove") => {
            dialogs::choose(
                &tv("xfer.removeConfirm", &[("name", name.into())]),
                &t("xfer.removeText"),
                vec![Choice { icon: "trash".into(), danger: true, ..choice(&t("xfer.removeServer"), "yes") }, choice(&t("common.cancel"), "")],
                move |ok| {
                    if ok.as_deref() == Some("yes") {
                        cx().send("remove_server", move |b| b.remove_server(&id));
                    }
                },
            );
        }
        _ => {}
    });
}

// ---------------------------------------------------------------- Add / edit a server

struct Edit {
    /// The server as last saved (None while adding a new one that hasn't been saved yet).
    existing: Option<Value>,
    draft: Map<String, Value>,
    /// None: keep what's stored.
    secret: Option<String>,
    /// Opened as "Add a server": once it connects, browse it.
    adding: bool,
}

/// Add (None) or edit a server (its id). Saving connects once to check it and to learn its host key; a
/// newly added server that connects is opened for browsing (the renderer's `add-server` action).
pub fn edit_server(existing: Option<String>) {
    let existing = existing.and_then(|id| server_of(&id));
    let draft = match &existing {
        Some(x) => x.as_object().cloned().unwrap_or_default(),
        None => match json!({ "name": "", "protocol": "sftp", "host": "", "port": 22, "username": "", "authType": "password", "keyPath": "", "root": "", "insecureTls": false }) {
            Value::Object(m) => m,
            _ => Map::new(),
        },
    };
    let adding = existing.is_none();
    edit_step(Edit { existing, draft, secret: None, adding });
}

fn ds<'a>(d: &'a Map<String, Value>, k: &str) -> &'a str {
    d.get(k).and_then(Value::as_str).unwrap_or("")
}

fn port_of(d: &Map<String, Value>) -> i64 {
    d.get("port").and_then(|p| p.as_i64().or_else(|| p.as_str().and_then(|x| x.parse().ok()))).unwrap_or(0)
}

fn edit_step(mut e: Edit) {
    let d = &e.draft;
    let protocol = ds(d, "protocol").to_string();
    let sftp = protocol == "sftp";
    let uses_key = sftp && ds(d, "authType") == "key";
    let has_secret = match &e.secret {
        Some(x) => !x.is_empty(),
        None => e.existing.as_ref().map(|x| b(x, "hasSecret")).unwrap_or(false),
    };
    let not_set = t("srv.notSet");
    let or_not_set = |v: &str| if v.is_empty() { not_set.clone() } else { v.to_string() };
    let row = |field: &str, label: &str, value: String, icon: &str| Choice { icon: icon.into(), ..choice(&format!("{label}: {value}"), field) };
    let mut choices = vec![
        row("protocol", &t("srv.protocol"), PROTOCOL_LABELS.iter().find(|(k, _)| *k == protocol).map(|(_, l)| l.to_string()).unwrap_or_default(), "link"),
        row("host", &t("srv.host"), or_not_set(ds(d, "host")), "server"),
        row("port", &t("srv.port"), port_of(d).to_string(), "server"),
        row("username", &t("srv.user"), or_not_set(ds(d, "username")), "edit"),
    ];
    if sftp {
        choices.push(row("authType", &t("srv.auth"), t(if uses_key { "srv.authKey" } else { "srv.authPassword" }), "edit"));
    }
    if uses_key {
        choices.push(row("keyPath", &t("srv.keyFile"), or_not_set(ds(d, "keyPath")), "file"));
    }
    choices.push(row("secret", &t(if uses_key { "srv.passphrase" } else { "srv.password" }), t(if has_secret { "srv.set" } else { "srv.notSet" }), "edit"));
    choices.push(row("root", &t("srv.root"), if ds(d, "root").is_empty() { t("srv.home") } else { ds(d, "root").to_string() }, "folder"));
    if protocol == "ftps" {
        choices.push(row("insecureTls", &t("srv.tls"), t(if b(&Value::Object(d.clone()), "insecureTls") { "srv.yes" } else { "srv.no" }), "check"));
    }
    let shown_name = [ds(d, "name"), ds(d, "host")].into_iter().find(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| not_set.clone());
    choices.push(row("name", &t("srv.name"), shown_name, "edit"));
    choices.push(Choice { icon: "check".into(), primary: true, ..choice(&t("srv.save"), "save") });
    choices.push(choice(&t("common.cancel"), ""));

    let title = match &e.existing {
        Some(x) => tv("srv.edit", &[("name", s(x, "name").into())]),
        None => t("srv.new"),
    };
    let text = if protocol == "ftp" { t("srv.ftpWarning") } else { String::new() };
    dialogs::choose(&title, &text, choices, move |v| {
        let Some(v) = v else { return };
        let demo = cx().demo();
        match v.as_str() {
            "protocol" => {
                let current = ds(&e.draft, "protocol").to_string();
                let list = PROTOCOL_LABELS.iter().map(|(value, label)| Choice { icon: if *value == current { "check".into() } else { "".into() }, ..choice(label, value) }).collect();
                dialogs::choose(&t("srv.protocol"), "", list, move |p| {
                    if let Some(p) = p.filter(|p| *p != current) {
                        // Keep a custom port; swap the default one along with the protocol.
                        if port_of(&e.draft) == if current == "sftp" { 22 } else { 21 } {
                            e.draft.insert("port".into(), json!(if p == "sftp" { 22 } else { 21 }));
                        }
                        e.draft.insert("protocol".into(), json!(p));
                    }
                    edit_step(e);
                });
            }
            "authType" => {
                e.draft.insert("authType".into(), json!(if uses_key { "password" } else { "key" }));
                edit_step(e);
            }
            "keyPath" => {
                if demo {
                    return edit_step(e);
                }
                cx().call("pick_key_file", |b| b.pick_key_file(), move |f| {
                    if let Some(f) = f {
                        e.draft.insert("keyPath".into(), json!(f));
                    }
                    edit_step(e);
                });
            }
            "insecureTls" => {
                let on = e.draft.get("insecureTls").and_then(Value::as_bool).unwrap_or(false);
                e.draft.insert("insecureTls".into(), json!(!on));
                edit_step(e);
            }
            "secret" => {
                let title = t(if uses_key { "srv.passphrase" } else { "srv.password" });
                dialogs::prompt(Prompt { title, symbols: true, secret: true, ..Default::default() }, move |val| {
                    if let Some(val) = val {
                        e.secret = Some(val);
                    }
                    edit_step(e);
                });
            }
            "port" => {
                dialogs::prompt(Prompt { title: t("srv.port"), value: port_of(&e.draft).to_string(), ..Default::default() }, move |val| {
                    if let Some(val) = val {
                        let val = val.trim();
                        if !val.is_empty() && val.chars().all(|c| c.is_ascii_digit()) {
                            e.draft.insert("port".into(), json!(val.parse::<i64>().unwrap_or(0)));
                        }
                    }
                    edit_step(e);
                });
            }
            field @ ("host" | "username" | "root" | "name") => {
                let label = match field {
                    "host" => "srv.host",
                    "username" => "srv.user",
                    "root" => "srv.root",
                    _ => "srv.name",
                };
                let placeholder = match field {
                    "host" => "nas.local".to_string(),
                    "root" => "/media/downloads".to_string(),
                    "name" => ds(&e.draft, "host").to_string(),
                    _ => String::new(),
                };
                let field = field.to_string();
                let value = ds(&e.draft, &field).to_string();
                dialogs::prompt(Prompt { title: t(label), value, placeholder, symbols: true, ..Default::default() }, move |val| {
                    if let Some(val) = val {
                        e.draft.insert(field, json!(val.trim()));
                    }
                    edit_step(e);
                });
            }
            "save" => save(e, has_secret),
            _ => {}
        }
    });
}

fn save(mut e: Edit, has_secret: bool) {
    // The checks the old main process made before saving.
    if ds(&e.draft, "host").trim().is_empty() {
        toast(&t("err.remote.noHost"), "error");
        return edit_step(e);
    }
    if !(1..=65535).contains(&port_of(&e.draft)) {
        toast(&t("err.remote.port"), "error");
        return edit_step(e);
    }
    if ds(&e.draft, "id").is_empty() {
        e.draft.insert("id".into(), json!(format!("srv-{:x}", fmt::now_ms())));
    }
    let id = ds(&e.draft, "id").to_string();
    let mut input = e.draft.clone();
    if let Some(secret) = &e.secret {
        input.insert("secret".into(), json!(secret));
    }
    let ctx = cx();
    if ctx.demo() {
        eprintln!("[demo] skipped backend call: save_server");
        return;
    }
    ctx.call("save_server", move |b| b.save_server(Value::Object(input)), move |_| {
        let mut existing = e.existing.take().unwrap_or_else(|| Value::Object(e.draft.clone()));
        existing["hasSecret"] = json!(has_secret);
        if let (Some(o), Some(name)) = (existing.as_object_mut(), e.draft.get("name")) {
            o.insert("name".into(), name.clone());
        }
        e.existing = Some(existing);
        e.secret = None;
        toast(&t("xfer.connecting"), "info");
        let test_id = id.clone();
        cx().call("test_server", move |b| b.test_server(&test_id), move |r| {
            if list_ok(&r) {
                let name = [ds(&e.draft, "name"), ds(&e.draft, "host")].into_iter().find(|x| !x.is_empty()).unwrap_or("").to_string();
                toast(&tv("xfer.connected", &[("name", name.into())]), "info");
                if e.adding {
                    router::go("remote", &id);
                }
            } else {
                toast(&err_text(&r), "error");
                edit_step(e);
            }
        });
    });
}

// ---------------------------------------------------------------- Download to the library

#[derive(Clone)]
struct Req {
    server: String,
    path: String,
    is_dir: bool,
    kind: Option<String>,
    library: Option<String>,
}

impl Req {
    fn json(&self) -> Value {
        json!({ "serverId": self.server, "path": self.path, "isDir": self.is_dir, "kind": self.kind, "library": self.library })
    }
}

/// Work out where a remote file or folder would go, confirm it, and queue the download.
pub fn download_remote(server: &str, path: &str, is_dir: bool) {
    plan_step(Req { server: server.into(), path: path.into(), is_dir, kind: None, library: None });
}

fn plan_step(req: Req) {
    toast(&t("xfer.planning"), "info");
    let body = req.json();
    cx().call("remote_plan", move |b| b.remote_plan(body), move |plan| on_plan(req, plan));
}

fn on_plan(mut req: Req, plan: Value) {
    if !b(&plan, "ok") {
        return toast(&err_text(&plan), "error");
    }
    let name = req.path.rsplit('/').next().unwrap_or("").to_string();
    let kind = s(&plan, "kind").to_string();
    let tv_kind = kind == "tv";
    let other = if tv_kind {
        Choice { icon: "edit".into(), ..choice(&t("xfer.asMovie"), "movie") }
    } else {
        Choice { icon: "edit".into(), ..choice(&t("xfer.asShow"), "tv") }
    };
    let title = tv("xfer.planTitle", &[("name", name.into())]);
    let root = plan.get("root").and_then(Value::as_str).map(String::from);
    let Some(root) = root else {
        dialogs::choose(
            &title,
            &t(if tv_kind { "err.remote.noTvLibrary" } else { "err.remote.noMovieLibrary" }),
            vec![Choice { icon: "plus".into(), primary: true, ..choice(&t("xfer.addLibrary"), "add") }, other, choice(&t("common.cancel"), "")],
            move |v| match v.as_deref() {
                Some("add") => add_library(if tv_kind { "tv" } else { "movies" }, move || plan_step(req)),
                Some(k) => {
                    req.kind = Some(k.to_string());
                    plan_step(req);
                }
                None => {}
            },
        );
        return;
    };
    let files = n(&plan, "files") as i64;
    let videos = n(&plan, "videos") as i64;
    let subs = files - videos;
    let mut counts = vec![tv(if tv_kind { "xfer.planEpisodes" } else { "xfer.planMovies" }, &[("n", videos.into())])];
    if subs > 0 {
        counts.push(tv("xfer.planSubs", &[("n", subs.into())]));
    }
    counts.push(fmt::size(n(&plan, "totalSize")));
    let folders: Vec<String> = model::arr(&plan, "folders").iter().filter_map(Value::as_str).map(String::from).collect();
    let into = if folders.len() > 1 {
        tv("xfer.planMore", &[("path", folders[0].clone().into()), ("n", (folders.len() - 1).into())])
    } else {
        tv("xfer.planInto", &[("path", folders.first().cloned().unwrap_or_else(|| root.clone()).into())])
    };
    let roots: Vec<String> = model::arr(&plan, "roots").iter().filter_map(Value::as_str).map(String::from).collect();
    let mut choices = vec![Choice { icon: "download".into(), primary: true, ..choice(&t("xfer.download"), "go") }, other];
    if roots.len() > 1 {
        choices.push(Choice { icon: "folder".into(), ..choice(&t("xfer.otherFolder"), "folder") });
    }
    choices.push(choice(&t("common.cancel"), ""));
    dialogs::choose(&title, &format!("{}\n{}", counts.join(" · "), into), choices, move |v| match v.as_deref() {
        Some("go") => {
            req.kind = Some(kind);
            req.library = Some(root);
            let body = req.json();
            cx().call("remote_download", move |b| b.remote_download(body), |r| {
                if b(&r, "ok") {
                    toast(&t("xfer.added"), "info");
                } else {
                    toast(&err_text(&r), "error");
                }
            });
        }
        Some("folder") => {
            let list = roots.iter().map(|p| Choice { icon: if *p == root { "check".into() } else { "folder".into() }, ..choice(p, p) }).collect();
            dialogs::choose(&t("xfer.otherFolder"), "", list, move |f| {
                if let Some(f) = f {
                    req.library = Some(f);
                    req.kind = Some(kind);
                }
                plan_step(req);
            });
        }
        Some(k) => {
            req.kind = Some(k.to_string());
            req.library = None;
            plan_step(req);
        }
        None => {}
    });
}

/// "Add a folder" from the download flow (the renderer's addLibrary): pick a folder, add it as a
/// library of `kind` ("movies" | "tv"), then carry on.
fn add_library(kind: &'static str, then: impl FnOnce() + 'static) {
    cx().call("pick_folder", |b| b.pick_folder(), move |dir| {
        let Some(dir) = dir else { return then() };
        let mut libs = cx().setting("libraries").as_array().cloned().unwrap_or_default();
        if libs.iter().any(|l| s(l, "path") == dir) {
            toast(&t("lib.already"), "info");
            return then();
        }
        libs.push(json!({ "path": dir, "type": kind }));
        let mut patch = Map::new();
        patch.insert("libraries".into(), Value::Array(libs));
        cx().call("save_settings", move |b| b.save_settings(patch), move |_| {
            toast(&tv("lib.added", &[("path", dir.into())]), "info");
            then();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_videos() {
        assert_eq!(join_remote("/", "Movies"), "/Movies");
        assert_eq!(join_remote("/media/", "a.mkv"), "/media/a.mkv");
        assert_eq!(join_remote("", "x"), "/x");
        assert!(is_video("Night.Freight.MKV"));
        assert!(!is_video("notes.txt"));
        assert!(!is_video("noext"));
    }
}
