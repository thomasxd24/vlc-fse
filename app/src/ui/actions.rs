//! What activating things does — opening, playing, the options menu (X / right-click / long-press),
//! adding and editing games. The renderer's `ACTIONS`, `openOptions`, `playMovie`, `playEpisode`,
//! `playGame`, `editGame`, `pickArtwork`, `matchSteam`, `removeGame`, `addGame`.
//!
//! STUB: the real flows are to be ported here.

use super::ctx::cx;
use super::dialogs::{self, choice};
use super::router;

/// The options menu for an item: kind is "movie" | "show" | "game" | "episode" | "app".
pub fn options(kind: &str, id: &str, _show_id: &str) {
    let (kind, id) = (kind.to_string(), id.to_string());
    let title = cx().find(&format!("{kind}s"), &id).and_then(|v| v.get("title").and_then(|t| t.as_str()).map(String::from)).unwrap_or_default();
    dialogs::choose(&title, "", vec![choice("Open", "open")], move |v| {
        if v.as_deref() == Some("open") {
            router::go(&kind, &id);
        }
    });
}

/// "Add a game": pick an executable, then open its page.
pub fn add_game() {}

// Shared entry points other screens call (home, search, now playing…). Signatures are fixed; the
// detail-pages port fills in the bodies.

/// Play a movie. mode: "" (ask resume / start over when there's progress), "resume", "start".
pub fn play_movie(_id: &str, _mode: &str) {}

/// Play an episode of a show; mode as for `play_movie`.
pub fn play_episode(_show_id: &str, _id: &str, _mode: &str) {}

/// Launch a game.
pub fn play_game(_id: &str) {}

/// Pick a language for audio / subtitles. kind: "audio" | "subtitles"; extra: leading (label, value)
/// choices (e.g. "Default", "Off"). Calls back with the chosen code, or None if cancelled.
pub fn choose_language(_kind: &str, _current: &str, _extra: Vec<(String, String)>, _done: impl FnOnce(Option<String>) + 'static) {}
