//! View preferences (sorts, filters, the stats period) — the renderer kept these in localStorage as
//! `prefs`; here they're `ui-prefs.json` in the data directory (or in memory for the demo).

use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    static PREFS: RefCell<(Option<PathBuf>, Map<String, Value>)> = RefCell::new((None, defaults()));
}

fn defaults() -> Map<String, Value> {
    match json!({
        "movieSort": "title", "movieFilter": "all",
        "showSort": "title", "showFilter": "all",
        "gameSort": "recent", "gameFilter": "all",
        "statsPeriod": "week",
        "appSort": "recent", "appFilter": "all",
    }) {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

pub fn load(dir: Option<PathBuf>) {
    let file = dir.map(|d| d.join("ui-prefs.json"));
    let mut map = defaults();
    if let Some(saved) = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str::<Map<String, Value>>(&s).ok()) {
        map.extend(saved);
    }
    PREFS.with(|p| *p.borrow_mut() = (file, map));
}

pub fn get(key: &str) -> String {
    PREFS.with(|p| p.borrow().1.get(key).and_then(Value::as_str).unwrap_or("").to_string())
}

pub fn set(key: &str, value: &str) {
    PREFS.with(|p| {
        let mut p = p.borrow_mut();
        p.1.insert(key.to_string(), json!(value));
        if let Some(f) = &p.0 {
            let _ = std::fs::write(f, serde_json::to_string_pretty(&p.1).unwrap_or_default());
        }
    });
}
