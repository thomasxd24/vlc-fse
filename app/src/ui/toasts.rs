//! Toasts (bottom right): `toast(text, kind)` from the UI, and the backend's `toast` events, which carry
//! an i18n key and vars (`{ key, vars, kind }`) or plain `{ text, kind }`.

use super::ctx::Ctx;
use super::i18n::{tv, Arg};
use crate::{Toast, Toasts};
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const SHOW_FOR: Duration = Duration::from_millis(4200);
const MAX: usize = 3;

thread_local! {
    static MODEL: Rc<VecModel<Toast>> = Rc::new(VecModel::default());
    static NEXT: Cell<i32> = const { Cell::new(1) };
    static RECENT: std::cell::RefCell<Vec<(String, std::time::Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub fn install(ctx: &Ctx) {
    MODEL.with(|m| ctx.ui().global::<Toasts>().set_items(ModelRc::from(m.clone())));
    ctx.on_event("toast", |_, payload| show_payload(payload));
    // Toasts that queued up inside a state payload (e.g. while the UI was unloaded).
    ctx.on_event("state", |_, st| {
        if let Some(list) = st.get("toasts").and_then(Value::as_array) {
            for p in list {
                show_payload(p);
            }
        }
    });
}

pub fn show_payload(p: &Value) {
    // The backend both emits a toast and queues it into the next state payload (for a UI that wasn't
    // listening); show each only once.
    let sig = p.to_string();
    let now = std::time::Instant::now();
    let seen = RECENT.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|(_, at)| now.duration_since(*at) < Duration::from_secs(15));
        if r.iter().any(|(s, _)| *s == sig) {
            true
        } else {
            r.push((sig, now));
            false
        }
    });
    if seen {
        return;
    }
    let kind = p.get("kind").and_then(Value::as_str).unwrap_or("info");
    if let Some(key) = p.get("key").and_then(Value::as_str) {
        let vars: Vec<(String, Arg)> = p
            .get("vars")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), match v {
                        Value::Number(n) => Arg::N(n.as_f64().unwrap_or(0.0)),
                        Value::String(s) => Arg::S(s.clone()),
                        other => Arg::S(other.to_string()),
                    }))
                    .collect()
            })
            .unwrap_or_default();
        let refs: Vec<(&str, Arg)> = vars.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        toast(&tv(key, &refs), kind);
    } else if let Some(text) = p.get("text").and_then(Value::as_str) {
        toast(text, kind);
    }
}

/// Show a toast for a few seconds. `kind`: "info" | "error" | "success".
pub fn toast(text: &str, kind: &str) {
    let id = NEXT.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    MODEL.with(|m| {
        m.push(Toast { id, text: text.into(), kind: kind.into() });
        while m.row_count() > MAX {
            m.remove(0);
        }
    });
    slint::Timer::single_shot(SHOW_FOR, move || {
        MODEL.with(|m| {
            if let Some(i) = m.iter().position(|t| t.id == id) {
                m.remove(i);
            }
        });
    });
}
