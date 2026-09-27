//! Tiny JSON-file store with debounced, atomic writes. Direct port of `src/store.js`.
//!
//! The data shape is caller-defined arbitrary JSON (settings, library cache, stats, ...), so — like the
//! JS version — this holds a generic JSON object rather than a fixed struct; callers get/set individual
//! top-level keys with `serde_json::Value`s.
//!
//! `src/store.js` had no dedicated test file in `test/*.test.js`; the tests below are new, covering the
//! same behaviour read directly from that file (defaults merge, corrupt-file fallback, debounced vs.
//! immediate flush, atomic write-then-rename).

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(400);

struct State {
    data: Map<String, Value>,
    /// Bumped on every `save()`; a pending debounced flush checks this before writing, so only the
    /// most recent `save()` within the debounce window actually flushes (same effect as the JS
    /// version's `clearTimeout` + fresh `setTimeout`, without needing a cancellable timer handle).
    generation: u64,
}

#[derive(Clone)]
pub struct JsonStore {
    file: PathBuf,
    state: Arc<Mutex<State>>,
}

impl JsonStore {
    pub fn new(file: impl Into<PathBuf>, defaults: Map<String, Value>) -> Self {
        let file = file.into();
        let mut data = defaults;
        if let Ok(raw) = fs::read_to_string(&file) {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw) {
                for (k, v) in map {
                    data.insert(k, v);
                }
            }
            // Missing, corrupt, or not a JSON object: start from defaults, same as the JS version's
            // catch-and-ignore.
        }
        JsonStore { file, state: Arc::new(Mutex::new(State { data, generation: 0 })) }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.state.lock().unwrap().data.get(key).cloned()
    }

    pub fn set(&self, key: impl Into<String>, value: Value) {
        {
            let mut st = self.state.lock().unwrap();
            st.data.insert(key.into(), value);
        }
        self.save();
    }

    /// Schedule a write 400ms from now; a `set`/`save` in the meantime pushes it back further, same as
    /// the JS version's debounce.
    pub fn save(&self) {
        let my_generation = {
            let mut st = self.state.lock().unwrap();
            st.generation += 1;
            st.generation
        };
        let state = Arc::clone(&self.state);
        let file = self.file.clone();
        thread::spawn(move || {
            thread::sleep(DEBOUNCE);
            let is_still_latest = state.lock().unwrap().generation == my_generation;
            if is_still_latest {
                let _ = Self::write_now(&file, &state);
            }
        });
    }

    /// Write immediately, superseding any pending debounced write.
    pub fn flush(&self) -> std::io::Result<()> {
        {
            self.state.lock().unwrap().generation += 1;
        }
        Self::write_now(&self.file, &self.state)
    }

    fn write_now(file: &Path, state: &Arc<Mutex<State>>) -> std::io::Result<()> {
        let json = {
            let st = state.lock().unwrap();
            serde_json::to_string(&st.data).expect("a JSON Value always serializes")
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension(match file.extension() {
            Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
            None => "tmp".to_string(),
        });
        fs::write(&tmp, json)?;
        fs::rename(&tmp, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn defaults() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("uiLanguage".into(), json!("en"));
        m.insert("volume".into(), json!(80));
        m
    }

    #[test]
    fn starts_from_defaults_when_the_file_is_missing() {
        let dir = tempdir().unwrap();
        let store = JsonStore::new(dir.path().join("settings.json"), defaults());
        assert_eq!(store.get("uiLanguage"), Some(json!("en")));
        assert_eq!(store.get("volume"), Some(json!(80)));
        assert_eq!(store.get("nope"), None);
    }

    #[test]
    fn merges_saved_data_over_defaults_without_dropping_unset_defaults() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, r#"{"uiLanguage":"fr"}"#).unwrap();
        let store = JsonStore::new(&file, defaults());
        assert_eq!(store.get("uiLanguage"), Some(json!("fr")));
        assert_eq!(store.get("volume"), Some(json!(80))); // default, since the saved file didn't set it
    }

    #[test]
    fn falls_back_to_defaults_on_a_corrupt_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "not json").unwrap();
        let store = JsonStore::new(&file, defaults());
        assert_eq!(store.get("uiLanguage"), Some(json!("en")));
    }

    #[test]
    fn falls_back_to_defaults_when_the_file_holds_a_json_array() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "[1,2,3]").unwrap();
        let store = JsonStore::new(&file, defaults());
        assert_eq!(store.get("uiLanguage"), Some(json!("en")));
    }

    #[test]
    fn flush_writes_atomically_and_leaves_no_tmp_file_behind() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("nested").join("settings.json");
        let store = JsonStore::new(&file, defaults());
        store.set("uiLanguage", json!("de"));
        store.flush().unwrap();

        let on_disk: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(on_disk["uiLanguage"], "de");
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_fresh_store_reads_back_what_a_previous_one_flushed() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let first = JsonStore::new(&file, defaults());
        first.set("volume", json!(42));
        first.flush().unwrap();

        let second = JsonStore::new(&file, defaults());
        assert_eq!(second.get("volume"), Some(json!(42)));
    }

    #[test]
    fn save_debounces_and_eventually_flushes_on_its_own() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let store = JsonStore::new(&file, defaults());
        store.set("uiLanguage", json!("it")); // schedules a debounced flush
        assert!(!file.exists(), "shouldn't have flushed yet");
        thread::sleep(DEBOUNCE + Duration::from_millis(200));
        assert!(file.exists(), "debounced flush should have run by now");
        let on_disk: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(on_disk["uiLanguage"], "it");
    }
}
