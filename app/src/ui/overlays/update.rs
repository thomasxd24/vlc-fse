//! Updates: the "Lounge x.y.z is available" prompt and the full-screen update layer — the renderer's
//! `promptedVersions`, `updateNotes`, `maybePromptUpdate`, `startUpdate`, `renderUpdateLayer` and
//! `onUpdateState` (app.js), and `updateErrorText` (core.js).
//!
//! The prompt is asked once per version per session (never for a skipped version), and only when nothing
//! else is on screen — otherwise it retries every 5 s. The layer shows while an update the user started is
//! downloading / installing; "Continue in the background" hides it (the top bar then shows the progress),
//! and it comes back by itself once installing starts.
//!
//! Settings calls [`check`] ("Check for updates") and [`start`] ("Update now").

use super::super::ctx::{cx, Ctx};
use super::super::dialogs::{self, choice, Choice};
use super::super::i18n::{t, tv};
use super::super::toasts;
use crate::{GameLayer, Hint, Hints, Nav, NowPlaying, QuickMenu, Sys, UpdateLayer};
use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::Duration;

const RETRY: Duration = Duration::from_secs(5);
/// Statuses during which a started update keeps its layer up.
const LAYER_STATUSES: [&str; 6] = ["downloading", "ready", "elevating", "installing", "checking", "available"];

thread_local! {
    /// The latest update state (`S.update`).
    static UPDATE: RefCell<Value> = const { RefCell::new(Value::Null) };
    static PROMPTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The user started an update (`updateLayerOpen`).
    static WANT_LAYER: Cell<bool> = const { Cell::new(false) };
    /// The page's hints, put back when the layer closes.
    static SAVED_HINTS: RefCell<Option<ModelRc<Hint>>> = const { RefCell::new(None) };
    static BOOTED: Cell<bool> = const { Cell::new(false) };
    /// The top bar's busy line is showing the download.
    static OWN_STATUS: Cell<bool> = const { Cell::new(false) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    ui.global::<UpdateLayer>().on_hide(|| {
        WANT_LAYER.set(false);
        render();
    });

    ctx.on_event("state", |ctx, st| {
        // The demo fixture ships an available update (for Settings); don't greet every demo run with the
        // prompt — `event update {…}` in a script still brings it up.
        if !ctx.demo() {
            if let Some(up) = st.get("update") {
                UPDATE.with(|u| *u.borrow_mut() = up.clone());
            }
        }
        if !BOOTED.replace(true) {
            maybe_prompt(false);
        }
        // `sys` has just rewritten the busy line from this payload.
        OWN_STATUS.set(false);
        status_line();
    });

    ctx.on_event("update", |_, st| on_update_state(st.clone()));

    // Back (e.g. the mouse's back button) sends a download to the background; while installing the layer
    // just stays.
    super::super::on_close_overlay(|| {
        let ui = cx().ui();
        let layer = ui.global::<UpdateLayer>();
        if !layer.get_open() {
            return false;
        }
        if layer.get_can_hide() {
            WANT_LAYER.set(false);
            render();
        }
        true
    });
}

fn status_of(up: &Value) -> String {
    up.get("status").and_then(Value::as_str).unwrap_or("").to_string()
}

fn current() -> Value {
    UPDATE.with(|u| u.borrow().clone())
}

/// `onUpdateState`: a new state from the updater.
fn on_update_state(st: Value) {
    let prev = status_of(&current());
    let status = status_of(&st);
    UPDATE.with(|u| *u.borrow_mut() = st.clone());
    // Once installing starts, show it even if the user sent the download to the background.
    if status == "installing" {
        WANT_LAYER.set(true);
    }
    render();
    if status == "available" && prev != "available" {
        maybe_prompt(false);
    }
    if status == "error" && (prev == "downloading" || prev == "elevating") {
        toasts::toast(&error_text(st.get("error").and_then(Value::as_str).unwrap_or("")), "error");
    }
}

/// `updateErrorText`: "Update failed: …", in words for GitHub's hourly limit (shared by every device on the
/// network).
pub fn error_text(message: &str) -> String {
    let named_key = message.strip_prefix("upd.").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_alphanumeric() || c == '_'));
    if named_key {
        return t(message); // a reason the backend named by key
    }
    let lower = message.to_lowercase();
    let code = |c: &str| {
        lower.match_indices(c).any(|(i, _)| {
            let before = lower[..i].chars().next_back().is_none_or(|ch| !ch.is_alphanumeric() && ch != '_');
            let after = lower[i + c.len()..].chars().next().is_none_or(|ch| !ch.is_alphanumeric() && ch != '_');
            before && after
        })
    };
    if code("403") || code("429") || lower.contains("rate limit") {
        return t("upd.rateLimited");
    }
    tv("err.update", &[("message", message.into())])
}

/// `updateNotes`: the first few meaningful lines of the release notes, without Markdown noise.
pub fn notes(md: &str) -> String {
    md.lines()
        .map(clean_line)
        .filter(|l| {
            let low = l.to_lowercase();
            !l.is_empty() && !low.starts_with("full changelog") && !low.starts_with("what's changed")
        })
        .take(6)
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_line(line: &str) -> String {
    let mut l = line.trim_start().to_string();
    // ^#+\s*
    if l.starts_with('#') {
        l = l.trim_start_matches('#').trim_start().to_string();
    }
    // ^[-*]\s+ → "• "
    for bullet in ["- ", "* ", "-\t", "*\t"] {
        if let Some(rest) = l.strip_prefix(bullet) {
            l = format!("• {}", rest.trim_start());
            break;
        }
    }
    // **, __, `
    l = l.replace("**", "").replace("__", "").replace('`', "");
    // [text](url) → text
    l = strip_links(&l);
    // " by @user in https://…" (GitHub's generated notes)
    l = strip_by_author(&l);
    // bare URLs
    l = l.split(' ').filter(|w| !(w.starts_with("http://") || w.starts_with("https://"))).collect::<Vec<_>>().join(" ");
    l.trim().to_string()
}

fn strip_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let tail = &after[close + 1..];
        if tail.starts_with('(') {
            if let Some(end) = tail.find(')') {
                out.push_str(&rest[..open]);
                out.push_str(&after[..close]);
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&rest[..open + 1]);
        rest = after;
    }
    out.push_str(rest);
    out
}

fn strip_by_author(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(i) = out.find(" by @") {
        let start = i + " by @".len();
        let name_len = out[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').map(char::len_utf8).sum::<usize>();
        if name_len == 0 {
            break;
        }
        let mut end = start + name_len;
        for scheme in [" in https://", " in http://"] {
            if out[end..].starts_with(scheme) {
                end += scheme.len();
                end += out[end..].chars().take_while(|c| !c.is_whitespace()).map(char::len_utf8).sum::<usize>();
                break;
            }
        }
        out.replace_range(i..end, "");
    }
    out
}

/// Something else has the screen: a dialog, the quick menu, now playing, the running-game layer.
fn busy() -> bool {
    let ui = cx().ui();
    dialogs::is_open() || ui.global::<QuickMenu>().get_open() || ui.global::<NowPlaying>().get_open() || ui.global::<GameLayer>().get_open()
}

/// `maybePromptUpdate`: "Lounge x.y.z is available", asked once per version per session, only when nothing
/// else is going on.
fn maybe_prompt(force: bool) {
    let up = current();
    if status_of(&up) != "available" {
        return;
    }
    let version = up.get("version").and_then(Value::as_str).unwrap_or("").to_string();
    let skipped = cx().setting("skippedVersion");
    if !force && (PROMPTED.with(|p| p.borrow().contains(&version)) || skipped.as_str() == Some(version.as_str())) {
        return;
    }
    if busy() {
        slint::Timer::single_shot(RETRY, move || maybe_prompt(force));
        return;
    }
    PROMPTED.with(|p| p.borrow_mut().insert(version.clone()));
    let current_v = up.get("current").and_then(Value::as_str).unwrap_or("");
    let size = up.get("size").and_then(Value::as_f64).filter(|s| *s > 0.0).map(|s| format!(" ({} MB)", (s / 1_048_576.0).round())).unwrap_or_default();
    let notes = notes(up.get("notes").and_then(Value::as_str).unwrap_or(""));
    let text = [tv("upd.availableText", &[("current", current_v.into())]) + &size, notes].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
    let choices = vec![
        Choice { primary: true, icon: "refresh".into(), ..choice(&t("upd.now"), "now") },
        choice(&t("upd.later"), "later"),
        choice(&t("upd.skip"), "skip"),
    ];
    dialogs::choose(&tv("upd.availableTitle", &[("version", version.as_str().into())]), &text, choices, move |v| match v.as_deref() {
        Some("now") => start(),
        Some("skip") => cx().send("skip update", move |b| b.skip_update(&version)),
        _ => {}
    });
}

/// `startUpdate`: download (if needed) and install, with the layer up meanwhile.
pub fn start() {
    WANT_LAYER.set(true);
    render();
    cx().call(
        "install update",
        |b| b.install_update(),
        |r: Value| {
            if r.get("ok").and_then(Value::as_bool) == Some(true) {
                return;
            }
            WANT_LAYER.set(false);
            render();
            if let Some(key) = r.get("errorKey").and_then(Value::as_str) {
                toasts::show_payload(&json!({ "key": key, "vars": r.get("vars").cloned().unwrap_or(Value::Null), "kind": "error" }));
            }
        },
    );
}

/// Settings' "Check for updates".
pub fn check() {
    toasts::toast(&t("upd.checking"), "info");
    cx().call(
        "check update",
        |b| b.check_update(),
        |st: Value| {
            if st.is_null() {
                return;
            }
            UPDATE.with(|u| *u.borrow_mut() = st.clone());
            match status_of(&st).as_str() {
                "available" => maybe_prompt(true),
                "uptodate" => toasts::toast(&t("upd.upToDate"), "info"),
                "error" => toasts::toast(&error_text(st.get("error").and_then(Value::as_str).unwrap_or("")), "error"),
                _ => {}
            }
        },
    );
}

/// `renderUpdateLayer`.
fn render() {
    let up = current();
    let status = status_of(&up);
    let ui = cx().ui();
    let layer = ui.global::<UpdateLayer>();
    let show = WANT_LAYER.get() && LAYER_STATUSES.contains(&status.as_str());
    let was_open = layer.get_open();
    if !show {
        if was_open {
            layer.set_open(false);
            let hints = ui.global::<Hints>();
            if let Some(saved) = SAVED_HINTS.take() {
                hints.set_items(saved);
            }
            ui.global::<Nav>().invoke_focus_page();
        }
        status_line();
        return;
    }
    let installing = matches!(status.as_str(), "installing" | "ready" | "elevating");
    let pct = if installing { 100 } else { up.get("progress").and_then(Value::as_f64).unwrap_or(0.0).round() as i64 };
    let version = up.get("version").and_then(Value::as_str).unwrap_or("");
    layer.set_title(tv("upd.updatingTo", &[("version", version.into())]).into());
    layer.set_progress(pct as f32 / 100.0);
    layer.set_status(
        if status == "elevating" {
            t("upd.elevating")
        } else if installing {
            t("upd.installing")
        } else {
            tv("upd.downloading", &[("n", pct.into())])
        }
        .into(),
    );
    layer.set_can_hide(!installing);
    let hints = ui.global::<Hints>();
    if !was_open {
        SAVED_HINTS.set(Some(hints.get_items()));
    }
    let items: Vec<Hint> = if installing { Vec::new() } else { vec![Hint { button: "a".into(), label: t("hint.select").into() }] };
    hints.set_items(ModelRc::new(VecModel::from(items)));
    layer.set_open(true);
    status_line();
}

/// The top bar's "Update 42%" while a download runs in the background (`renderStatus`). Other busy states
/// (scanning…) are set by `sys`; this only takes the line over while downloading.
fn status_line() {
    let up = current();
    let ui = cx().ui();
    let sys = ui.global::<Sys>();
    if status_of(&up) == "downloading" && !ui.global::<UpdateLayer>().get_open() {
        let n = up.get("progress").and_then(Value::as_f64).unwrap_or(0.0).round() as i64;
        sys.set_busy(true);
        sys.set_busy_text(tv("upd.downloadingShort", &[("n", n.into())]).into());
        OWN_STATUS.set(true);
    } else if OWN_STATUS.replace(false) {
        // Hand the line back; the next state payload restores whatever else is going on.
        sys.set_busy(false);
        sys.set_busy_text("".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_lose_markdown_noise() {
        let md = "## What's Changed\n\n- **Faster** start-up by @someone in https://github.com/x/y/pull/1\n* See [the docs](https://x.y) and `code`\n\nFull Changelog: https://github.com/a/b\n# Fixes\nPlain https://example.com line\n";
        assert_eq!(notes(md), "• Faster start-up\n• See the docs and code\nFixes\nPlain line");
    }

    #[test]
    fn notes_keep_at_most_six_lines() {
        let md = (1..=9).map(|i| format!("- item {i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(notes(&md).lines().count(), 6);
    }

    #[test]
    fn rate_limits_are_spelled_out() {
        assert_eq!(error_text("HTTP 403 Forbidden"), t("upd.rateLimited"));
        assert_eq!(error_text("API rate limit exceeded"), t("upd.rateLimited"));
        assert_eq!(error_text("upd.declined"), t("upd.declined"));
        assert_eq!(error_text("disk full"), tv("err.update", &[("message", "disk full".into())]));
        assert_ne!(error_text("error 4030"), t("upd.rateLimited"));
    }
}
