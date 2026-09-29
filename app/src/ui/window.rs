//! Window operations the backend asks for (minimise, fullscreen, suspend while a game runs…).

use crate::backend::WindowOp;
use crate::{App, AppWindow};
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;

pub fn apply(ui: &AppWindow, op: WindowOp) {
    let w = ui.window();
    match op {
        WindowOp::Minimize => w.set_minimized(true),
        WindowOp::BringToFront => bring_to_front(ui),
        WindowOp::SetFullscreen(on) => w.set_fullscreen(on),
        WindowOp::ToggleFullscreen => w.set_fullscreen(!w.is_fullscreen()),
        WindowOp::SuspendUi => {
            // Drop every page and overlay (App.suspended removes the whole tree), free decoded artwork,
            // and get out of the game's way.
            ui.global::<App>().set_suspended(true);
            super::art::clear();
            w.set_minimized(true);
        }
        WindowOp::ResumeUi => {
            ui.global::<App>().set_suspended(false);
            bring_to_front(ui);
        }
        WindowOp::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

pub fn bring_to_front(ui: &AppWindow) {
    let w = ui.window();
    w.set_minimized(false);
    let _ = w.show();
    w.with_winit_window(|ww| {
        ww.focus_window();
    });
}
