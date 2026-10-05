//! Scripted runs for development and screenshots: `lounge --demo --script "go games; key Right; shot a.png"`.
//! Steps, separated by `;`:
//!
//! - `go NAME [ID]` — switch to a tab / tool page, or push any other page (e.g. `go game g1`)
//! - `key NAME` — a controller key: Up Down Left Right Return Escape Menu Tab Backtab PageUp PageDown F1 F2 F3
//! - `type TEXT` — type text as a physical keyboard would
//! - `mode pad|keyboard|mouse|touch` — the input mode (hint glyphs etc.)
//! - `event NAME JSON` — deliver a backend event, e.g. `event now-playing {"title":"…"}`, or `event now-playing demo`
//! - `wait MS` — pause
//! - `shot PATH` — save a PNG of the window (waits for artwork decodes and fades first)
//! - `intro` — play the startup intro again (then `snap` its frames)
//! - `snap PATH` — save the window right away as raw RGBA, `PATH.WxH.rgba` (frames of an animation)
//! - `quit`
//!
//! Steps run ~150 ms apart so transitions settle. The run quits after the last step.

use super::ctx::cx;
use super::{input, router};
use crate::Input;
use serde_json::Value;
use slint::platform::{Key, WindowEvent};
use slint::ComponentHandle;
use std::rc::Rc;
use std::time::Duration;

const GAP: Duration = Duration::from_millis(150);

pub fn run(script: &str) {
    let steps: Rc<Vec<String>> = Rc::new(script.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
    // Let the first frame render.
    slint::Timer::single_shot(Duration::from_millis(600), move || step(steps, 0));
}

fn step(steps: Rc<Vec<String>>, i: usize) {
    let Some(line) = steps.get(i) else {
        let _ = slint::quit_event_loop();
        return;
    };
    let line = line.clone();
    let (cmd, rest) = line.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((line.as_str(), ""));
    let next = move |after: Duration| {
        let steps = steps.clone();
        slint::Timer::single_shot(after, move || step(steps, i + 1));
    };
    match cmd {
        "go" => {
            let (name, id) = rest.split_once(' ').unwrap_or((rest, ""));
            if router::TABS.contains(&name) || router::TOOLS.contains(&name) {
                router::switch_tab(name);
            } else {
                router::go(name, id);
            }
            next(GAP * 3);
        }
        "key" => {
            match key_named(rest) {
                Some(k) => input::synthesize(k),
                None => eprintln!("[script] unknown key {rest}"),
            }
            next(GAP);
        }
        "type" => {
            let ui = cx().ui();
            for c in rest.chars() {
                let text: slint::SharedString = c.to_string().into();
                ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                ui.window().dispatch_event(WindowEvent::KeyReleased { text });
            }
            next(GAP);
        }
        "mode" => {
            cx().ui().global::<Input>().set_mode(rest.into());
            next(GAP);
        }
        "event" => {
            let (name, json) = rest.split_once(' ').unwrap_or((rest, "null"));
            let payload = if json.trim() == "demo" { demo_payload(name) } else { serde_json::from_str(json).unwrap_or(Value::Null) };
            cx().dispatch(name, payload);
            next(GAP * 2);
        }
        "wait" => next(Duration::from_millis(rest.parse().unwrap_or(500))),
        "intro" => {
            crate::ui::overlays::intro::replay();
            next(Duration::ZERO);
        }
        // Right now, without waiting for artwork or fades, as raw RGBA (PNG encoding is too slow
        // in a debug build to catch an animation): PATH.WxH.rgba.
        "snap" => {
            if let Ok(buf) = cx().ui().window().take_snapshot() {
                let _ = std::fs::write(format!("{rest}.{}x{}.rgba", buf.width(), buf.height()), buf.as_bytes());
            }
            next(Duration::ZERO);
        }
        "shot" => {
            let path = rest.to_string();
            wait_for_art(0, move || {
                // Fades and focus springs.
                slint::Timer::single_shot(Duration::from_millis(700), move || {
                    shot(&path);
                    next(GAP);
                });
            });
        }
        "quit" => {
            let _ = slint::quit_event_loop();
        }
        other => {
            eprintln!("[script] unknown step {other}");
            next(GAP);
        }
    }
}

fn demo_payload(event: &str) -> Value {
    match event {
        "now-playing" => crate::demo::now_playing(),
        _ => Value::Null,
    }
}

fn wait_for_art(tries: u32, then: impl FnOnce() + 'static) {
    if tries > 60 || !super::art::busy() {
        then();
        return;
    }
    slint::Timer::single_shot(Duration::from_millis(50), move || wait_for_art(tries + 1, then));
}

fn shot(path: &str) {
    let ui = cx().ui();
    match ui.window().take_snapshot() {
        Ok(buf) => {
            if let Err(e) = image::save_buffer(path, buf.as_bytes(), buf.width(), buf.height(), image::ExtendedColorType::Rgba8) {
                eprintln!("[script] couldn't save {path}: {e}");
            } else {
                eprintln!("[script] wrote {path}");
            }
        }
        Err(e) => eprintln!("[script] snapshot failed: {e}"),
    }
}

pub fn key_named(name: &str) -> Option<Key> {
    Some(match name {
        "Up" => Key::UpArrow,
        "Down" => Key::DownArrow,
        "Left" => Key::LeftArrow,
        "Right" => Key::RightArrow,
        "Return" | "A" => Key::Return,
        "Escape" | "B" => Key::Escape,
        "Menu" | "X" => Key::Menu,
        "F3" | "Y" => Key::F3,
        "Tab" | "RB" => Key::Tab,
        "Backtab" | "LB" => Key::Backtab,
        "PageUp" | "LT" => Key::PageUp,
        "PageDown" | "RT" => Key::PageDown,
        "F1" | "Start" => Key::F1,
        "F2" | "View" => Key::F2,
        "Backspace" => Key::Backspace,
        _ => return None,
    })
}
