//! Ported business logic from `src/*.js`, module by module, as Lounge moves off Electron onto Tauri.
//!
//! Each module here is a straight behavioural port of its `src/<name>.js` counterpart — same inputs,
//! same outputs, no redesign — verified against a Rust translation of that file's existing test suite.
//! Nothing in this crate depends on Tauri or a specific windowing/webview layer; the future `tauri`
//! binary crate (Windows-only, not yet scaffolded — see `../MIGRATION.md`) will call into this one.

pub mod games;
pub mod gameinfo;
pub mod library;
pub mod locale;
pub mod metadata;
pub mod migrate;
pub mod parse;
pub mod steam;
pub mod store;
pub mod vdf;
pub mod vlc;
