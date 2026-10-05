//! Input: the controller (Windows, via gilrs), the input mode that drives hint glyphs, and navigation
//! feedback (sounds / rumble).
//!
//! The controller doesn't get its own navigation code path: its buttons become the same key events a
//! keyboard sends (`Input.synthetic` is set around them so they keep pad mode), so every screen handles
//! one set of keys:
//!
//! | pad            | key            | meaning                         |
//! |----------------|----------------|---------------------------------|
//! | D-pad / stick  | arrows         | move                            |
//! | A              | Return         | activate                        |
//! | B              | Escape         | back                            |
//! | X              | Menu           | options                         |
//! | Y              | F3             | search                          |
//! | LB / RB        | Backtab / Tab  | previous / next tab             |
//! | LT / RT        | PageUp / Down  | a page up / down                |
//! | ☰ (Start)      | F1             | quick menu                      |
//! | ⧉ (View)       | F2             | view                            |
//!
//! Screens that want raw pad state (the controller tester) register with [`set_capture`]; while one is
//! registered, pads stop navigating.

use super::ctx::{cx, Ctx};
use crate::Input;
use slint::platform::{Key, WindowEvent};
use slint::ComponentHandle;
use std::cell::RefCell;

pub const REPEAT_DELAY_MS: u64 = 360;
pub const REPEAT_RATE_MS: u64 = 95;

/// A controller snapshot, for [`set_capture`] (the pad tester).
#[derive(Debug, Clone, Default)]
pub struct PadSnapshot {
    pub name: String,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
    /// Standard-mapping order: A B X Y LB RB LT RT View Menu LS RS Up Down Left Right Guide.
    pub buttons: [f32; 17],
    /// Left X/Y, right X/Y.
    pub axes: [f32; 4],
    pub connected_count: usize,
}

type Capture = Box<dyn Fn(&PadSnapshot)>;
type Feedback = Box<dyn Fn(&str)>;

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static FEEDBACK: RefCell<Vec<Feedback>> = const { RefCell::new(Vec::new()) };
}

pub fn install(ctx: &Ctx) {
    let ui = ctx.ui();
    let input = ui.global::<Input>();
    input.on_feedback(|kind| feedback(&kind));
    #[cfg(windows)]
    pad::start(ctx.ui.clone());
}

pub fn mode() -> String {
    cx().ui().global::<Input>().get_mode().to_string()
}

/// Hand every pad snapshot to `f` instead of navigating (None gives the controller back).
pub fn set_capture(f: Option<Capture>) {
    CAPTURE.with(|c| *c.borrow_mut() = f);
}

fn capturing() -> bool {
    CAPTURE.with(|c| c.borrow().is_some())
}

/// Sounds and rumble subscribe here (see feedback.rs); `kind` is move / select / back / edge / open.
pub fn on_feedback(f: impl Fn(&str) + 'static) {
    FEEDBACK.with(|l| l.borrow_mut().push(Box::new(f)));
}

pub fn feedback(kind: &str) {
    FEEDBACK.with(|l| {
        for f in l.borrow().iter() {
            f(kind);
        }
    });
}

/// Send a key press + release to the window as if typed, marked as coming from the controller.
pub fn synthesize(key: Key) {
    let ui = cx().ui();
    let input = ui.global::<Input>();
    input.set_mode("pad".into());
    input.set_synthetic(true);
    let text: slint::SharedString = key.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text });
    input.set_synthetic(false);
}

#[cfg(windows)]
mod pad {
    //! gilrs polling on a thread; button edges (with auto-repeat for directions and shoulders) are sent
    //! to the event loop as synthetic keys, or as snapshots to a capture.
    use super::*;
    use gilrs::{Axis, Button, Gilrs};
    use std::time::{Duration, Instant};

    const THRESHOLD: f32 = 0.55;

    pub fn start(ui: slint::Weak<crate::AppWindow>) {
        std::thread::spawn(move || {
            // Keep trying rather than giving up on controllers for the whole session.
            let mut gilrs = loop {
                match Gilrs::new() {
                    Ok(g) => break g,
                    Err(e) => {
                        eprintln!("[input] gamepads unavailable, retrying: {e}");
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
            };
            let mut held: [Option<Instant>; 14] = [None; 14];
            let mut last_connected = usize::MAX;
            loop {
                while gilrs.next_event().is_some() {}
                let pads: Vec<_> = gilrs.gamepads().filter(|(_, g)| g.is_connected()).collect();
                if pads.len() != last_connected {
                    last_connected = pads.len();
                    let name = pads.first().map(|(_, g)| g.name().to_string()).unwrap_or_default();
                    let n = pads.len();
                    let ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            let input = ui.global::<Input>();
                            input.set_pad_connected(n > 0);
                            input.set_pad_name(name.into());
                        }
                    });
                }
                // The pad with something pressed (handhelds can expose both built-in and Bluetooth pads).
                let active = pads.iter().find(|(_, g)| {
                    g.state().buttons().any(|(_, b)| b.is_pressed()) || [Axis::LeftStickX, Axis::LeftStickY].iter().any(|a| g.value(*a).abs() > THRESHOLD)
                });
                let snapshot = pads.first().map(|(_, g)| snapshot(g, pads.len()));
                let ui2 = ui.clone();
                let pressed: [bool; 14] = match active {
                    Some((_, g)) => {
                        let b = |x: Button| g.is_pressed(x);
                        let (ax, ay) = (g.value(Axis::LeftStickX), g.value(Axis::LeftStickY));
                        [
                            b(Button::DPadUp) || ay > THRESHOLD,
                            b(Button::DPadDown) || ay < -THRESHOLD,
                            b(Button::DPadLeft) || ax < -THRESHOLD,
                            b(Button::DPadRight) || ax > THRESHOLD,
                            b(Button::South),
                            b(Button::East),
                            b(Button::West),
                            b(Button::North),
                            b(Button::LeftTrigger),
                            b(Button::RightTrigger),
                            b(Button::LeftTrigger2),
                            b(Button::RightTrigger2),
                            b(Button::Start),
                            b(Button::Select),
                        ]
                    }
                    None => [false; 14],
                };
                const KEYS: [Key; 14] = [
                    Key::UpArrow, Key::DownArrow, Key::LeftArrow, Key::RightArrow, Key::Return, Key::Escape, Key::Menu, Key::F3,
                    Key::Backtab, Key::Tab, Key::PageUp, Key::PageDown, Key::F1, Key::F2,
                ];
                // Directions, shoulders and triggers repeat while held.
                const REPEATS: [bool; 14] = [true, true, true, true, false, false, false, false, true, true, true, true, false, false];
                let now = Instant::now();
                let mut fire = Vec::new();
                for i in 0..14 {
                    match (pressed[i], held[i]) {
                        (true, None) => {
                            held[i] = Some(now + Duration::from_millis(REPEAT_DELAY_MS));
                            fire.push(KEYS[i]);
                        }
                        (true, Some(next)) if REPEATS[i] && now >= next => {
                            held[i] = Some(now + Duration::from_millis(REPEAT_RATE_MS));
                            fire.push(KEYS[i]);
                        }
                        (false, _) => held[i] = None,
                        _ => {}
                    }
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if ui2.upgrade().is_none() {
                        return;
                    }
                    if capturing() {
                        if let Some(s) = snapshot {
                            CAPTURE.with(|c| {
                                if let Some(f) = c.borrow().as_ref() {
                                    f(&s);
                                }
                            });
                        }
                        return;
                    }
                    for k in fire {
                        synthesize(k);
                    }
                });
                std::thread::sleep(Duration::from_millis(8));
            }
        });
    }

    fn snapshot(g: &gilrs::Gamepad, count: usize) -> PadSnapshot {
        let v = |b: Button| g.button_data(b).map(|d| d.value()).unwrap_or(if g.is_pressed(b) { 1.0 } else { 0.0 });
        PadSnapshot {
            name: g.name().to_string(),
            vendor: g.vendor_id(),
            product: g.product_id(),
            buttons: [
                v(Button::South), v(Button::East), v(Button::West), v(Button::North),
                v(Button::LeftTrigger), v(Button::RightTrigger), v(Button::LeftTrigger2), v(Button::RightTrigger2),
                v(Button::Select), v(Button::Start), v(Button::LeftThumb), v(Button::RightThumb),
                v(Button::DPadUp), v(Button::DPadDown), v(Button::DPadLeft), v(Button::DPadRight), v(Button::Mode),
            ],
            axes: [g.value(Axis::LeftStickX), -g.value(Axis::LeftStickY), g.value(Axis::RightStickX), -g.value(Axis::RightStickY)],
            connected_count: count,
        }
    }
}

/// Rumble the controllers: `strong` / `weak` motor magnitudes 0..1 for `ms` milliseconds (the pad
/// tester's vibration buttons and L3 + R3). No-op without force feedback, and off Windows.
pub fn rumble(strong: f32, weak: f32, ms: u32) {
    #[cfg(windows)]
    rumbler::play(strong, weak, ms);
    #[cfg(not(windows))]
    let _ = (strong, weak, ms);
}

#[cfg(windows)]
mod rumbler {
    //! Force feedback on its own gilrs instance and thread (the polling thread above is left alone);
    //! requests arrive over a channel.
    use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};
    use gilrs::Gilrs;
    use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    static TX: OnceLock<Mutex<Sender<(f32, f32, u32)>>> = OnceLock::new();

    pub fn play(strong: f32, weak: f32, ms: u32) {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = channel::<(f32, f32, u32)>();
            std::thread::spawn(move || {
                let Ok(mut gilrs) = Gilrs::new() else { return };
                let mut playing = Vec::new();
                loop {
                    while gilrs.next_event().is_some() {}
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok((strong, weak, ms)) => {
                            let pads: Vec<_> = gilrs.gamepads().filter(|(_, g)| g.is_connected() && g.is_ff_supported()).map(|(id, _)| id).collect();
                            if pads.is_empty() {
                                continue;
                            }
                            let replay = Replay { play_for: Ticks::from_ms(ms), ..Default::default() };
                            let mag = |v: f32| (v.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
                            let effect = EffectBuilder::new()
                                .add_effect(BaseEffect { kind: BaseEffectType::Strong { magnitude: mag(strong) }, scheduling: replay, ..Default::default() })
                                .add_effect(BaseEffect { kind: BaseEffectType::Weak { magnitude: mag(weak) }, scheduling: replay, ..Default::default() })
                                .repeat(Repeat::For(Ticks::from_ms(ms)))
                                .gamepads(&pads)
                                .finish(&mut gilrs);
                            if let Ok(effect) = effect {
                                let _ = effect.play();
                                // Dropping an effect stops it: keep it until it has played.
                                playing.push((effect, Instant::now() + Duration::from_millis(ms as u64 + 100)));
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    let now = Instant::now();
                    playing.retain(|(_, until)| *until > now);
                }
            });
            Mutex::new(tx)
        });
        if let Ok(tx) = tx.lock() {
            let _ = tx.send((strong, weak, ms));
        }
    }
}
