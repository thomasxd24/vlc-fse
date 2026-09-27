//! Manually-added game helpers: id generation, a readable default title from an .exe path, and
//! launch-options splitting. Direct port of the pure-function parts of `src/games.js`.
//!
//! `GameSession` (the JS `EventEmitter`-based class that tracks a running Steam or manual game — polling
//! Steam's `RunningAppID`, or watching a spawned child process) is **not** ported here. It has no
//! existing JS test to verify a port against, and how it should report state back to the UI (events?
//! polling? a channel?) is an actual design decision that depends on the Tauri `app` crate's structure,
//! which doesn't exist yet (see `../MIGRATION.md`). Porting it now, blind, would mean writing untestable
//! code against a runtime shape we'd likely have to redo anyway.

use sha1::{Digest, Sha1};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn manual_id(exe: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(exe.to_lowercase());
    hasher.update(now_millis().to_string());
    let digest = hasher.finalize();
    format!("game-{}", &hex::encode(digest)[..12])
}

fn is_generic_build_folder(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "bin" | "bin32" | "bin64" | "binaries" | "win64" | "win32" | "windows" | "x64" | "x86" | "game" | "release" | "shipping" | "retail"
    )
}

fn is_drive_letter(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some(':'), None) if c.is_ascii_alphabetic())
}

fn is_generic_container_folder(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "game" | "games" | "program files" | "program files (x86)" | "jeux")
}

/// Collapse runs of `.`, `_` and whitespace into a single space, then trim the ends.
fn tidy_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for c in s.chars() {
        if c == '.' || c == '_' || c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

/// Turn `"C:\Games\Hades II\Hades2.exe"` into a readable default title (`"Hades II"`).
pub fn title_from_exe(exe: &str) -> String {
    let exe_path = Path::new(exe);
    let base = exe_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

    // Skip build-output folders ("Game\Binaries\Win64\Game.exe") to reach the game's own folder.
    let mut dir = exe_path.parent().map(Path::to_path_buf);
    while let Some(d) = &dir {
        let Some(name) = d.file_name() else { break };
        if !is_generic_build_folder(&name.to_string_lossy()) {
            break;
        }
        match d.parent() {
            Some(p) if p != d => dir = Some(p.to_path_buf()),
            _ => break,
        }
    }

    let mut name = dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() || is_drive_letter(&name) || is_generic_container_folder(&name) {
        name = base;
    }
    tidy_spaces(&name)
}

/// Split a launch-options string, honouring double quotes.
pub fn split_args(s: &str) -> Vec<String> {
    fn strip_quotes(s: &str) -> String {
        let s = s.strip_prefix('"').unwrap_or(s);
        s.strip_suffix('"').unwrap_or(s).to_string()
    }

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        if chars[i] == '"' {
            // A quoted token requires a literal closing quote somewhere ahead, same as the JS
            // regex `"[^"]*"`; if there isn't one, this falls through to the plain-token case below,
            // exactly like the JS alternation would.
            if let Some(close_offset) = chars[i + 1..].iter().position(|&c| c == '"') {
                let close = i + 1 + close_offset;
                let token: String = chars[i..=close].iter().collect();
                out.push(strip_quotes(&token));
                i = close + 1;
                continue;
            }
        }
        let start = i;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        out.push(strip_quotes(&chars[start..i].iter().collect::<String>()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parity with the JS test "manual game titles and launch options" (test/games.test.js). That test
    // builds its paths with `path.sep`, i.e. platform-native separators; these use `/`, this crate's
    // separator when built for this (Linux) target. The logic itself is identical either way — Rust's
    // `std::path::Path` parses `\`-separated paths the same way Node's `path` module does when actually
    // compiled for Windows, but that specific parsing can only be exercised on a Windows build.
    #[test]
    fn manual_game_titles() {
        assert_eq!(title_from_exe("C:/Games/Hades II/Hades2.exe"), "Hades II");
        assert_eq!(title_from_exe("C:/Games/Celeste/bin/x64/Celeste.exe"), "Celeste");
        assert_eq!(title_from_exe("C:/Games/Tetris.exe"), "Tetris");
        assert_eq!(title_from_exe("D:/Emu/Dolphin/Binaries/Dolphin.exe"), "Dolphin");
    }

    #[test]
    fn launch_options() {
        assert_eq!(split_args(r#"-windowed --profile "My Profile""#), vec!["-windowed", "--profile", "My Profile"]);
        assert_eq!(split_args(""), Vec::<String>::new());
    }

    #[test]
    fn manual_id_has_the_expected_shape() {
        let id = manual_id("C:/Games/Foo/Foo.exe");
        assert!(id.starts_with("game-"));
        assert_eq!(id.len(), "game-".len() + 12);
        assert!(id["game-".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
