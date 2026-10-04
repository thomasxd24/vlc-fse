//! Playtime stats (the renderer's VIEWS.stats, loadStats, STATS_PERIODS, fmtMinutes, bucketLabel,
//! bucketSummary, statsChart, statsList — and renderer/stats.js, which turns the session log into
//! chart buckets, totals and top lists).
//!
//! The log (`get_stats`: `{ sessions: [{ kind, id, start, minutes }], items: { id: { type, title, … } } }`)
//! is fetched every time the page opens; in demo mode it comes from `demo::stats()`.

use crate::ui::ctx::{cx, Ctx};
use crate::ui::fmt;
use crate::ui::i18n::{lang, t, tv};
use crate::ui::model::{self, n, s};
use crate::ui::{actions, prefs, router};
use crate::{Backdrop, GridChip, StatBucket, StatGridLine, StatRow, StatTile, StatsData, StatsFocus, StatsPage};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde_json::{Map, Value};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

thread_local! {
    /// The last `get_stats` payload (None until it first arrives).
    static DATA: RefCell<Option<Value>> = const { RefCell::new(None) };
    /// What the lists show, so indices from Slint map back to items: (type, id) per row.
    static ROWS: RefCell<[Vec<(String, String)>; 2]> = const { RefCell::new([Vec::new(), Vec::new()]) };
}

const STATS_PERIODS: [(&str, &str); 4] = [("week", "stats.week"), ("month", "stats.month"), ("year", "stats.year"), ("all", "stats.all")];
const DAY: i64 = 86_400_000;

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let page = ui.global::<StatsPage>();
    page.on_period(|i| {
        let Some((value, _)) = STATS_PERIODS.get(i as usize) else { return };
        if prefs::get("statsPeriod") == *value {
            return;
        }
        prefs::set("statsPeriod", value);
        let ui = cx().ui();
        let page = ui.global::<StatsPage>();
        page.set_readout(-1);
        // A new period re-renders the chart; its default bar is the latest one.
        page.set_pos(StatsFocus { bar: i32::MAX, ..page.get_pos() });
        rebuild();
    });
    page.on_open(|list, row| {
        if let Some((kind, id)) = row_at(list, row) {
            router::go(&kind, &id);
        }
    });
    page.on_options(|list, row| {
        if let Some((kind, id)) = row_at(list, row) {
            actions::options(&kind, &id, "");
        }
    });
    ctx.on_event("route", |ctx, r| {
        if s(r, "name") != "stats" {
            return;
        }
        ctx.ui().global::<Backdrop>().set_src("".into());
        if model::b(r, "forward") {
            // A fresh visit starts on the current period's chip, like data-autofocus.
            let ui = ctx.ui();
            let page = ui.global::<StatsPage>();
            let period = STATS_PERIODS.iter().position(|(v, _)| *v == prefs::get("statsPeriod")).unwrap_or(0) as i32;
            page.set_pos(StatsFocus { zone: 0, period, bar: i32::MAX, list: 0, row: 0 });
            page.set_readout(-1);
        }
        load();
    });
    // Language changes (and library renames) re-render the strings.
    ctx.on_event("state", |_, _| {
        if DATA.with(|d| d.borrow().is_some()) {
            rebuild();
        }
    });
}

/// `loadStats`: fetch the session log, then render.
fn load() {
    let ctx = cx();
    if ctx.demo() {
        DATA.with(|d| *d.borrow_mut() = Some(crate::demo::stats()));
        rebuild();
        return;
    }
    if DATA.with(|d| d.borrow().is_some()) {
        rebuild();
    }
    ctx.call(
        "get_stats",
        |b| b.get_stats(),
        |v| {
            DATA.with(|d| *d.borrow_mut() = Some(v));
            rebuild();
        },
    );
}

fn row_at(list: i32, row: i32) -> Option<(String, String)> {
    ROWS.with(|r| r.borrow().get(list as usize).and_then(|l| l.get(row as usize).cloned()))
}

// ---------------------------------------------------------------- Aggregation (renderer/stats.js)

#[derive(Clone, Copy, PartialEq, Debug)]
enum Unit {
    Day,
    Month,
    Year,
}

#[derive(Clone, Debug)]
struct Bucket {
    start: i64,
    end: i64,
    unit: Unit,
    game: f64,
    watch: f64,
}

struct Ranked {
    id: String,
    minutes: f64,
    item: Value,
}

struct Totals {
    game: f64,
    watch: f64,
    sessions: usize,
    active_days: usize,
    daily_average: f64,
}

struct Aggregate {
    buckets: Vec<Bucket>,
    max: f64,
    totals: Totals,
    games: Vec<Ranked>,
    watched: Vec<Ranked>,
    has_log: bool,
}

fn date_of(ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ms).earliest().map(|d| d.date_naive()).unwrap_or_default()
}

/// Local midnight of a calendar date, in ms.
fn midnight(d: NaiveDate) -> i64 {
    let naive = d.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local.from_local_datetime(&naive).earliest().map(|x| x.timestamp_millis()).unwrap_or_else(|| naive.and_utc().timestamp_millis())
}

fn start_of_day(ms: i64) -> i64 {
    midnight(date_of(ms))
}

/// `new Date(y, m + offset, 1)` for the month containing `ms`.
fn start_of_month(ms: i64, offset: i32) -> i64 {
    let d = date_of(ms);
    let idx = d.year() * 12 + d.month0() as i32 + offset;
    midnight(NaiveDate::from_ymd_opt(idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1, 1).unwrap_or_default())
}

fn start_of_year(ms: i64, offset: i32) -> i64 {
    midnight(NaiveDate::from_ymd_opt(date_of(ms).year() + offset, 1, 1).unwrap_or_default())
}

/// Days ending today; built from calendar dates so daylight-saving days stay whole.
fn day_buckets(now: i64, count: i64) -> Vec<Bucket> {
    let today = date_of(now);
    (0..count)
        .rev()
        .map(|i| {
            let d = today - chrono::Duration::days(i);
            Bucket { start: midnight(d), end: midnight(d + chrono::Duration::days(1)), unit: Unit::Day, game: 0.0, watch: 0.0 }
        })
        .collect()
}

fn month_buckets(from: i64, now: i64) -> Vec<Bucket> {
    let mut out = Vec::new();
    let mut m = start_of_month(from, 0);
    while m <= now {
        let end = start_of_month(m, 1);
        out.push(Bucket { start: m, end, unit: Unit::Month, game: 0.0, watch: 0.0 });
        m = end;
    }
    out
}

fn year_buckets(from: i64, now: i64) -> Vec<Bucket> {
    let mut out = Vec::new();
    let mut y = start_of_year(from, 0);
    while y <= now {
        let end = start_of_year(y, 1);
        out.push(Bucket { start: y, end, unit: Unit::Year, game: 0.0, watch: 0.0 });
        y = end;
    }
    out
}

/// The chart's buckets for a period. 'all' uses months, or years once the log spans more than three.
fn buckets_for(period: &str, now: i64, first_start: Option<i64>) -> Vec<Bucket> {
    match period {
        "week" => day_buckets(now, 7),
        "month" => day_buckets(now, 30),
        "year" => month_buckets(start_of_month(now, -11), now),
        _ => {
            let from = first_start.unwrap_or(now).min(now);
            let months = month_buckets(from, now);
            if months.len() <= 36 {
                if months.len() >= 3 {
                    months
                } else {
                    month_buckets(start_of_month(now, -2), now)
                }
            } else {
                year_buckets(from, now)
            }
        }
    }
}

fn aggregate(data: &Value, period: &str, now: i64) -> Aggregate {
    let sessions = model::arr(data, "sessions");
    let empty = Map::new();
    let items = data.get("items").and_then(Value::as_object).unwrap_or(&empty);
    let first_start = sessions.iter().map(|x| n(x, "start") as i64).min();
    let mut buckets = buckets_for(period, now, first_start);
    let from = if period == "all" { i64::MIN } else { buckets[0].start };

    let mut per_game: HashMap<String, f64> = HashMap::new();
    let mut per_watch: HashMap<String, f64> = HashMap::new();
    let mut days = HashSet::new();
    let mut count = 0;
    for x in sessions {
        let start = n(x, "start") as i64;
        if start < from || start > now {
            continue;
        }
        let minutes = n(x, "minutes");
        let game = s(x, "kind") == "game";
        if let Some(b) = buckets.iter_mut().find(|k| start >= k.start && start < k.end) {
            if game {
                b.game += minutes;
            } else {
                b.watch += minutes;
            }
        }
        *(if game { &mut per_game } else { &mut per_watch }).entry(s(x, "id").to_string()).or_default() += minutes;
        days.insert(start_of_day(start));
        count += 1;
    }

    // All time also counts playtime Lounge didn't see (Steam's own total, time from before the log existed).
    if period == "all" {
        for (id, it) in items {
            let playtime = n(it, "playtime");
            if s(it, "type") == "game" && playtime > per_game.get(id).copied().unwrap_or(0.0) {
                per_game.insert(id.clone(), playtime);
            }
        }
    }

    let top = |map: HashMap<String, f64>| -> Vec<Ranked> {
        let mut v: Vec<Ranked> = map.into_iter().filter_map(|(id, minutes)| items.get(&id).map(|it| Ranked { id, minutes, item: it.clone() })).collect();
        v.sort_by(|a, b| b.minutes.partial_cmp(&a.minutes).unwrap_or(std::cmp::Ordering::Equal).then_with(|| model::title_cmp(s(&a.item, "title"), s(&b.item, "title"))));
        v
    };
    let games = top(per_game);
    let watched = top(per_watch);
    let sum = |list: &[Ranked]| list.iter().map(|x| x.minutes).sum::<f64>();
    // Totals: the chart's own sums for a period; for all time, the per-title totals (which include Steam's).
    let game_total = if period == "all" { sum(&games) } else { buckets.iter().map(|b| b.game).sum() };
    let watch_total = if period == "all" { sum(&watched) } else { buckets.iter().map(|b| b.watch).sum() };
    let span_days = if period == "all" {
        (((start_of_day(now) - start_of_day(first_start.unwrap_or(now))) as f64 / DAY as f64).round() + 1.0).max(1.0)
    } else {
        ((buckets[buckets.len() - 1].end - buckets[0].start) as f64 / DAY as f64).round()
    };
    Aggregate {
        max: buckets.iter().map(|b| b.game + b.watch).fold(0.0, f64::max),
        totals: Totals { game: game_total, watch: watch_total, sessions: count, active_days: days.len(), daily_average: ((game_total + watch_total) / span_days).round() },
        buckets,
        games,
        watched,
        has_log: !sessions.is_empty(),
    }
}

/// Round hour gridlines for a chart whose tallest bar is `max` minutes: [0, step, 2·step, …] covering max.
fn grid_steps(max: f64) -> Vec<f64> {
    const STEPS: [f64; 15] = [15.0, 30.0, 60.0, 120.0, 180.0, 240.0, 360.0, 600.0, 1200.0, 1800.0, 3000.0, 6000.0, 12000.0, 30000.0, 60000.0];
    let step = STEPS.iter().copied().find(|s| max / s <= 4.0).unwrap_or_else(|| (max / 4.0).ceil());
    let top = step.max((max / step).ceil() * step);
    let mut out = Vec::new();
    let mut v = 0.0;
    while v <= top {
        out.push(v);
        v += step;
    }
    out
}

// ---------------------------------------------------------------- Formatting

fn fmt_minutes(min: f64) -> String {
    if min != 0.0 {
        fmt::runtime(min.round() as i64)
    } else {
        tv("fmt.m", &[("m", 0.into())])
    }
}

const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MONTHS_FR: [&str; 12] = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];
const MONTHS_FR_SHORT: [&str; 12] = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
const DAYS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DAYS_FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];

/// `bucketLabel`: what's under a bar ("Mon", "14", "Oct", "2025") or, long, in the readout.
fn bucket_label(b: &Bucket, long: bool, period: &str) -> String {
    let d = date_of(b.start);
    let fr = lang() == "fr";
    let (m, wd) = (d.month0() as usize, d.weekday().num_days_from_monday() as usize);
    match b.unit {
        Unit::Year => d.year().to_string(),
        Unit::Month if long => format!("{} {}", if fr { MONTHS_FR[m] } else { MONTHS_EN[m] }, d.year()),
        Unit::Month => if fr { MONTHS_FR_SHORT[m].to_string() } else { MONTHS_EN[m][..3].to_string() },
        Unit::Day if long => {
            if fr {
                format!("{} {} {}", DAYS_FR[wd], d.day(), MONTHS_FR[m])
            } else {
                format!("{}, {} {}", DAYS_EN[wd], MONTHS_EN[m], d.day())
            }
        }
        Unit::Day if period == "week" => if fr { format!("{}.", &DAYS_FR[wd][..3]) } else { DAYS_EN[wd][..3].to_string() },
        Unit::Day => d.day().to_string(),
    }
}

fn bucket_summary(b: &Bucket, period: &str) -> String {
    let label = bucket_label(b, true, period);
    if b.game == 0.0 && b.watch == 0.0 {
        return format!("{label} · {}", t("stats.none"));
    }
    format!("{label} · {}", tv("stats.bucketLine", &[("playing", fmt_minutes(b.game).into()), ("watching", fmt_minutes(b.watch).into())]))
}

// ---------------------------------------------------------------- Rendering

fn stat_rows(list: &[Ranked]) -> Vec<StatRow> {
    let max = list.first().map(|x| x.minutes).unwrap_or(0.0);
    list.iter()
        .take(8)
        .map(|x| {
            let title = s(&x.item, "title");
            let hue = fmt::hue_of(title) as f32;
            StatRow {
                title: title.into(),
                art: s(&x.item, "poster").into(),
                icon: s(&x.item, "icon").into(),
                tint: fmt::hsl(hue, 0.42, 0.30),
                tint2: fmt::hsl(hue + 40.0, 0.48, 0.12),
                frac: if max > 0.0 { (x.minutes / max) as f32 } else { 0.0 },
                time: fmt_minutes(x.minutes).into(),
            }
        })
        .collect()
}

fn rebuild() {
    let ui = cx().ui();
    let page = ui.global::<StatsPage>();
    let Some(data) = DATA.with(|d| d.borrow().clone()) else {
        page.set_data(StatsData::default());
        return;
    };
    let period = match prefs::get("statsPeriod") {
        p if STATS_PERIODS.iter().any(|(v, _)| *v == p) => p,
        _ => "week".to_string(),
    };
    let a = aggregate(&data, &period, fmt::now_ms());

    let tile = |label: &str, value: String, sub: String| StatTile { label: label.into(), value: value.into(), sub: sub.into() };
    let mostly = |list: &[Ranked]| list.first().map(|x| tv("stats.mostly", &[("name", s(&x.item, "title").into())])).unwrap_or_default();
    let tiles = vec![
        tile(&t("stats.playing"), fmt_minutes(a.totals.game), mostly(&a.games)),
        tile(&t("stats.watching"), fmt_minutes(a.totals.watch), mostly(&a.watched)),
        tile(&t("stats.dailyAvg"), fmt_minutes(a.totals.daily_average), String::new()),
        tile(&t("stats.activeDays"), a.totals.active_days.to_string(), tv("stats.sessions", &[("n", a.totals.sessions.into())])),
    ];

    let grid = grid_steps(a.max);
    let top = *grid.last().unwrap_or(&15.0);
    let count = a.buckets.len();
    // Label every bucket for weeks and months of the year; thin out 30 days and long all-time spans.
    let every = if count <= 12 { 1 } else if count <= 31 { 5 } else { count.div_ceil(12) };
    let buckets = a
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| StatBucket {
            game: (b.game / top) as f32,
            watch: (b.watch / top) as f32,
            has_game: b.game > 0.0,
            has_watch: b.watch > 0.0,
            label: if i % every == 0 || i == count - 1 { bucket_label(b, false, &period) } else { String::new() }.into(),
            summary: bucket_summary(b, &period).into(),
        })
        .collect::<Vec<_>>();
    let axis = |v: f64| -> String {
        if v == 0.0 {
            "0".into()
        } else if v % 60.0 != 0.0 {
            fmt::runtime(v as i64)
        } else {
            tv("fmt.h", &[("h", (v / 60.0).into())])
        }
    };
    let grid_lines = grid.iter().map(|&v| StatGridLine { frac: (v / top) as f32, label: axis(v).into() }).collect();

    let periods = STATS_PERIODS
        .iter()
        .map(|(value, key)| GridChip { label: t(key).into(), value: (*value).into(), on: *value == period, ..Default::default() })
        .collect();

    ROWS.with(|r| {
        let ids = |list: &[Ranked]| list.iter().take(8).map(|x| (s(&x.item, "type").to_string(), x.id.clone())).collect::<Vec<_>>();
        *r.borrow_mut() = [ids(&a.games), ids(&a.watched)];
    });

    // Keep focus inside what's there now (the default bar is the latest one).
    let mut pos = page.get_pos();
    if count > 0 {
        pos.bar = pos.bar.clamp(0, count as i32 - 1);
    }
    let (ng, nw) = (a.games.len().min(8) as i32, a.watched.len().min(8) as i32);
    if pos.zone == 2 {
        let len = if pos.list == 0 { ng } else { nw };
        if len == 0 {
            pos.zone = 0;
        } else {
            pos.row = pos.row.min(len - 1);
        }
    }
    if pos.zone == 1 && !a.has_log {
        pos.zone = 0;
    }
    if page.get_readout() >= count as i32 {
        page.set_readout(-1);
    }

    page.set_data(StatsData {
        loaded: true,
        periods: model::model(periods),
        tiles: model::model(tiles),
        has_log: a.has_log,
        buckets: model::model(buckets),
        grid: model::model(grid_lines),
        games: model::model(stat_rows(&a.games)),
        watched: model::model(stat_rows(&a.watched)),
    });
    page.set_pos(pos);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grid_steps_cover_the_max() {
        assert_eq!(grid_steps(0.0), vec![0.0, 15.0]);
        assert_eq!(grid_steps(100.0), vec![0.0, 30.0, 60.0, 90.0, 120.0]);
        assert_eq!(*grid_steps(500.0).last().unwrap(), 540.0);
    }

    #[test]
    fn periods_have_the_right_bucket_counts() {
        let now = fmt::now_ms();
        assert_eq!(buckets_for("week", now, None).len(), 7);
        assert_eq!(buckets_for("month", now, None).len(), 30);
        assert_eq!(buckets_for("year", now, None).len(), 12);
        assert_eq!(buckets_for("all", now, Some(now)).len(), 3);
        assert!((14..=15).contains(&buckets_for("all", now, Some(now - 400 * DAY)).len()));
        assert!(buckets_for("all", now, Some(now - 4000 * DAY)).iter().all(|b| b.unit == Unit::Year));
    }

    #[test]
    fn aggregates_sessions() {
        let now = fmt::now_ms();
        let data = json!({
            "sessions": [
                {"kind": "game", "id": "g1", "start": now - 1000, "minutes": 30},
                {"kind": "game", "id": "g2", "start": now - DAY, "minutes": 50},
                {"kind": "watch", "id": "m1", "start": now - 2000, "minutes": 90},
                {"kind": "game", "id": "g1", "start": now - 100 * DAY, "minutes": 20},
            ],
            "items": {
                "g1": {"type": "game", "title": "One", "playtime": 600},
                "g2": {"type": "game", "title": "Two"},
                "m1": {"type": "movie", "title": "Film"},
            }
        });
        let w = aggregate(&data, "week", now);
        assert_eq!(w.totals.game, 80.0);
        assert_eq!(w.totals.watch, 90.0);
        assert_eq!(w.totals.sessions, 3);
        assert_eq!(w.games[0].id, "g2");
        assert_eq!(w.buckets.last().unwrap().watch, 90.0);
        let all = aggregate(&data, "all", now);
        // Steam's playtime counts for all time.
        assert_eq!(all.games[0].id, "g1");
        assert_eq!(all.games[0].minutes, 600.0);
        assert!(all.has_log);
    }
}
