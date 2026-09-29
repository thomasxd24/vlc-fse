//! The UI context: the window, the backend (or the demo fixture), the latest state payload, and the
//! plumbing between the event-loop thread and worker threads.
//!
//! Everything UI-side runs on the event-loop thread and reaches the context through [`cx()`]. Backend
//! commands block, so [`Ctx::call`] runs them on a worker thread and hands the result back to a closure
//! on the event loop. Backend events (`state`, `now-playing`, `toast`…) arrive through [`UiHost`] and are
//! fanned out to whoever subscribed with [`Ctx::on_event`].

use crate::backend::{Backend, Host, WindowOp};
use crate::AppWindow;
use slint::ComponentHandle;
use serde_json::Value;
use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

type Listener = Box<dyn Fn(&Ctx, &Value)>;
type Pending = Box<dyn FnOnce(Box<dyn Any + Send>)>;

pub struct Ctx {
    pub ui: slint::Weak<AppWindow>,
    /// None in demo mode: commands are skipped (logged), state comes from `demo.rs`.
    pub backend: Option<Backend>,
    /// The latest `state` payload.
    pub state: RefCell<Value>,
    listeners: RefCell<HashMap<String, Vec<Rc<Listener>>>>,
    pending: RefCell<HashMap<u64, Pending>>,
    next_id: Cell<u64>,
}

thread_local! {
    static CTX: OnceCell<Rc<Ctx>> = const { OnceCell::new() };
}

/// The UI context. Only valid on the event-loop thread, after [`install`].
pub fn cx() -> Rc<Ctx> {
    CTX.with(|c| c.get().expect("ui context not installed").clone())
}

pub fn install(ctx: Ctx) -> Rc<Ctx> {
    let rc = Rc::new(ctx);
    CTX.with(|c| {
        let _ = c.set(rc.clone());
    });
    rc
}

impl Ctx {
    pub fn new(ui: &AppWindow, backend: Option<Backend>, state: Value) -> Self {
        Ctx {
            ui: ui.as_weak(),
            backend,
            state: RefCell::new(state),
            listeners: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
        }
    }

    pub fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window gone")
    }

    pub fn demo(&self) -> bool {
        self.backend.is_none()
    }

    /// Subscribe to a backend event (`state`, `now-playing`, `game`, `toast`, `update`, `transfers`,
    /// `legion-report`, `legion-state`). `state` listeners run after `ctx.state` has been replaced.
    pub fn on_event(&self, event: &str, f: impl Fn(&Ctx, &Value) + 'static) {
        self.listeners.borrow_mut().entry(event.to_string()).or_default().push(Rc::new(Box::new(f)));
    }

    /// Deliver an event to its listeners (also used by demo mode and tests to fake events).
    pub fn dispatch(&self, event: &str, payload: Value) {
        if event == "state" {
            *self.state.borrow_mut() = payload.clone();
        }
        let list: Vec<Rc<Listener>> = self.listeners.borrow().get(event).cloned().unwrap_or_default();
        for f in list {
            f(self, &payload);
        }
    }

    /// Re-run the `state` listeners with the current state (after a UI-only change such as a sort).
    pub fn refresh(&self) {
        let st = self.state.borrow().clone();
        self.dispatch("state", st);
    }

    /// Run a blocking backend command on a worker thread, then `then(result)` on the event loop.
    /// In demo mode the command is skipped and `then` is not called.
    pub fn call<R: Send + 'static>(&self, what: &str, f: impl FnOnce(&Backend) -> R + Send + 'static, then: impl FnOnce(R) + 'static) {
        let Some(backend) = self.backend.clone() else {
            eprintln!("[demo] skipped backend call: {what}");
            return;
        };
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.pending.borrow_mut().insert(
            id,
            Box::new(move |any: Box<dyn Any + Send>| {
                if let Ok(r) = any.downcast::<R>() {
                    then(*r);
                }
            }),
        );
        std::thread::spawn(move || {
            let r = f(&backend);
            let _ = slint::invoke_from_event_loop(move || {
                let ctx = cx();
                let cb = ctx.pending.borrow_mut().remove(&id);
                if let Some(cb) = cb {
                    cb(Box::new(r));
                }
            });
        });
    }

    /// Fire-and-forget backend command.
    pub fn send(&self, what: &str, f: impl FnOnce(&Backend) + Send + 'static) {
        self.call(what, f, |_| {});
    }

    // ---- Convenience readers over the state payload

    pub fn setting(&self, key: &str) -> Value {
        self.state.borrow().pointer(&format!("/settings/{key}")).cloned().unwrap_or(Value::Null)
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.setting(key).as_bool().unwrap_or(default)
    }

    /// A clone of a library list (`games`, `movies`, `shows`, `continueWatching`).
    pub fn library(&self, key: &str) -> Vec<Value> {
        self.state.borrow().pointer(&format!("/library/{key}")).and_then(Value::as_array).cloned().unwrap_or_default()
    }

    pub fn find(&self, list: &str, id: &str) -> Option<Value> {
        self.library(list).into_iter().find(|x| x.get("id").and_then(Value::as_str) == Some(id))
    }
}

/// The backend's view of the shell: events are forwarded to the event loop; window operations too.
pub struct UiHost {
    pub ui: slint::Weak<AppWindow>,
}

impl Host for UiHost {
    fn emit(&self, event: &str, payload: Value) {
        let event = event.to_string();
        let _ = slint::invoke_from_event_loop(move || {
            CTX.with(|c| {
                if let Some(ctx) = c.get() {
                    ctx.dispatch(&event, payload);
                }
            });
        });
    }

    fn window(&self, op: WindowOp) {
        let ui = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                super::window::apply(&ui, op);
            }
        });
    }
}

pub fn host_for(ui: &AppWindow) -> Arc<dyn Host> {
    Arc::new(UiHost { ui: ui.as_weak() })
}
