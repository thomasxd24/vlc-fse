//! STUB — to be replaced by the real screen.

use crate::ui::ctx::Ctx;

pub fn install(_ctx: &Ctx) {}

/// STUB (the Remote port provides it): the options menu for a saved server (the renderer's serverMenu).
pub fn server_menu(_id: &str) {}

/// STUB (the Remote port provides it): add (None) or edit a server (the renderer's editServer).
pub fn edit_server(_id: Option<String>) {}
