//! Input: the controller (Windows, XInput), the input mode that drives hint glyphs, and navigation
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
    #[cfg(windows)]
    pad::CAPTURING.store(f.is_some(), std::sync::atomic::Ordering::Relaxed);
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

/// While a game runs Lounge stops reading the controller altogether (see `WindowOp::SuspendUi`).
pub fn set_paused(paused: bool) {
    #[cfg(windows)]
    pad::PAUSED.store(paused, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(windows))]
    let _ = paused;
}

/// Rumble the controllers: `strong` / `weak` motor magnitudes 0..1 for `ms` milliseconds (navigation
/// feedback, the pad tester's vibration buttons and L3 + R3). No-op without a controller, and off Windows.
pub fn rumble(strong: f32, weak: f32, ms: u32) {
    #[cfg(windows)]
    pad::rumble(strong, weak, ms);
    #[cfg(not(windows))]
    let _ = (strong, weak, ms);
}

#[cfg(windows)]
mod pad {
    //! XInput, read directly on one thread that also drives rumble. Button edges (with auto-repeat for
    //! directions and shoulders) are sent to the event loop as synthetic keys, or as snapshots to a
    //! capture.
    //!
    //! Kept cheap on battery: the event loop only hears about it when there is something to deliver;
    //! it polls every 10 ms while a pad is in use and every 40 ms once it has been idle for 2 s; empty
    //! slots are probed once a second (XInputGetState on an empty slot is slow); and it stops while a
    //! game runs. (gilrs, used before, polled all four slots every 10 ms on a thread per instance that
    //! never stopped, and Lounge had three instances.)
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::Input::XboxController::*;

    const THRESHOLD: f32 = 0.55;
    const ACTIVE: Duration = Duration::from_millis(10);
    const IDLE: Duration = Duration::from_millis(40);
    const IDLE_AFTER: Duration = Duration::from_secs(2);
    const PROBE: Duration = Duration::from_secs(1);
    const PAUSED_POLL: Duration = Duration::from_millis(500);
    /// XInput reports no names; this is what gilrs called them.
    const NAME: &str = "Xbox Controller";

    pub static CAPTURING: AtomicBool = AtomicBool::new(false);
    pub static PAUSED: AtomicBool = AtomicBool::new(false);
    static RUMBLE: OnceLock<Mutex<Sender<(f32, f32, u32)>>> = OnceLock::new();

    pub fn rumble(strong: f32, weak: f32, ms: u32) {
        if let Some(Ok(tx)) = RUMBLE.get().map(|t| t.lock()) {
            let _ = tx.send((strong, weak, ms));
        }
    }

    fn read(slot: u32) -> Option<XINPUT_GAMEPAD> {
        let mut s: XINPUT_STATE = unsafe { std::mem::zeroed() };
        (unsafe { XInputGetState(slot, &mut s) } == 0).then_some(s.Gamepad)
    }

    fn vibrate(connected: &[bool; 4], strong: f32, weak: f32) {
        let m = |v: f32| (v.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
        let v = XINPUT_VIBRATION { wLeftMotorSpeed: m(strong), wRightMotorSpeed: m(weak) };
        for slot in (0..4).filter(|s| connected[*s as usize]) {
            unsafe { XInputSetState(slot, &v) };
        }
    }

    fn axis(v: i16) -> f32 {
        (v as f32 / 32767.0).clamp(-1.0, 1.0)
    }

    fn trigger(v: u8) -> f32 {
        v as f32 / 255.0
    }

    fn busy(g: &XINPUT_GAMEPAD) -> bool {
        g.wButtons != 0
            || trigger(g.bLeftTrigger) > 0.5
            || trigger(g.bRightTrigger) > 0.5
            || axis(g.sThumbLX).abs() > THRESHOLD
            || axis(g.sThumbLY).abs() > THRESHOLD
    }

    pub fn start(ui: slint::Weak<crate::AppWindow>) {
        let (tx, rx) = channel();
        let _ = RUMBLE.set(Mutex::new(tx));
        let _ = std::thread::Builder::new().name("lounge-pad".into()).spawn(move || run(ui, rx));
    }

    fn run(ui: slint::Weak<crate::AppWindow>, rx: Receiver<(f32, f32, u32)>) {
        let mut connected = [false; 4];
        let mut last_probe: Option<Instant> = None;
        let mut held: [Option<Instant>; 14] = [None; 14];
        let mut last_count = usize::MAX;
        let mut last_input: Option<Instant> = None;
        let mut rumble_until: Option<Instant> = None;
        let mut wait = Duration::ZERO;
        loop {
            // The rumble channel doubles as the poll timer, so a rumble request is handled at once.
            match rx.recv_timeout(wait) {
                Ok((strong, weak, ms)) if !PAUSED.load(Ordering::Relaxed) => {
                    vibrate(&connected, strong, weak);
                    rumble_until = Some(Instant::now() + Duration::from_millis(ms as u64));
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if rumble_until.is_some_and(|t| now >= t) || (rumble_until.is_some() && PAUSED.load(Ordering::Relaxed)) {
                vibrate(&connected, 0.0, 0.0);
                rumble_until = None;
            }
            if PAUSED.load(Ordering::Relaxed) {
                held = [None; 14];
                wait = PAUSED_POLL;
                continue;
            }

            let probe = last_probe.is_none_or(|t| now - t >= PROBE);
            if probe {
                last_probe = Some(now);
            }
            let mut pads = Vec::new();
            for (slot, on) in connected.iter_mut().enumerate() {
                if *on || probe {
                    let g = read(slot as u32);
                    *on = g.is_some();
                    pads.extend(g);
                }
            }

            if pads.len() != last_count {
                last_count = pads.len();
                let n = pads.len();
                let ui = ui.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui.upgrade() {
                        let input = ui.global::<Input>();
                        input.set_pad_connected(n > 0);
                        input.set_pad_name(if n > 0 { NAME.into() } else { Default::default() });
                    }
                });
            }

            // The pad with something pressed (handhelds can expose both built-in and Bluetooth pads).
            let active = pads.iter().find(|g| busy(g));
            let pressed: [bool; 14] = match active {
                Some(g) => {
                    let b = |x: u16| g.wButtons & x != 0;
                    let (ax, ay) = (axis(g.sThumbLX), axis(g.sThumbLY));
                    [
                        b(XINPUT_GAMEPAD_DPAD_UP) || ay > THRESHOLD,
                        b(XINPUT_GAMEPAD_DPAD_DOWN) || ay < -THRESHOLD,
                        b(XINPUT_GAMEPAD_DPAD_LEFT) || ax < -THRESHOLD,
                        b(XINPUT_GAMEPAD_DPAD_RIGHT) || ax > THRESHOLD,
                        b(XINPUT_GAMEPAD_A),
                        b(XINPUT_GAMEPAD_B),
                        b(XINPUT_GAMEPAD_X),
                        b(XINPUT_GAMEPAD_Y),
                        b(XINPUT_GAMEPAD_LEFT_SHOULDER),
                        b(XINPUT_GAMEPAD_RIGHT_SHOULDER),
                        trigger(g.bLeftTrigger) > 0.5,
                        trigger(g.bRightTrigger) > 0.5,
                        b(XINPUT_GAMEPAD_START),
                        b(XINPUT_GAMEPAD_BACK),
                    ]
                }
                None => [false; 14],
            };
            if active.is_some() {
                last_input = Some(now);
            }
            const KEYS: [Key; 14] = [
                Key::UpArrow, Key::DownArrow, Key::LeftArrow, Key::RightArrow, Key::Return, Key::Escape, Key::Menu, Key::F3,
                Key::Backtab, Key::Tab, Key::PageUp, Key::PageDown, Key::F1, Key::F2,
            ];
            // Directions, shoulders and triggers repeat while held.
            const REPEATS: [bool; 14] = [true, true, true, true, false, false, false, false, true, true, true, true, false, false];
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

            let capturing = CAPTURING.load(Ordering::Relaxed);
            let snapshot = if capturing { pads.first().map(|g| snapshot(g, pads.len())) } else { None };
            if !fire.is_empty() || snapshot.is_some() {
                let ui = ui.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if ui.upgrade().is_none() {
                        return;
                    }
                    if super::capturing() {
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
            }

            let in_use = capturing || rumble_until.is_some() || last_input.is_some_and(|t| now - t < IDLE_AFTER);
            wait = if pads.is_empty() {
                PROBE
            } else if in_use {
                ACTIVE
            } else {
                IDLE
            };
            if let Some(t) = rumble_until {
                wait = wait.min(t.saturating_duration_since(now));
            }
        }
    }

    fn snapshot(g: &XINPUT_GAMEPAD, count: usize) -> PadSnapshot {
        let b = |x: u16| if g.wButtons & x != 0 { 1.0 } else { 0.0 };
        PadSnapshot {
            name: NAME.to_string(),
            vendor: None,
            product: None,
            buttons: [
                b(XINPUT_GAMEPAD_A), b(XINPUT_GAMEPAD_B), b(XINPUT_GAMEPAD_X), b(XINPUT_GAMEPAD_Y),
                b(XINPUT_GAMEPAD_LEFT_SHOULDER), b(XINPUT_GAMEPAD_RIGHT_SHOULDER), trigger(g.bLeftTrigger), trigger(g.bRightTrigger),
                b(XINPUT_GAMEPAD_BACK), b(XINPUT_GAMEPAD_START), b(XINPUT_GAMEPAD_LEFT_THUMB), b(XINPUT_GAMEPAD_RIGHT_THUMB),
                b(XINPUT_GAMEPAD_DPAD_UP), b(XINPUT_GAMEPAD_DPAD_DOWN), b(XINPUT_GAMEPAD_DPAD_LEFT), b(XINPUT_GAMEPAD_DPAD_RIGHT),
                // The guide button isn't reachable through public XInput.
                0.0,
            ],
            axes: [axis(g.sThumbLX), -axis(g.sThumbLY), axis(g.sThumbRX), -axis(g.sThumbRY)],
            connected_count: count,
        }
    }
}
