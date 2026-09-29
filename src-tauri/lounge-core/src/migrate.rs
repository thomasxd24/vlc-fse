//! The app was called Marquee, then Foyer; it's Lounge now. Each name keeps its own data folder
//! (`%APPDATA%\<Name>` on Windows), so on the first launch under a new name, carry the newest earlier
//! folder over: settings, library, progress, stats, servers, apps, artwork and the page's saved view
//! preferences. Direct port of `src/migrate.js`.

use std::path::{Component, Path, PathBuf};
use std::{fs, io};

pub const LEGACY_NAMES: &[&str] = &["Foyer", "Marquee"]; // newest first
const COPY_DIRS: &[&str] = &["artwork", "Local Storage"];

/// Copy the newest legacy data folder into `user_data`, unless it already has settings. Stores keep
/// absolute paths to cached artwork, so those are rewritten to point into the new folder.
///
/// Returns the folder migrated from, if any.
pub fn migrate_user_data(app_data: &Path, user_data: &Path, legacy_names: &[&str]) -> io::Result<Option<PathBuf>> {
    if user_data.join("settings.json").is_file() {
        return Ok(None);
    }
    let resolved_user_data = lexical_resolve(user_data);
    for name in legacy_names {
        let old = app_data.join(name);
        if lexical_resolve(&old) == resolved_user_data || !old.join("settings.json").is_file() {
            continue;
        }
        fs::create_dir_all(user_data)?;
        // JSON-escaped forms of the two folders, for rewriting paths inside the stores (the stores hold
        // paths inside already-JSON-serialized text, so the substitution must match how they're escaped
        // there too — matters on Windows, where a path's backslashes are JSON-escaped as `\\`).
        let from = json_escaped_path(&old);
        let to = json_escaped_path(user_data);
        for entry in fs::read_dir(&old)? {
            let entry = entry?;
            let file_name = entry.file_name();
            if !file_name.to_string_lossy().ends_with(".json") {
                continue;
            }
            let text = fs::read_to_string(entry.path())?;
            fs::write(user_data.join(&file_name), text.replace(&from, &to))?;
        }
        for d in COPY_DIRS {
            let src = old.join(d);
            if src.exists() {
                copy_dir_recursive(&src, &user_data.join(d))?;
            }
        }
        return Ok(Some(old));
    }
    Ok(None)
}

/// `path.resolve()`-alike: normalizes `.`/`..` segments without touching the filesystem (unlike
/// `fs::canonicalize`, which requires the path to exist and resolves symlinks — neither of which is
/// wanted here, since this runs before the folder in question is necessarily created).
fn lexical_resolve(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `JSON.stringify(path).slice(1, -1)`: the path as it would appear inside a JSON string's quotes.
fn json_escaped_path(p: &Path) -> String {
    let quoted = serde_json::to_string(&p.to_string_lossy()).expect("string serialization can't fail");
    quoted[1..quoted.len() - 1].to_string()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    struct Fixture {
        app_data: PathBuf,
        user_data: PathBuf,
        _tmp: tempfile::TempDir,
    }

    fn setup() -> Fixture {
        let tmp = tempdir().unwrap();
        let app_data = tmp.path().to_path_buf();
        let user_data = app_data.join("Lounge");
        Fixture { app_data, user_data, _tmp: tmp }
    }

    fn put(fixture: &Fixture, rel: &str, content: &str) {
        let p = fixture.app_data.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    // Parity with the JS test "carries Foyer data over to Lounge, pointing artwork paths at the new folder".
    #[test]
    fn carries_foyer_data_over_to_lounge_rewriting_artwork_paths() {
        let f = setup();
        let old_art = f.app_data.join("Foyer").join("artwork").join("p.jpg");
        put(&f, "Foyer/settings.json", r#"{"uiLanguage":"fr"}"#);
        put(&f, "Foyer/metadata.json", &serde_json::json!({"entries": {"x": {"poster": old_art.to_string_lossy()}}}).to_string());
        put(&f, "Foyer/stats.json", r#"{"sessions":[{"id":"g","minutes":5}]}"#);
        put(&f, "Foyer/artwork/p.jpg", "img");
        put(&f, "Foyer/Local Storage/leveldb/000003.log", "prefs");
        put(&f, "Foyer/Cache/data_0", "browser cache, not copied");
        put(&f, "Marquee/settings.json", r#"{"uiLanguage":"en"}"#);

        let migrated = migrate_user_data(&f.app_data, &f.user_data, LEGACY_NAMES).unwrap();
        assert_eq!(migrated, Some(f.app_data.join("Foyer")));

        let settings: serde_json::Value = serde_json::from_str(&fs::read_to_string(f.user_data.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["uiLanguage"], "fr");

        let metadata: serde_json::Value = serde_json::from_str(&fs::read_to_string(f.user_data.join("metadata.json")).unwrap()).unwrap();
        let poster = metadata["entries"]["x"]["poster"].as_str().unwrap();
        assert_eq!(poster, f.user_data.join("artwork").join("p.jpg").to_string_lossy());
        assert!(Path::new(poster).exists());

        assert!(f.user_data.join("stats.json").exists());
        assert!(f.user_data.join("Local Storage/leveldb/000003.log").exists());
        assert!(!f.user_data.join("Cache").exists());

        // Never twice: Lounge now has its own settings.
        assert_eq!(migrate_user_data(&f.app_data, &f.user_data, LEGACY_NAMES).unwrap(), None);
    }

    // Parity with the JS test "falls back to Marquee, and does nothing without earlier data".
    #[test]
    fn falls_back_to_marquee_and_does_nothing_without_earlier_data() {
        let a = setup();
        put(&a, "Marquee/settings.json", "{}");
        put(&a, "Marquee/progress.json", r#"{"items":{}}"#);
        assert_eq!(migrate_user_data(&a.app_data, &a.user_data, LEGACY_NAMES).unwrap(), Some(a.app_data.join("Marquee")));
        assert!(a.user_data.join("progress.json").exists());

        let b = setup();
        assert_eq!(migrate_user_data(&b.app_data, &b.user_data, LEGACY_NAMES).unwrap(), None);
        assert!(!b.user_data.exists());
    }
}
