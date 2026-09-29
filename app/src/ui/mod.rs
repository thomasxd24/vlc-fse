//! The Slint UI: wiring the window to the backend (or the demo fixture) and every screen's Rust side.
//!
//! Layout: `ctx` (the context, backend calls, events), `router` (navigation), `dialogs` / `toasts`,
//! `art` (image loading), `input` (controller → keys, feedback), `sys` (status, app facts), `i18n` /
//! `fmt` (strings and formatting), `model` (JSON → cards), `prefs` (view prefs), `actions` (what
//! activating things does), `pages/*` and `overlays/*` (one module per screen), `script` (scripted runs
//! for screenshots).

// (dead-code allowed while screens are being ported)
#![allow(dead_code)]
// helpers the screens still being ported will use

pub mod actions;
pub mod art;
pub mod ctx;
pub mod dialogs;
pub mod fmt;
pub mod i18n;
pub mod input;
pub mod model;
pub mod overlays;
pub mod pages;
pub mod prefs;
pub mod router;
pub mod script;
pub mod sys;
pub mod toasts;
pub mod window;

use crate::backend::{self, Backend};
use crate::{AppWindow, Util, T};
use ctx::Ctx;
use serde_json::Value;
use slint::ComponentHandle;
use std::cell::RefCell;

pub struct RunOpts {
    pub demo: Option<crate::demo::DemoOpts>,
    pub script: Option<String>,
    pub size: Option<(u32, u32)>,
}

type Closer = Box<dyn Fn() -> bool>;

thread_local! {
    static CLOSERS: RefCell<Vec<Closer>> = const { RefCell::new(Vec::new()) };
}

/// Overlays (quick menu, now playing…) register how to close themselves; Back closes the topmost open one.
/// `f` returns true if it was open and has closed.
pub fn on_close_overlay(f: impl Fn() -> bool + 'static) {
    CLOSERS.with(|c| c.borrow_mut().push(Box::new(f)));
}

pub fn overlays_close_top() -> bool {
    CLOSERS.with(|c| c.borrow().iter().rev().any(|f| f()))
}

pub fn run(opts: RunOpts) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    if let Some((w, h)) = opts.size {
        ui.window().set_size(slint::LogicalSize::new(w as f32, h as f32));
    }

    let (backend, state) = match &opts.demo {
        Some(d) => {
            crate::demo::ensure_artwork();
            (None, crate::demo::state(d))
        }
        None => {
            let weak = ui.as_weak();
            let first = backend::single_instance::acquire(move || {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        window::bring_to_front(&ui);
                    }
                    let ctx = ctx::cx();
                    ctx.send("second instance", |b| b.on_second_instance());
                });
            });
            if !first {
                return Ok(());
            }
            let b = Backend::start(ctx::host_for(&ui)).map_err(slint::PlatformError::Other)?;
            let st = b.get_state();
            (Some(b), st)
        }
    };

    prefs::load(backend.as_ref().map(|b| b.data_dir().to_path_buf()));
    let lang = state.get("lang").and_then(Value::as_str).unwrap_or("en").to_string();
    i18n::set_lang(&lang);

    let ctx = ctx::install(Ctx::new(&ui, backend.clone(), Value::Null));
    install_strings(&ui);
    art::install(&ui);
    router::install(&ctx);
    dialogs::install(&ctx);
    toasts::install(&ctx);
    input::install(&ctx);
    sys::install(&ctx);
    pages::install(&ctx);
    overlays::install(&ctx);

    ctx.dispatch("state", state.clone());
    let first_run = state.get("library").map(|l| {
        let empty = |k: &str| l.get(k).and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true);
        empty("games") && empty("movies") && empty("shows")
    });
    router::reset(if first_run == Some(true) { "welcome" } else { "home" });

    if let Some(b) = backend {
        if state.pointer("/settings/startFullscreen").and_then(Value::as_bool).unwrap_or(true) && opts.script.is_none() {
            ui.window().set_fullscreen(true);
        }
        // Focusing Lounge while a game runs brings the UI back (the Tauri shell's Focused(true) hook).
        {
            use slint::winit_030::{EventResult, WinitWindowAccessor};
            let b2 = b.clone();
            ui.window().on_winit_window_event(move |_, ev| {
                if let slint::winit_030::winit::event::WindowEvent::Focused(true) = ev {
                    let b = b2.clone();
                    std::thread::spawn(move || b.on_window_focused());
                }
                EventResult::Propagate
            });
        }
        ui.window().on_close_requested(move || {
            let b = b.clone();
            std::thread::spawn(move || b.quit());
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    if let Some(s) = &opts.script {
        script::run(s);
    }
    ui.run()
}

fn install_strings(ui: &AppWindow) {
    let t = ui.global::<T>();
    t.set_lang(i18n::lang().into());
    t.on_lookup(|key, vars, _rev| {
        use slint::Model;
        let vars: Vec<String> = vars.iter().map(|s| s.to_string()).collect();
        i18n::lookup_from_slint(&key, &vars).into()
    });
    let util = ui.global::<Util>();
    util.on_drop_last(|s| {
        let mut s = s.to_string();
        s.pop();
        s.into()
    });
    util.on_mask(|s| "•".repeat(s.chars().count()).into());
}
