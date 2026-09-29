//! Ported business logic from `src/*.js`, module by module, as Lounge moved off Electron onto Tauri.
//!
//! Each module here is a straight behavioural port of its `src/<name>.js` counterpart — same inputs,
//! same outputs, no redesign — verified against a Rust translation of that file's existing test suite.
//! Nothing in this crate depends on Tauri or a specific windowing/webview layer; the future `tauri`
//! binary crate (Windows-only, not yet scaffolded — see `../MIGRATION.md`) will call into this one.

pub mod apps;
mod fsops;
pub mod games;
pub mod gameinfo;
pub mod library;
pub mod locale;
pub mod metadata;
pub mod migrate;
pub mod parse;
pub mod remote;
pub mod steam;
pub mod store;
pub mod system;
pub mod transfer_plan;
pub mod transfers;
pub mod updater;
pub mod vdf;
pub mod viewmodel;
pub mod vlc;

/// A command for a console helper (PowerShell, netsh, reg…) that won't flash a console window over the
/// launcher. Off Windows it's a plain `Command`.
pub fn hidden_command<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}
