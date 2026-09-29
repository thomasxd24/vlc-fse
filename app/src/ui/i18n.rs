//! UI strings in English and French — the web renderer's `i18n.js`, as JSON (`app/i18n/*.json`).
//! `t("key")` / `tv("key", &[("name", "Ada".into())])` fill `{placeholders}`; an entry can be
//! `{ "one": …, "other": … }` for plurals, chosen by the `n` var (French treats 0 and 1 as singular).
//! Numbers are formatted for the language like `toLocaleString` did (except a var called `code`).

use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static EN_JSON: &str = include_str!("../../i18n/en.json");
static FR_JSON: &str = include_str!("../../i18n/fr.json");
static FRENCH: AtomicBool = AtomicBool::new(false);

fn dicts() -> &'static (Map<String, Value>, Map<String, Value>) {
    static D: OnceLock<(Map<String, Value>, Map<String, Value>)> = OnceLock::new();
    D.get_or_init(|| {
        let parse = |s: &str| serde_json::from_str::<Map<String, Value>>(s).expect("bad i18n json");
        (parse(EN_JSON), parse(FR_JSON))
    })
}

pub fn set_lang(lang: &str) {
    FRENCH.store(lang == "fr", Ordering::SeqCst);
}

pub fn lang() -> &'static str {
    if FRENCH.load(Ordering::SeqCst) {
        "fr"
    } else {
        "en"
    }
}

/// A placeholder value: text is inserted as is, numbers are formatted for the language.
#[derive(Debug, Clone)]
pub enum Arg {
    S(String),
    N(f64),
}

impl From<&str> for Arg {
    fn from(s: &str) -> Self {
        Arg::S(s.to_string())
    }
}
impl From<String> for Arg {
    fn from(s: String) -> Self {
        Arg::S(s)
    }
}
impl From<&String> for Arg {
    fn from(s: &String) -> Self {
        Arg::S(s.clone())
    }
}
macro_rules! num_arg {
    ($($t:ty),*) => { $(impl From<$t> for Arg { fn from(n: $t) -> Self { Arg::N(n as f64) } })* };
}
num_arg!(i32, i64, u32, u64, usize, f32, f64);

pub fn t(key: &str) -> String {
    tv(key, &[])
}

pub fn tv(key: &str, vars: &[(&str, Arg)]) -> String {
    let (en, fr) = dicts();
    let french = FRENCH.load(Ordering::SeqCst);
    let entry = if french { fr.get(key).or_else(|| en.get(key)) } else { en.get(key) };
    let Some(entry) = entry else { return key.to_string() };
    let n = vars.iter().find(|(k, _)| *k == "n").and_then(|(_, v)| match v {
        Arg::N(n) => Some(*n),
        Arg::S(s) => s.parse::<f64>().ok(),
    });
    let template = match entry {
        Value::String(s) => s.as_str(),
        Value::Object(o) => {
            let n = n.unwrap_or(0.0);
            let one = if french { n.abs() < 2.0 } else { n.abs() == 1.0 };
            o.get(if one { "one" } else { "other" }).and_then(Value::as_str).unwrap_or("")
        }
        _ => return key.to_string(),
    };
    fill(template, vars)
}

fn fill(template: &str, vars: &[(&str, Arg)]) -> String {
    if vars.is_empty() || !template.contains('{') {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if after[..end].chars().all(|c| c.is_alphanumeric() || c == '_') => {
                let name = &after[..end];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((k, Arg::N(n))) if *k != "code" => out.push_str(&fmt_number(*n)),
                    Some((_, Arg::N(n))) => out.push_str(&trim_float(*n)),
                    Some((_, Arg::S(s))) => out.push_str(s),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn trim_float(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        let s = format!("{:.3}", n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// `n.toLocaleString(lang)`: grouping (1,234 / 1 234) and the decimal mark (1.5 / 1,5), up to 3 decimals.
pub fn fmt_number(n: f64) -> String {
    let french = FRENCH.load(Ordering::SeqCst);
    let s = trim_float(n.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let sep = if french { '\u{202f}' } else { ',' };
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(sep);
        }
        grouped.push(c);
    }
    let mut out = if n < 0.0 { format!("-{grouped}") } else { grouped };
    if let Some(f) = frac {
        out.push(if french { ',' } else { '.' });
        out.push_str(&f);
    }
    out
}

/// For Slint's `T.lookup(key, vars, rev)`: vars alternate name, value. Values that look like numbers are
/// treated as numbers (formatted, and `n` picks the plural).
pub fn lookup_from_slint(key: &str, vars: &[String]) -> String {
    let args: Vec<(&str, Arg)> = vars
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| {
            let v = &c[1];
            let is_num = !v.is_empty() && v.parse::<f64>().is_ok() && !v.starts_with('+') && !(v.len() > 1 && v.starts_with('0') && !v.starts_with("0."));
            (c[0].as_str(), if is_num { Arg::N(v.parse().unwrap()) } else { Arg::S(v.clone()) })
        })
        .collect();
    tv(key, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // One test: the language is process-wide.
    fn plurals_placeholders_and_numbers() {
        set_lang("en");
        assert_eq!(t("tab.home"), "Home");
        assert_eq!(t("no.such.key"), "no.such.key");
        assert_eq!(fmt_number(1234.5), "1,234.5");
        let s = tv("pad.reconnected", &[("name", "Pad".into())]);
        assert!(s.starts_with("Pad"), "{s}");
        set_lang("fr");
        assert_eq!(fmt_number(1234.5), "1\u{202f}234,5");
        assert_eq!(t("tab.home"), "Accueil");
        set_lang("en");
        // Slint passes vars as alternating name, value strings.
        assert_eq!(lookup_from_slint("pad.hz", &["n".into(), "60".into()]), "60 Hz");
    }
}
