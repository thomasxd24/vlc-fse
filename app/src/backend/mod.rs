//! Lounge's backend: application state, the stores, sessions (VLC, games), background work
//! (scans, enrichment, updater, transfers) and every command the UI can issue. A port of the Tauri
//! shell's `src-tauri/app/src/lib.rs` (branch `tauri-migration`) with Tauri taken out: events go out
//! through [`Host::emit`], window operations through [`Host::window`], and commands are plain blocking
//! methods on [`Backend`]. Nothing in here depends on Slint either.
//!
//! Commands block (they may touch disk, the network or spawn processes), so the UI calls them off its
//! event-loop thread. Payloads are the same `serde_json::Value` shapes the web renderer used.

use serde_json::Value;

/// What the backend needs from the shell hosting it. Called from any thread.
pub trait Host: Send + Sync + 'static {
    /// One of the renderer's event channels: `state`, `now-playing`, `game`, `toast`, `update`,
    /// `transfers`, `legion-report`, `legion-state` — same names and payloads as the Tauri shell.
    fn emit(&self, event: &str, payload: Value);
    fn window(&self, op: WindowOp);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Minimize,
    /// Restore if minimised, show and focus.
    BringToFront,
    SetFullscreen(bool),
    ToggleFullscreen,
    /// A game is running: unload the interface (drop pages, stop animations) and minimise.
    SuspendUi,
    /// The game ended or Lounge was focused again: rebuild the interface and bring it to front.
    ResumeUi,
    /// Leave the event loop; the backend has already flushed its stores.
    Quit,
}
