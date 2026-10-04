//! STUB — to be replaced by the real screen.

use crate::ui::ctx::Ctx;

pub fn install(_ctx: &Ctx) {}

/// Launch an app (the renderer's `openApp`): toast "Opening…", then the backend's launch_app.
/// Home's "your apps" row calls this too.
pub fn open_app(_id: &str) {}
