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
