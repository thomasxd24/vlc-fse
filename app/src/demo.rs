//! A made-up application state for developing and screenshotting every screen without Steam, VLC or
//! a library. Ports the stubbed state of the old `scripts/screenshots.js` (same titles, hues and
//! numbers) but in the shape the real backend emits: `state()` mirrors `build_state`, `stats()` is
//! `stats_data`, `remote_listing()` is `remote_list`, `now_playing()` is the `now-playing` event.
//!
//! Artwork fields are absolute filesystem paths to PNGs generated (deterministically) by
//! [`ensure_artwork`]; nothing here touches the network or the user's real data.

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::imageops;
use image::{Rgb, RgbImage, Rgba, RgbaImage};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resume {
    /// Home shows a game to resume (recently played games; empty Continue watching).
    Game,
    /// Home shows an episode to resume (games never played; Continue watching filled).
    Show,
}

pub struct DemoOpts {
    /// "en" | "fr"
    pub lang: &'static str,
    pub resume: Resume,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Definitions (from screenshots.js)

/// title, hue, days since last played, minutes played, genres
const GAME_DEFS: [(&str, u32, f64, u32, &str); 6] = [
    ("Starfall Drift", 30, 2.0 / 24.0, 4200, "Space · Roguelike"),
    ("Ember Keep", 190, 2.0, 9100, "Action RPG · Fantasy"),
    ("Tidewalker", 100, 6.0, 2600, "Adventure · Exploration"),
    ("Neon Courier", 340, 21.0, 15000, "Racing · Arcade"),
    ("Glass Orbit", 200, 35.0, 800, "Puzzle · Space"),
    ("Mossbound", 60, 50.0, 6400, "Platformer · Cozy"),
];

/// title, hue, year, runtime (min), rating
const MOVIE_DEFS: [(&str, u32, i32, u32, f64); 5] = [
    ("The Long Tide", 210, 2021, 118, 7.8),
    ("Paper Lanterns", 20, 2019, 102, 7.4),
    ("Night Freight", 280, 2023, 131, 8.1),
    ("Small Hours", 160, 2018, 96, 7.0),
    ("Copper Sky", 40, 2022, 109, 7.6),
];

const EPISODE_TITLES: [&str; 8] = ["Pilot", "The Long Way Round", "Static", "Salt and Iron", "Homecoming", "Low Tide", "The Quiet Room", "Afterglow"];

/// id, title, hue, seasons, watched-through, latest activity (days ago)
type ShowDef = (&'static str, &'static str, u32, u32, u32, f64);
const SHOW_DEFS: [ShowDef; 3] = [
    ("sh0", "Harbor Lights", 210, 3, 11, 1.0 / 24.0),
    ("sh1", "The Salt Road", 20, 2, 16, 3.0),
    ("sh2", "Quiet Signals", 300, 1, 3, 9.0),
];

/// name, kind, icon hue (None: no icon), hidden, last launched (days ago; 0 = never)
const APP_DEFS: [(&str, &str, Option<u32>, bool, f64); 6] = [
    ("Lantern Browser", "desktop", Some(200), false, 0.2),
    ("Mixer", "desktop", Some(320), false, 3.0),
    ("Photo Roll", "store", Some(90), false, 12.0),
    ("File Explorer", "desktop", None, false, 1.0),
    ("Notes", "store", None, false, 0.0),
    ("Terminal", "desktop", None, true, 30.0),
];

// ---------------------------------------------------------------------------
// Artwork

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Art {
    Poster,
    Wide,
    Shot,
    Icon,
}

impl Art {
    fn size(self) -> (u32, u32) {
        match self {
            Art::Poster => (400, 600),
            Art::Wide => (960, 540),
            Art::Shot => (640, 360),
            Art::Icon => (128, 128),
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Art::Poster => "poster",
            Art::Wide => "wide",
            Art::Shot => "shot",
            Art::Icon => "icon",
        }
    }
}

pub fn artwork_dir() -> PathBuf {
    std::env::temp_dir().join("lounge-demo-art")
}

fn art_file(kind: Art, hue: u32) -> String {
    artwork_dir().join(format!("{}-{}.png", kind.prefix(), hue % 360)).to_string_lossy().into_owned()
}

/// Every image the fixture references.
fn art_jobs() -> Vec<(Art, u32)> {
    let mut jobs = Vec::new();
    for (_, hue, ..) in GAME_DEFS {
        jobs.push((Art::Poster, hue));
        jobs.push((Art::Wide, hue));
        for n in 1..=4 {
            jobs.push((Art::Shot, (hue + n * 30) % 360));
        }
    }
    for (_, hue, ..) in MOVIE_DEFS {
        jobs.push((Art::Poster, hue));
        jobs.push((Art::Wide, hue));
    }
    for (_, _, hue, ..) in SHOW_DEFS {
        jobs.push((Art::Poster, hue));
        jobs.push((Art::Wide, hue));
        for e in 1..=8 {
            jobs.push((Art::Shot, (hue + e * 17) % 360));
        }
    }
    for (.., icon, _, _) in APP_DEFS {
        if let Some(h) = icon {
            jobs.push((Art::Icon, h));
        }
    }
    jobs.sort_by_key(|&(k, h)| (k.prefix(), h % 360));
    jobs.dedup_by_key(|&mut (k, h)| (k, h % 360));
    jobs
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

/// A soft light blob: a flat-coloured layer whose alpha is a hard ellipse, then blurred.
fn blob(w: u32, h: u32, (cx, cy, rx, ry): (f32, f32, f32, f32), rgb: [f32; 3], sigma: f32) -> RgbaImage {
    let mut layer = RgbaImage::from_pixel(w, h, Rgba([rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, 0]));
    for (x, y, p) in layer.enumerate_pixels_mut() {
        let dx = (x as f32 - cx * w as f32) / (rx * w as f32);
        let dy = (y as f32 - cy * h as f32) / (ry * h as f32);
        if dx * dx + dy * dy <= 1.0 {
            p.0[3] = 255;
        }
    }
    imageops::blur(&layer, sigma)
}

fn render_art(kind: Art, hue: u32) -> RgbImage {
    let (w, h) = kind.size();
    let hue = hue as f32;
    let c = |dh: f32, l: f32| hsl(hue + dh, 0.7, l / 100.0);
    // Work on a small canvas (gradient + blurred blobs), then scale up.
    let (sw, sh) = ((w / 6).max(16), (h / 6).max(16));
    let (a, b) = (c(0.0, 26.0), c(40.0, 8.0));
    let mut small = RgbImage::new(sw, sh);
    for (x, y, p) in small.enumerate_pixels_mut() {
        let t = ((x as f32 / sw as f32) + (y as f32 / sh as f32)) / 2.0;
        *p = Rgb([0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * t) as u8));
    }
    let sigma = sw as f32 / 10.0;
    for layer in [blob(sw, sh, (0.7, 0.4, 0.2, 0.22), c(120.0, 58.0), sigma), blob(sw, sh, (0.25, 0.62, 0.14, 0.14), c(200.0, 50.0), sigma)] {
        for (x, y, p) in small.enumerate_pixels_mut() {
            let l = layer.get_pixel(x, y).0;
            let al = l[3] as f32 / 255.0;
            for (dst, src) in p.0.iter_mut().zip(l) {
                *dst = (*dst as f32 * (1.0 - al) + src as f32 * al) as u8;
            }
        }
    }
    // Scale up (bilinear, fixed point) and darken below the wave, in one pass over raw rows.
    let edges: Vec<u32> = (0..w)
        .map(|x| {
            let u = x as f32 / w as f32;
            (h as f32 * (0.78 + 0.07 * (u * std::f32::consts::TAU * 1.1 + 0.6).sin() - 0.04 * u)).max(0.0) as u32
        })
        .collect();
    let xs: Vec<(usize, usize, u32)> = (0..w)
        .map(|x| {
            let f = ((x as f32 + 0.5) * sw as f32 / w as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let i = f as usize;
            (i * 3, (i + 1).min(sw as usize - 1) * 3, ((f - i as f32) * 256.0) as u32)
        })
        .collect();
    let src = small.as_raw();
    let stride = sw as usize * 3;
    let mut out = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        let f = ((y as f32 + 0.5) * sh as f32 / h as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, fy) = (f as usize, ((f - f.floor()) * 256.0) as u32);
        let y1 = (y0 + 1).min(sh as usize - 1);
        let (r0, r1) = (&src[y0 * stride..(y0 + 1) * stride], &src[y1 * stride..(y1 + 1) * stride]);
        let row = &mut out[(y * w * 3) as usize..((y + 1) * w * 3) as usize];
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let (a, b, fx) = xs[x];
            let dark = y >= edges[x];
            for c in 0..3 {
                let top = r0[a + c] as u32 * (256 - fx) + r0[b + c] as u32 * fx;
                let bot = r1[a + c] as u32 * (256 - fx) + r1[b + c] as u32 * fx;
                let v = (top * (256 - fy) + bot * fy) >> 16;
                px[c] = if dark { (v * 3 / 5) as u8 } else { v as u8 };
            }
        }
    }
    RgbImage::from_raw(w, h, out).expect("buffer size")
}

fn write_art(kind: Art, hue: u32) {
    let path = PathBuf::from(art_file(kind, hue));
    if path.is_file() {
        return;
    }
    let img = render_art(kind, hue);
    // Write beside, then rename, so concurrent processes never see a half-written PNG.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let ok = std::fs::File::create(&tmp).ok().and_then(|f| {
        let enc = PngEncoder::new_with_quality(std::io::BufWriter::new(f), CompressionType::Fast, FilterType::Sub);
        img.write_with_encoder(enc).ok()
    });
    if ok.is_some() {
        let _ = std::fs::rename(&tmp, &path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

static ART_LOCK: Mutex<()> = Mutex::new(());

/// Generate the PNGs (skipping ones that already exist) and return their directory.
pub fn ensure_artwork() -> PathBuf {
    let _guard = ART_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = artwork_dir();
    let _ = std::fs::create_dir_all(&dir);
    let jobs: Vec<_> = art_jobs().into_iter().filter(|&(k, h)| !PathBuf::from(art_file(k, h)).is_file()).collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(4);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(&(k, h)) = jobs.get(i) else { break };
                write_art(k, h);
            });
        }
    });
    dir
}

// ---------------------------------------------------------------------------
// Library

fn progress(time: f64, length: f64, watched: bool, updated_at: i64) -> Value {
    let resumable = !watched && time >= 60.0 && (length == 0.0 || time < length * 0.9);
    json!({"time": time, "length": length, "watched": watched, "updatedAt": updated_at, "resumable": resumable})
}

fn games(resume: Resume, now: i64) -> Vec<Value> {
    GAME_DEFS
        .iter()
        .enumerate()
        .map(|(i, &(title, hue, days, minutes, genres))| {
            let steam = i != 3;
            let install_dir = if steam { format!("C:\\Program Files (x86)\\Steam\\steamapps\\common\\{title}") } else { format!("D:\\Games\\{title}") };
            let appid = if steam { json!(format!("{}", 1_100_000 + i * 137)) } else { Value::Null };
            let shots: Vec<Value> = (1..=4).map(|n| json!({"thumb": art_file(Art::Shot, hue + n * 30), "full": ""})).collect();
            json!({
                "id": format!("g{i}"),
                "type": "game",
                "source": if steam { "steam" } else { "manual" },
                "appid": appid,
                "title": title,
                "poster": art_file(Art::Poster, hue),
                "hero": art_file(Art::Wide, hue),
                "logo": Value::Null,
                "header": art_file(Art::Wide, hue),
                "icon": Value::Null,
                "overview": "Made-up sample game for the screenshots. Nothing here is a real title.",
                "about": "",
                "genres": genres.split(" · ").collect::<Vec<_>>(),
                "developers": ["Lantern Works"],
                "publishers": ["Lantern Works"],
                "releaseDate": "12 Mar 2024",
                "metacritic": 70 + ((i * 7) % 25),
                "controller": "full",
                "screenshots": shots,
                "steamAppId": appid,
                "playtime": minutes,
                "lastPlayed": if resume == Resume::Game { now - (days * DAY as f64) as i64 } else { 0 },
                "addedAt": now - (10 + i as i64) * DAY,
                "exe": if steam { Value::Null } else { json!(format!("{install_dir}\\{title}.exe")) },
                "args": "",
                "installDir": install_dir,
                "override": {},
                "favorite": i == 1,
                "hidden": false,
            })
        })
        .collect()
}

fn movies(now: i64) -> Vec<Value> {
    MOVIE_DEFS
        .iter()
        .enumerate()
        .map(|(i, &(title, hue, year, runtime, rating))| {
            let pr = match i {
                0 => progress(2400.0, 7080.0, false, now - 2 * DAY),
                3 => progress(0.0, 0.0, true, now - 6 * DAY),
                _ => progress(0.0, 0.0, false, 0),
            };
            json!({
                "id": format!("m{i}"),
                "type": "movie",
                "title": title,
                "year": year,
                "overview": "A made-up film for the screenshots.",
                "tagline": "",
                "rating": rating,
                "runtime": runtime,
                "genres": ["Drama"],
                "poster": art_file(Art::Poster, hue),
                "backdrop": art_file(Art::Wide, hue),
                "path": format!("D:\\Movies\\{title} ({year})\\{title} ({year}).mkv"),
                "addedAt": now - (3 + i as i64) * DAY,
                "progress": pr,
                "languages": Value::Null,
                "favorite": false,
                "hidden": false,
            })
        })
        .collect()
}

fn show(def: ShowDef, now: i64) -> Value {
    let (id, title, hue, seasons, watched_through, ago_days) = def;
    let base = now - (ago_days * DAY as f64) as i64;
    let mut episodes: Vec<Value> = Vec::new();
    let mut next_up = Value::Null;
    for s in 1..=seasons {
        for e in 1..=8u32 {
            let n = (s - 1) * 8 + e;
            let watched = n <= watched_through;
            let partial = n == watched_through + 1;
            let pr = if watched {
                progress(0.0, 0.0, true, base - (watched_through + 1 - n) as i64 * DAY)
            } else if partial {
                progress(1100.0, 2900.0, false, base)
            } else {
                progress(0.0, 0.0, false, 0)
            };
            let ep_id = format!("{id}-s{s}e{e}");
            if next_up.is_null() && !watched {
                next_up = json!(ep_id);
            }
            episodes.push(json!({
                "id": ep_id,
                "season": s,
                "episode": e,
                "episodeEnd": Value::Null,
                "title": EPISODE_TITLES[e as usize - 1],
                "overview": "A made-up episode for the screenshots, in which nothing in particular happens.",
                "airDate": format!("{}-0{}-14", 2019 + s, e),
                "runtime": 48,
                "thumb": art_file(Art::Shot, hue + e * 17),
                "path": format!("D:\\TV\\{title}\\Season {s}\\{title}.S{s:02}E{e:02}.mkv"),
                "addedAt": now - 5 * DAY,
                "progress": pr,
            }));
        }
    }
    let watched_count = episodes.iter().filter(|e| e["progress"]["watched"] == json!(true)).count();
    json!({
        "id": id,
        "type": "show",
        "title": title,
        "year": 2020,
        "overview": "A made-up series for the screenshots: a small town, a stranded ferry, and a lot of weather.",
        "rating": 8.3,
        "genres": ["Drama", "Mystery"],
        "status": "Returning",
        "poster": art_file(Art::Poster, hue),
        "backdrop": art_file(Art::Wide, hue),
        "addedAt": now - 5 * DAY,
        "episodes": episodes,
        "seasons": (1..=seasons).collect::<Vec<_>>(),
        "watchedCount": watched_count,
        "nextUp": next_up,
        "lastActivity": base,
        "languages": Value::Null,
        "favorite": false,
        "hidden": false,
    })
}

/// Same rule as `build_view_model`: part-watched films, plus shows with activity and something left.
fn continue_watching(movies: &[Value], shows: &[Value]) -> Vec<Value> {
    let mut list: Vec<(i64, Value)> = Vec::new();
    for m in movies {
        if m["progress"]["resumable"] == json!(true) {
            let at = m["progress"]["updatedAt"].as_i64().unwrap_or(0);
            list.push((at, json!({"kind": "movie", "id": m["id"], "at": at})));
        }
    }
    for s in shows {
        let at = s["lastActivity"].as_i64().unwrap_or(0);
        if at > 0 && !s["nextUp"].is_null() {
            list.push((at, json!({"kind": "episode", "showId": s["id"], "id": s["nextUp"], "at": at})));
        }
    }
    list.sort_by_key(|e| std::cmp::Reverse(e.0));
    list.into_iter().map(|(_, v)| v).collect()
}

// ---------------------------------------------------------------------------
// The rest of the state

fn settings(lang: &str) -> Value {
    let mut m = Map::new();
    for (k, v) in [
        ("libraries", json!([{"path": "D:\\Movies", "type": "movies"}, {"path": "D:\\TV", "type": "tv"}])),
        ("vlcPath", json!("")),
        ("vlcFullscreen", json!(true)),
        ("vlcExtraArgs", json!("")),
        ("autoplayNext", json!(true)),
        ("audioLanguage", json!("en")),
        ("subLanguage", json!("en")),
        ("tmdbKey", json!("0123456789abcdef0123456789abcdef")),
        ("sgdbKey", json!("")),
        ("steamEnabled", json!(true)),
        ("steamPath", json!("")),
        ("quietSteam", json!(true)),
        ("uiLanguage", json!(lang)),
        ("startFullscreen", json!(true)),
        ("launchAtLogin", json!(false)),
        ("uiScale", json!(1)),
        ("haptics", json!(false)),
        ("sounds", json!(false)),
        ("animations", json!("full")),
        ("freeWhilePlaying", json!(true)),
        ("autoCheckUpdates", json!(true)),
        ("skippedVersion", json!("")),
    ] {
        m.insert(k.into(), v);
    }
    Value::Object(m)
}

fn servers() -> Vec<Value> {
    vec![
        json!({
            "id": "srv-nas", "name": "Home NAS", "protocol": "sftp", "host": "nas.local", "port": 22,
            "username": "thomas", "authType": "key", "keyPath": "C:\\Users\\Thomas\\.ssh\\id_ed25519",
            "root": "/volume1/media", "insecureTls": false,
            "hostKey": "3f9a1c5e7b2d4086a1e3c9d75f0b8a24c6e1d9f3b7a5028c4e6d1f9a3b5c7e08",
            "hasSecret": false,
        }),
        json!({
            "id": "srv-seedbox", "name": "Seedbox", "protocol": "ftp", "host": "203.0.113.24", "port": 21,
            "username": "media", "authType": "password", "keyPath": "", "root": "/downloads",
            "insecureTls": false, "hostKey": "", "hasSecret": true,
        }),
    ]
}

struct Job<'a> {
    id: &'a str,
    server: &'a str,
    title: &'a str,
    kind: &'a str,
    status: &'a str,
    files: u32,
    index: u32,
    done: u64,
    total: u64,
    rate: u64,
    error: Value,
    folder: &'a str,
}

fn transfers() -> Vec<Value> {
    const GB: u64 = 1 << 30;
    let jobs = [
        Job { id: "t1", server: "srv-nas", title: "Night Freight (2023)", kind: "movie", status: "running", files: 1, index: 0, done: GB * 4 / 10, total: GB, rate: 38_500_000, error: Value::Null, folder: "D:\\Movies\\Night Freight (2023)" },
        Job { id: "t2", server: "srv-nas", title: "Harbor Lights S02", kind: "tv", status: "queued", files: 8, index: 0, done: 0, total: 9 * GB, rate: 0, error: Value::Null, folder: "D:\\TV\\Harbor Lights\\Season 2" },
        Job { id: "t3", server: "srv-seedbox", title: "Copper Sky (2022)", kind: "movie", status: "done", files: 1, index: 0, done: 3 * GB, total: 3 * GB, rate: 0, error: Value::Null, folder: "D:\\Movies\\Copper Sky (2022)" },
        Job { id: "t4", server: "srv-seedbox", title: "The Salt Road S03", kind: "tv", status: "error", files: 8, index: 2, done: 2 * GB, total: 7 * GB, rate: 0, error: json!("ETIMEDOUT"), folder: "D:\\TV\\The Salt Road\\Season 3" },
    ];
    jobs.into_iter()
        .map(|j| {
            json!({
                "id": j.id, "serverId": j.server, "title": j.title, "kind": j.kind, "status": j.status,
                "files": j.files, "fileIndex": j.index, "bytesDone": j.done, "bytesTotal": j.total,
                "rate": j.rate, "skipped": 0, "error": j.error, "folders": [j.folder],
            })
        })
        .collect()
}

fn apps(now: i64) -> Vec<Value> {
    APP_DEFS
        .iter()
        .enumerate()
        .map(|(i, &(name, kind, icon, hidden, days))| {
            json!({
                "id": format!("app{i}"),
                "name": name,
                "kind": kind,
                "icon": icon.map(|h| art_file(Art::Icon, h)),
                "hidden": hidden,
                "lastLaunched": if days == 0.0 { 0 } else { now - (days * DAY as f64) as i64 },
            })
        })
        .collect()
}

fn update(now: i64) -> Value {
    json!({
        "status": "available",
        "current": "3.0.0",
        "version": "3.1.0",
        "notes": "## What's new\n\n- **Faster start-up**: the library now appears before artwork finishes loading.\n- Controller: hold **Y** on a card for quick options.\n- Transfers: failed downloads can be retried from the queue.\n\n## Fixes\n\n- Resume no longer restarts from zero after a suspend.\n- Long titles wrap on the Now playing screen.\n",
        "progress": 0,
        "size": 88_080_384,
        "error": Value::Null,
        "checkedAt": now - 10 * 60_000,
    })
}

/// The full `state` payload, shaped like the real backend's `build_state`.
pub fn state(opts: &DemoOpts) -> Value {
    let now = now();
    let games = games(opts.resume, now);
    let movies = movies(now);
    let shows: Vec<Value> = SHOW_DEFS.iter().map(|&d| show(d, now)).collect();
    let cont = if opts.resume == Resume::Show { continue_watching(&movies, &shows) } else { Vec::new() };
    json!({
        "settings": settings(opts.lang),
        "lang": opts.lang,
        "library": {
            "movies": movies,
            "shows": shows,
            "games": games,
            "continueWatching": cont,
            "scannedAt": now - 20 * 60_000,
            "steamFound": true,
            "steamUser": "Thomas",
        },
        "scanning": false,
        "metaStatus": {"running": false, "error": Value::Null},
        "gameInfoStatus": {"running": false, "error": Value::Null},
        "nowPlaying": Value::Null,
        "game": Value::Null,
        "uiState": Value::Null,
        "toasts": [],
        "platform": "win32",
        "packaged": true,
        "fsePackage": false,
        "systemControls": true,
        "update": update(now),
        "hasBattery": true,
        "servers": servers(),
        "apps": apps(now),
        "appsScanning": false,
        "appsScannedAt": now - 3 * HOUR,
        "transfers": transfers(),
        "version": "3.0.0",
    })
}

/// The `now-playing` payload: Harbor Lights S01E04, 18 minutes into 48.
pub fn now_playing() -> Value {
    json!({
        "title": "Harbor Lights · S01E04",
        "request": {"kind": "episode", "showId": "sh0", "id": "sh0-s1e4"},
        "current": "D:\\TV\\Harbor Lights\\Season 1\\Harbor.Lights.S01E04.mkv",
        "time": 1100,
        "length": 2900,
        "paused": false,
        "queueSize": 21,
        "index": 3,
    })
}

/// What `get_stats` returns: several weeks of sessions plus names/art for what they mention.
pub fn stats() -> Value {
    let now = now();
    let mut sessions = Vec::new();
    // Deterministic pseudo-pattern over the last 40 days, oldest first.
    for d in (0..40i64).rev() {
        let day = now - d * DAY;
        let k = (d * 7 + 3) % 11;
        if k != 0 && k != 5 {
            let g = ((d * 5 + 1) % GAME_DEFS.len() as i64) as usize;
            sessions.push(json!({"kind": "game", "id": format!("g{g}"), "start": day - 2 * HOUR, "minutes": 25 + (d * 37) % 110}));
        }
        if d % 3 == 0 && k != 2 {
            let g = ((d + 2) % GAME_DEFS.len() as i64) as usize;
            sessions.push(json!({"kind": "game", "id": format!("g{g}"), "start": day - 6 * HOUR, "minutes": 15 + (d * 13) % 50}));
        }
        if k % 3 != 1 {
            let id = if d % 4 == 0 { format!("m{}", (d / 4) % MOVIE_DEFS.len() as i64) } else { format!("sh{}", d % SHOW_DEFS.len() as i64) };
            let minutes = if id.starts_with('m') { 95 + (d * 3) % 30 } else { 44 + (d * 5) % 50 };
            sessions.push(json!({"kind": "watch", "id": id, "start": day - HOUR, "minutes": minutes}));
        }
    }
    let mut items = Map::new();
    for (i, &(title, hue, _, minutes, _)) in GAME_DEFS.iter().enumerate() {
        items.insert(format!("g{i}"), json!({"type": "game", "title": title, "poster": art_file(Art::Poster, hue), "icon": Value::Null, "playtime": minutes}));
    }
    for (i, &(title, hue, ..)) in MOVIE_DEFS.iter().enumerate() {
        items.insert(format!("m{i}"), json!({"type": "movie", "title": title, "poster": art_file(Art::Poster, hue)}));
    }
    for &(id, title, hue, ..) in SHOW_DEFS.iter() {
        items.insert(id.into(), json!({"type": "show", "title": title, "poster": art_file(Art::Poster, hue)}));
    }
    json!({"sessions": sessions, "items": items, "now": now})
}

/// What `remote_list` returns: `[{name, isDir, size}]`, folders first then by name.
pub fn remote_listing(path: &str) -> Value {
    const MB: u64 = 1 << 20;
    let dir = |n: &str| json!({"name": n, "isDir": true, "size": 0});
    let file = |n: &str, size: u64| json!({"name": n, "isDir": false, "size": size});
    let trimmed = path.trim_end_matches('/');
    match trimmed {
        "" => json!([dir("Downloads"), dir("Movies"), dir("Music"), dir("TV"), file("notes.txt", 2048)]),
        p if p.ends_with("/Movies") => json!([
            dir("Extras"),
            file("Night Freight (2023).mkv", 4_300 * MB),
            file("Paper Lanterns (2019).mkv", 2_100 * MB),
            file("Small Hours (2018).mp4", 1_450 * MB),
            file("The Long Tide (2021).mkv", 3_800 * MB),
        ]),
        p if p.ends_with("/TV") => json!([dir("Harbor Lights"), dir("Quiet Signals"), dir("The Salt Road")]),
        p if p.ends_with("Harbor Lights") => json!([dir("Season 1"), dir("Season 2"), dir("Season 3")]),
        _ => json!([
            dir("Subs"),
            file("Harbor.Lights.S02E01.1080p.mkv", 1_380 * MB),
            file("Harbor.Lights.S02E02.1080p.mkv", 1_410 * MB),
            file("Harbor.Lights.S02E03.1080p.mkv", 1_295 * MB),
            file("Harbor.Lights.S02E04.1080p.mkv", 1_502 * MB),
            file("poster.jpg", 412_000),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(lang: &'static str, resume: Resume) -> DemoOpts {
        DemoOpts { lang, resume }
    }

    fn collect_paths(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) if s.starts_with(artwork_dir().to_string_lossy().as_ref()) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|x| collect_paths(x, out)),
            Value::Object(m) => m.values().for_each(|x| collect_paths(x, out)),
            _ => {}
        }
    }

    #[test]
    fn state_has_every_top_level_key() {
        let s = state(&opts("en", Resume::Game));
        for k in [
            "settings", "lang", "library", "scanning", "metaStatus", "gameInfoStatus", "nowPlaying", "game", "uiState", "toasts", "platform", "packaged",
            "fsePackage", "systemControls", "update", "hasBattery", "servers", "apps", "appsScanning", "appsScannedAt", "transfers", "version",
        ] {
            assert!(s.get(k).is_some(), "missing {k}");
        }
        for k in ["movies", "shows", "games", "continueWatching", "scannedAt", "steamFound", "steamUser"] {
            assert!(s["library"].get(k).is_some(), "missing library.{k}");
        }
        assert_eq!(s["library"]["games"].as_array().unwrap().len(), 6);
        assert_eq!(s["library"]["shows"][0]["episodes"].as_array().unwrap().len(), 24);
        assert_eq!(s["transfers"].as_array().unwrap().len(), 4);
        assert_eq!(s["servers"].as_array().unwrap().len(), 2);
        assert_eq!(s["apps"].as_array().unwrap().len(), 6);
        assert!(s["library"]["continueWatching"].as_array().unwrap().is_empty());
        let show = state(&opts("en", Resume::Show));
        assert_eq!(show["library"]["continueWatching"][0]["id"], "sh0-s2e4");
        assert_eq!(show["library"]["games"][0]["lastPlayed"], 0);
    }

    #[test]
    fn french_sets_lang() {
        let s = state(&opts("fr", Resume::Show));
        assert_eq!(s["lang"], "fr");
        assert_eq!(s["settings"]["uiLanguage"], "fr");
    }

    #[test]
    fn artwork_exists_and_is_fast() {
        let t = std::time::Instant::now();
        let dir = ensure_artwork();
        eprintln!("artwork ready in {:?}", t.elapsed());
        assert_eq!(dir, artwork_dir());
        let mut paths = Vec::new();
        for v in [state(&opts("en", Resume::Show)), stats()] {
            collect_paths(&v, &mut paths);
        }
        assert!(paths.len() > 100);
        for p in paths {
            assert!(PathBuf::from(&p).is_file(), "missing {p}");
        }
        ensure_artwork();
    }

    #[test]
    fn stats_and_remote_shapes() {
        let s = stats();
        let sessions = s["sessions"].as_array().unwrap();
        assert!(sessions.len() > 40);
        assert!(sessions.iter().any(|x| x["kind"] == "game") && sessions.iter().any(|x| x["kind"] == "watch"));
        for x in sessions {
            assert!(s["items"].get(x["id"].as_str().unwrap()).is_some());
        }
        assert!(remote_listing("/")[0]["isDir"].as_bool().unwrap());
        assert!(remote_listing("/Movies").as_array().unwrap().iter().any(|e| e["size"].as_u64().unwrap() > 0));
        assert_eq!(now_playing()["length"], 2900);
    }
}
