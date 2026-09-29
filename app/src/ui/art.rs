//! Local artwork for Slint: decoded and downscaled to the size it's drawn at on worker threads, kept in
//! an LRU cache bounded by decoded bytes. (A 1080p Steam hero drawn on a 300 px card would otherwise
//! cost ~8 MB; downscaled it's a few hundred KB — the same reason the Tauri shell had `thumbs.rs`.)
//!
//! Slint asks through `Art.load(path, width, rev)`: a hit returns the image; a miss queues a decode
//! and returns an empty image. When decodes land, `Art.rev` is bumped (once per batch), which makes
//! every `Art.get` binding re-evaluate and pick its image up.

use crate::{Art, AppWindow};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

/// Decoded-bytes budget. Enough for a few hundred cards plus backdrops at handheld resolution.
const BUDGET: usize = 320 * 1024 * 1024;
/// Widths images are decoded at (physical px): the smallest bucket at least as wide as requested.
const BUCKETS: [u32; 10] = [128, 256, 384, 512, 768, 1024, 1536, 2048, 2560, 3840];
const WORKERS: usize = 2;

type Key = (String, u32);

enum Slot {
    Ready { img: Image, bytes: usize, used: u64 },
    Failed,
}

struct Cache {
    map: HashMap<Key, Slot>,
    pending: HashSet<Key>,
    bytes: usize,
    tick: u64,
    rev_scheduled: bool,
    scale: f32,
    ui: Option<slint::Weak<AppWindow>>,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache {
        map: HashMap::new(),
        pending: HashSet::new(),
        bytes: 0,
        tick: 0,
        rev_scheduled: false,
        scale: 1.0,
        ui: None,
    });
}

/// The job stack, newest first: what was asked for last is what's on screen now.
struct Queue {
    jobs: Mutex<Vec<Key>>,
    ready: Condvar,
}

fn queue() -> &'static Arc<Queue> {
    static Q: std::sync::OnceLock<Arc<Queue>> = std::sync::OnceLock::new();
    Q.get_or_init(|| {
        let q = Arc::new(Queue { jobs: Mutex::new(Vec::new()), ready: Condvar::new() });
        for _ in 0..WORKERS {
            let q = q.clone();
            std::thread::spawn(move || worker(q));
        }
        q
    })
}

pub fn install(ui: &AppWindow) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.ui = Some(ui.as_weak());
        c.scale = ui.window().scale_factor();
    });
    let art = ui.global::<Art>();
    art.on_load(|path, width, _rev| get(&path, width));
    art.on_loaded(|path, width, _rev| is_loaded(&path, width));
}

fn bucket(width_logical: f32, scale: f32) -> u32 {
    let px = if width_logical > 1.0 { (width_logical * scale).ceil() as u32 } else { 512 };
    *BUCKETS.iter().find(|b| **b >= px).unwrap_or(&BUCKETS[BUCKETS.len() - 1])
}

fn key_for(path: &str, width: f32) -> Key {
    let scale = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        // The window may have moved to another display since install.
        if let Some(ui) = c.ui.as_ref().and_then(|w| w.upgrade()) {
            c.scale = ui.window().scale_factor();
        }
        c.scale
    });
    (path.to_string(), bucket(width, scale))
}

pub fn get(path: &str, width: f32) -> Image {
    if path.is_empty() {
        return Image::default();
    }
    let key = key_for(path, width);
    let hit = CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.tick += 1;
        let tick = c.tick;
        match c.map.get_mut(&key) {
            Some(Slot::Ready { img, used, .. }) => {
                *used = tick;
                Some(img.clone())
            }
            Some(Slot::Failed) => Some(Image::default()),
            None => None,
        }
    });
    if let Some(img) = hit {
        return img;
    }
    // A bigger decode of the same file is as good (and avoids a second decode while it's cached).
    let bigger = CACHE.with(|c| {
        let c = c.borrow();
        BUCKETS.iter().filter(|b| **b > key.1).find_map(|b| match c.map.get(&(key.0.clone(), *b)) {
            Some(Slot::Ready { img, .. }) => Some(img.clone()),
            _ => None,
        })
    });
    request(key);
    bigger.unwrap_or_default()
}

pub fn is_loaded(path: &str, width: f32) -> bool {
    if path.is_empty() {
        return false;
    }
    let key = key_for(path, width);
    CACHE.with(|c| {
        let c = c.borrow();
        matches!(c.map.get(&key), Some(Slot::Ready { .. }) | Some(Slot::Failed))
            || BUCKETS.iter().filter(|b| **b > key.1).any(|b| matches!(c.map.get(&(key.0.clone(), *b)), Some(Slot::Ready { .. })))
    })
}

fn request(key: Key) {
    let fresh = CACHE.with(|c| c.borrow_mut().pending.insert(key.clone()));
    let q = queue();
    let mut jobs = q.jobs.lock().unwrap();
    if !fresh {
        // Already queued: move it to the top, it's wanted again.
        if let Some(i) = jobs.iter().position(|k| *k == key) {
            let k = jobs.remove(i);
            jobs.push(k);
        }
        return;
    }
    jobs.push(key);
    q.ready.notify_one();
}

/// Drop every decoded image (the UI is being unloaded while a game runs).
pub fn clear() {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.map.clear();
        c.bytes = 0;
    });
    queue().jobs.lock().unwrap().clear();
    CACHE.with(|c| c.borrow_mut().pending.clear());
}

fn worker(q: Arc<Queue>) {
    loop {
        let key = {
            let mut jobs = q.jobs.lock().unwrap();
            while jobs.is_empty() {
                jobs = q.ready.wait(jobs).unwrap();
            }
            jobs.pop().unwrap()
        };
        let decoded = decode(&key.0, key.1);
        let _ = slint::invoke_from_event_loop(move || deliver(key, decoded));
    }
}

fn decode(path: &str, width: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > width {
        let h = ((img.height() as u64 * width as u64) / img.width().max(1) as u64).max(1) as u32;
        img.thumbnail(width, h)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

fn deliver(key: Key, decoded: Option<(u32, u32, Vec<u8>)>) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if !c.pending.remove(&key) {
            return; // cleared meanwhile
        }
        c.tick += 1;
        let used = c.tick;
        match decoded {
            Some((w, h, bytes)) => {
                let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&bytes, w, h);
                let size = bytes.len();
                c.bytes += size;
                c.map.insert(key, Slot::Ready { img: Image::from_rgba8(buf), bytes: size, used });
                evict(&mut c);
            }
            None => {
                c.map.insert(key, Slot::Failed);
            }
        }
        if !c.rev_scheduled {
            c.rev_scheduled = true;
            let ui = c.ui.clone();
            // Coalesce a burst of decodes into one re-evaluation.
            slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
                CACHE.with(|c| c.borrow_mut().rev_scheduled = false);
                if let Some(ui) = ui.and_then(|w| w.upgrade()) {
                    let art = ui.global::<Art>();
                    art.set_rev(art.get_rev().wrapping_add(1));
                }
            });
        }
    });
}

fn evict(c: &mut Cache) {
    while c.bytes > BUDGET {
        let oldest = c
            .map
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready { used, .. } => Some((*used, k.clone())),
                Slot::Failed => None,
            })
            .min_by_key(|(u, _)| *u);
        let Some((_, k)) = oldest else { break };
        if let Some(Slot::Ready { bytes, .. }) = c.map.remove(&k) {
            c.bytes -= bytes;
        }
    }
}

/// Decodes still queued or in flight (the snapshot script waits for these).
pub fn busy() -> bool {
    CACHE.with(|c| !c.borrow().pending.is_empty())
}
