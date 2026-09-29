//! Window bring-to-front / suspend / resume, and the small window commands.

use super::state::Inner;
use super::{Backend, WindowOp};
use std::sync::atomic::Ordering;

impl Inner {
    pub(super) fn bring_to_front(&self) {
        self.host.window(WindowOp::BringToFront);
        self.host.window(WindowOp::SetFullscreen(self.setting_bool("startFullscreen", true)));
    }

    /// While a game runs, Lounge gets out of the way: the interface is unloaded, the window
    /// minimised, and (Windows) every Lounge process drops to low CPU priority. It all comes back
    /// when the game exits or when you switch back to Lounge.
    pub(super) fn suspend_ui(&self) {
        if self.suspended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.host.window(WindowOp::SuspendUi);
        lounge_core::system::set_priority(&[std::process::id()], true);
    }

    pub(super) fn resume_ui(&self) {
        if self.suspended.swap(false, Ordering::SeqCst) {
            // The Electron version put priority back on resume; the Tauri port forgot to.
            lounge_core::system::set_priority(&[std::process::id()], false);
            self.host.window(WindowOp::ResumeUi);
        }
        self.bring_to_front();
    }
}

impl Backend {
    /// The shell reports the window gained focus: switching back to Lounge while a game runs brings
    /// the interface back.
    pub fn on_window_focused(&self) {
        if self.suspended.load(Ordering::SeqCst) {
            self.resume_ui();
        }
    }

    /// A second launch focuses/resumes the existing window, like Electron's second-instance.
    pub fn on_second_instance(&self) {
        if self.suspended.load(Ordering::SeqCst) {
            self.resume_ui();
        } else {
            self.bring_to_front();
        }
    }

    pub fn quit(&self) {
        // before-quit: kill playback, flush everything.
        if let Some(p) = self.play.lock().unwrap().as_ref() {
            p.session.kill();
        }
        self.helper.stop();
        self.flush_all();
        self.host.window(WindowOp::Quit);
    }

    pub fn minimize(&self) {
        self.host.window(WindowOp::Minimize);
    }

    pub fn toggle_fullscreen(&self) {
        self.host.window(WindowOp::ToggleFullscreen);
    }
}
