//! Display formatting shared by screens — the renderer's `fmtTime`, `fmtRuntime`, `fmtPlaytime`,
//! `fmtAgo`, `remaining`, `epCode`, `seasonName`, `fmtSize`, `pct`, plus the clock and date.

use super::i18n::{fmt_number, lang, t, tv, Arg};
use chrono::{Datelike, Local, TimeZone, Timelike};
use serde_json::Value;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `1:02:03` / `2:03`.
pub fn time(sec: f64) -> String {
    let sec = sec.max(0.0).floor() as i64;
    let (h, m, s) = (sec / 3600, (sec % 3600) / 60, sec % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Minutes as "1 h 58 min" / "42 min".
pub fn runtime(min: i64) -> String {
    if min <= 0 {
        return String::new();
    }
    let (h, m) = (min / 60, min % 60);
    if h > 0 {
        tv("fmt.hm", &[("h", h.into()), ("m", m.into())])
    } else {
        tv("fmt.m", &[("m", m.into())])
    }
}

/// "42 minutes played" / "1.5 hours played" / "70 hours played".
pub fn playtime(min: i64) -> String {
    if min <= 0 {
        return String::new();
    }
    if min < 60 {
        return tv("game.minutesPlayed", &[("n", min.into())]);
    }
    let hours = min as f64 / 60.0;
    let n = if hours < 10.0 { (hours * 10.0).round() / 10.0 } else { hours.round() };
    tv("game.hoursPlayed", &[("n", n.into())])
}

/// `Intl.RelativeTimeFormat(lang, { numeric: 'auto' })` for a past or future timestamp (ms).
pub fn ago(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    let diff = (ms - now_ms()) as f64 / 1000.0;
    let abs = diff.abs();
    if abs < 90.0 {
        return t("time.justNow");
    }
    let (n, unit) = if abs < 3600.0 {
        ((diff / 60.0).round(), Unit::Minute)
    } else if abs < 86400.0 {
        ((diff / 3600.0).round(), Unit::Hour)
    } else if abs < 86400.0 * 30.0 {
        ((diff / 86400.0).round(), Unit::Day)
    } else if abs < 86400.0 * 365.0 {
        ((diff / (86400.0 * 30.0)).round(), Unit::Month)
    } else {
        ((diff / (86400.0 * 365.0)).round(), Unit::Year)
    };
    relative(n as i64, unit)
}

#[derive(Clone, Copy)]
enum Unit {
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

fn relative(n: i64, unit: Unit) -> String {
    let fr = lang() == "fr";
    let a = n.abs();
    // numeric: 'auto' names for ±1 (and ±2 days in French).
    match (fr, unit, n) {
        (false, Unit::Day, -1) => return "yesterday".into(),
        (false, Unit::Day, 1) => return "tomorrow".into(),
        (false, Unit::Month, -1) => return "last month".into(),
        (false, Unit::Month, 1) => return "next month".into(),
        (false, Unit::Year, -1) => return "last year".into(),
        (false, Unit::Year, 1) => return "next year".into(),
        (true, Unit::Day, -1) => return "hier".into(),
        (true, Unit::Day, -2) => return "avant-hier".into(),
        (true, Unit::Day, 1) => return "demain".into(),
        (true, Unit::Day, 2) => return "après-demain".into(),
        (true, Unit::Month, -1) => return "le mois dernier".into(),
        (true, Unit::Month, 1) => return "le mois prochain".into(),
        (true, Unit::Year, -1) => return "l’année dernière".into(),
        (true, Unit::Year, 1) => return "l’année prochaine".into(),
        _ => {}
    }
    let word = if fr {
        match unit {
            Unit::Minute => if a < 2 { "minute" } else { "minutes" },
            Unit::Hour => if a < 2 { "heure" } else { "heures" },
            Unit::Day => if a < 2 { "jour" } else { "jours" },
            Unit::Month => "mois",
            Unit::Year => if a < 2 { "an" } else { "ans" },
        }
    } else {
        match unit {
            Unit::Minute => if a == 1 { "minute" } else { "minutes" },
            Unit::Hour => if a == 1 { "hour" } else { "hours" },
            Unit::Day => if a == 1 { "day" } else { "days" },
            Unit::Month => if a == 1 { "month" } else { "months" },
            Unit::Year => if a == 1 { "year" } else { "years" },
        }
    };
    let num = fmt_number(a as f64);
    match (fr, n < 0) {
        (false, true) => format!("{num} {word} ago"),
        (false, false) => format!("in {num} {word}"),
        (true, true) => format!("il y a {num} {word}"),
        (true, false) => format!("dans {num} {word}"),
    }
}

fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

/// "22 min left" / "Almost done", from a progress object `{ time, length }`.
pub fn remaining(pr: &Value) -> String {
    let length = num(pr.get("length"));
    if length <= 0.0 {
        return String::new();
    }
    let left = ((length - num(pr.get("time"))) / 60.0).round().max(0.0) as i64;
    if left > 0 {
        tv("media.minLeft", &[("n", left.into())])
    } else {
        t("media.almostDone")
    }
}

/// Resume position 0..1 (the renderer's `pct` / 100).
pub fn pct(pr: &Value) -> f32 {
    let length = num(pr.get("length"));
    if length > 0.0 {
        (num(pr.get("time")) / length).min(1.0) as f32
    } else {
        0.0
    }
}

/// "S01E04" / "Special 3", with double episodes as "E04–05".
pub fn ep_code(e: &Value) -> String {
    let season = e.get("season").and_then(Value::as_i64).unwrap_or(0);
    let episode = e.get("episode").and_then(Value::as_i64).unwrap_or(0);
    let end = e.get("episodeEnd").and_then(Value::as_i64).map(|x| format!("–{x}")).unwrap_or_default();
    if season == 0 {
        tv("ep.special", &[("n", Arg::S(format!("{episode}{end}")))])
    } else {
        tv("ep.code", &[("s", season.into()), ("e", Arg::S(format!("{episode}{end}")))])
    }
}

pub fn season_name(n: i64) -> String {
    if n == 0 {
        t("season.specials")
    } else {
        tv("season.n", &[("n", n.into())])
    }
}

/// "1.4 GB" (binary steps, like the renderer).
pub fn size(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let shown = if v < 10.0 && i > 0 { (v * 10.0).round() / 10.0 } else { v.round() };
    format!("{} {}", fmt_number(shown), UNITS[i])
}

/// Stable 0..360 hue from a title (`hueOf` / `hue` in the renderer).
pub fn hue_of(s: &str) -> u32 {
    let mut x: u32 = 0;
    for c in s.encode_utf16() {
        x = x.wrapping_mul(31).wrapping_add(c as u32);
    }
    x % 360
}

/// The placeholder gradient's base colour: `hsl(h 42% 30%)`.
pub fn tint_of(title: &str) -> slint::Color {
    hsl(hue_of(title) as f32, 0.42, 0.30)
}

pub fn hsl(h: f32, s: f32, l: f32) -> slint::Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    slint::Color::from_rgb_u8(to(r), to(g), to(b))
}

/// The clock: "4:55 PM" (en) / "16:55" (fr). `two_digit` pads the hour ("04:55 PM"), as the top bar does.
pub fn clock(two_digit: bool) -> String {
    let now = Local::now();
    if lang() == "fr" {
        format!("{:02}:{:02}", now.hour(), now.minute())
    } else {
        let (pm, h) = now.hour12();
        let h = if two_digit { format!("{h:02}") } else { h.to_string() };
        format!("{h}:{:02} {}", now.minute(), if pm { "PM" } else { "AM" })
    }
}

const DAYS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DAYS_FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTHS_FR: [&str; 12] = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

/// "Tuesday, September 29" / "mardi 29 septembre".
pub fn long_date() -> String {
    let now = Local::now();
    let wd = now.weekday().num_days_from_monday() as usize;
    let m = now.month0() as usize;
    if lang() == "fr" {
        format!("{} {} {}", DAYS_FR[wd], now.day(), MONTHS_FR[m])
    } else {
        format!("{}, {} {}", DAYS_EN[wd], MONTHS_EN[m], now.day())
    }
}

/// A short date for a timestamp (ms): "12 Mar 2024" / "12 mars 2024".
pub fn short_date(ms: i64) -> String {
    let Some(d) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let m = d.month0() as usize;
    if lang() == "fr" {
        format!("{} {} {}", d.day(), MONTHS_FR[m], d.year())
    } else {
        format!("{} {} {}", d.day(), &MONTHS_EN[m][..3], d.year())
    }
}

/// Hour of day, for the greeting.
pub fn hour() -> u32 {
    Local::now().hour()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(time(3723.0), "1:02:03");
        assert_eq!(time(65.0), "1:05");
        assert_eq!(size(1536.0), "1.5 KB");
        assert_eq!(hue_of("Ember Keep"), {
            // Same as the JS: x = (x * 31 + code) >>> 0, then % 360.
            let mut x: u32 = 0;
            for c in "Ember Keep".chars() {
                x = x.wrapping_mul(31).wrapping_add(c as u32);
            }
            x % 360
        });
        assert!(ago(now_ms() - 2 * 3600 * 1000).contains("2 hours ago") || lang() == "fr");
    }
}
