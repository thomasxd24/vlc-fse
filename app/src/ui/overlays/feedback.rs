//! Navigation feedback: short synthesised UI sounds and controller rumble — the renderer's `Sound`
//! (core.js, a WebAudio oscillator per `tone(...)`) and `Nav.haptic`.
//!
//! Screens call `Input.feedback(kind)` (or `input::feedback`) with `move`, `select`, `back`, `edge` or
//! `open`; the intro plays `boot`. Sounds follow Settings › Sounds (`Input.sounds`), rumble follows
//! Settings › Vibration (`Input.haptics`) and, like the renderer, only happens in pad mode.
//!
//! Both outputs are Windows-only in this build, like the controller itself: sounds go out through `cpal`
//! (WASAPI; on Linux it would need ALSA's development files) and rumble through `gilrs`. The synthesis is
//! plain Rust and runs (and is tested) everywhere.

use super::super::ctx::{cx, Ctx};
use super::super::input;
use crate::Input;
use slint::ComponentHandle;

pub fn install(_ctx: &Ctx) {
    let player = out::Player::start();
    let rumble = out::Rumble::start();
    input::on_feedback(move |kind| {
        let ui = cx().ui();
        let inp = ui.global::<Input>();
        if inp.get_sounds() {
            let tones = sound(kind);
            if !tones.is_empty() {
                player.play(tones);
            }
        }
        if inp.get_haptics() && inp.get_mode() == "pad" {
            if let Some((strength, ms)) = haptic(kind) {
                rumble.pulse(strength, ms);
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wave {
    Sine,
    Triangle,
}

/// One oscillator note: `tone(freq, dur, vol, type, slideTo)`, optionally starting `delay` s later.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub freq: f32,
    pub dur: f32,
    pub vol: f32,
    pub wave: Wave,
    pub slide_to: Option<f32>,
    pub delay: f32,
}

const fn tone(freq: f32, dur: f32, vol: f32, wave: Wave, slide_to: Option<f32>) -> Tone {
    Tone { freq, dur, vol, wave, slide_to, delay: 0.0 }
}

/// The renderer's sound for each feedback kind (quiet enough not to be annoying).
pub fn sound(kind: &str) -> Vec<Tone> {
    use Wave::*;
    match kind {
        "move" => vec![tone(1500.0, 0.028, 0.018, Sine, None)],
        "select" => vec![tone(700.0, 0.07, 0.04, Sine, Some(1050.0))],
        "back" => vec![tone(560.0, 0.07, 0.03, Sine, Some(360.0))],
        "edge" => vec![tone(170.0, 0.05, 0.025, Triangle, None)],
        "open" => vec![tone(440.0, 0.09, 0.03, Sine, Some(880.0))],
        // Startup: a soft rising two-note chime under the intro.
        "boot" => vec![tone(392.0, 0.5, 0.03, Sine, Some(523.0)), Tone { delay: 0.17, ..tone(659.0, 0.7, 0.025, Sine, Some(784.0)) }],
        _ => Vec::new(),
    }
}

/// The renderer's rumble for each kind: (weak-motor strength 0..1, milliseconds).
pub fn haptic(kind: &str) -> Option<(f32, u32)> {
    match kind {
        "move" => Some((0.18, 10)),
        "select" => Some((0.35, 16)),
        "edge" => Some((0.5, 22)),
        _ => None,
    }
}

/// A playing tone: the WebAudio graph `oscillator → gain` with exponential ramps, sample by sample.
#[derive(Debug, Clone)]
pub struct Voice {
    tone: Tone,
    rate: f32,
    pos: u64,
    phase: f32,
}

impl Voice {
    pub fn new(tone: Tone, sample_rate: u32) -> Self {
        Voice { tone, rate: sample_rate as f32, pos: 0, phase: 0.0 }
    }

    /// The oscillator stops 20 ms after its ramps end.
    pub fn done(&self) -> bool {
        self.pos as f32 / self.rate > self.tone.delay + self.tone.dur + 0.02
    }

    pub fn next_sample(&mut self) -> f32 {
        let t = self.pos as f32 / self.rate - self.tone.delay;
        self.pos += 1;
        if t < 0.0 || self.done() {
            return 0.0;
        }
        let Tone { freq, dur, vol, wave, slide_to, .. } = self.tone;
        let k = (t / dur).min(1.0);
        let f = match slide_to {
            Some(to) => freq * (to / freq).powf(k),
            None => freq,
        };
        let gain = vol * (0.0001 / vol).powf(k);
        let p = self.phase;
        self.phase = (self.phase + f / self.rate).fract();
        let s = match wave {
            Wave::Sine => (p * std::f32::consts::TAU).sin(),
            // Starts at 0 and rises, like WebAudio's triangle.
            Wave::Triangle => {
                let q = (p + 0.25).fract();
                4.0 * (q - 0.5).abs() - 1.0
            }
        };
        s * gain
    }
}

/// Sum the voices into `frames` samples (mono), dropping the ones that have finished.
pub fn mix(voices: &mut Vec<Voice>, frames: usize, mut write: impl FnMut(f32)) {
    for _ in 0..frames {
        let s: f32 = voices.iter_mut().map(Voice::next_sample).sum();
        write(s.clamp(-1.0, 1.0));
    }
    voices.retain(|v| !v.done());
}

#[cfg(windows)]
mod out {
    //! Windows: sounds through cpal (WASAPI), rumble through gilrs. Each runs on its own thread, fed by
    //! a channel, so nothing blocks the event loop.
    use super::{mix, Tone, Voice};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, Stream};
    use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Pause the stream after this long without sounds: an open audio stream keeps Windows from sleeping.
    const IDLE: Duration = Duration::from_secs(2);
    /// Don't retry a failing audio device on every key press.
    const RETRY: Duration = Duration::from_secs(5);

    enum Msg {
        Play(Vec<Tone>),
        Reset,
    }

    pub struct Player {
        tx: Sender<Msg>,
    }

    type Mixer = Arc<Mutex<Vec<Voice>>>;

    impl Player {
        pub fn start() -> Self {
            let (tx, rx) = channel::<Msg>();
            let tx2 = tx.clone();
            let _ = std::thread::Builder::new().name("lounge-sounds".into()).spawn(move || {
                let mixer: Mixer = Arc::new(Mutex::new(Vec::new()));
                let mut out: Option<(Stream, u32)> = None;
                let mut failed_at: Option<Instant> = None;
                let mut playing = false;
                loop {
                    match rx.recv_timeout(IDLE) {
                        Ok(Msg::Play(tones)) => {
                            if out.is_none() && failed_at.is_none_or(|t| t.elapsed() > RETRY) {
                                out = open(&mixer, tx2.clone());
                                failed_at = if out.is_none() { Some(Instant::now()) } else { None };
                            }
                            let Some((stream, rate)) = &out else { continue };
                            mixer.lock().unwrap().extend(tones.into_iter().map(|t| Voice::new(t, *rate)));
                            if !playing {
                                playing = stream.play().is_ok();
                            }
                        }
                        Ok(Msg::Reset) => {
                            out = None;
                            playing = false;
                            mixer.lock().unwrap().clear();
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some((stream, _)) = &out {
                                if playing && mixer.lock().unwrap().is_empty() {
                                    let _ = stream.pause();
                                    playing = false;
                                }
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
            Player { tx }
        }

        pub fn play(&self, tones: Vec<Tone>) {
            let _ = self.tx.send(Msg::Play(tones));
        }
    }

    fn open(mixer: &Mixer, tx: Sender<Msg>) -> Option<(Stream, u32)> {
        let device = cpal::default_host().default_output_device()?;
        let supported = device.default_output_config().ok()?;
        let config = supported.config();
        let rate = config.sample_rate;
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, mixer, tx),
            SampleFormat::F64 => build::<f64>(&device, config, mixer, tx),
            SampleFormat::I16 => build::<i16>(&device, config, mixer, tx),
            SampleFormat::I32 => build::<i32>(&device, config, mixer, tx),
            SampleFormat::U16 => build::<u16>(&device, config, mixer, tx),
            SampleFormat::U8 => build::<u8>(&device, config, mixer, tx),
            SampleFormat::I8 => build::<i8>(&device, config, mixer, tx),
            _ => None,
        }?;
        Some((stream, rate))
    }

    fn build<T: SizedSample + FromSample<f32>>(device: &cpal::Device, config: cpal::StreamConfig, mixer: &Mixer, tx: Sender<Msg>) -> Option<Stream> {
        let channels = config.channels.max(1) as usize;
        let mixer = mixer.clone();
        device
            .build_output_stream::<T, _, _>(
                config,
                move |data: &mut [T], _| {
                    let mut voices = mixer.lock().unwrap();
                    let mut i = 0;
                    mix(&mut voices, data.len() / channels, |s| {
                        for _ in 0..channels {
                            data[i] = T::from_sample(s);
                            i += 1;
                        }
                    });
                },
                move |e| {
                    // The device went away (headphones unplugged…): open the new default next time.
                    if !matches!(e.kind(), cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied) {
                        let _ = tx.send(Msg::Reset);
                    }
                },
                None,
            )
            .ok()
    }

    pub struct Rumble {
        tx: Sender<(f32, u32)>,
    }

    impl Rumble {
        pub fn start() -> Self {
            use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Repeat, Replay, Ticks};
            let (tx, rx) = channel::<(f32, u32)>();
            let _ = std::thread::Builder::new().name("lounge-rumble".into()).spawn(move || {
                let Ok(mut gilrs) = gilrs::Gilrs::new() else { return };
                // Kept alive while it plays (dropping an effect stops it).
                let mut _current = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok((strength, ms)) => {
                            while gilrs.next_event().is_some() {}
                            let pads: Vec<_> = gilrs.gamepads().filter(|(_, g)| g.is_connected() && g.is_ff_supported()).map(|(id, _)| id).collect();
                            if pads.is_empty() {
                                continue;
                            }
                            let ticks = Ticks::from_ms(ms);
                            let effect = EffectBuilder::new()
                                .add_effect(BaseEffect {
                                    kind: BaseEffectType::Weak { magnitude: (strength.clamp(0.0, 1.0) * u16::MAX as f32) as u16 },
                                    scheduling: Replay { play_for: ticks, ..Default::default() },
                                    envelope: Default::default(),
                                })
                                .repeat(Repeat::For(ticks))
                                .gamepads(&pads)
                                .finish(&mut gilrs);
                            if let Ok(effect) = effect {
                                let _ = effect.play();
                                _current = Some(effect);
                            }
                        }
                        // Keep gilrs' gamepad list current.
                        Err(RecvTimeoutError::Timeout) => while gilrs.next_event().is_some() {},
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
            Rumble { tx }
        }

        pub fn pulse(&self, strength: f32, ms: u32) {
            let _ = self.tx.send((strength, ms));
        }
    }
}

#[cfg(not(windows))]
mod out {
    //! No audio or rumble output outside Windows in this build (see the module docs).
    use super::Tone;

    pub struct Player;
    impl Player {
        pub fn start() -> Self {
            Player
        }
        pub fn play(&self, _tones: Vec<Tone>) {}
    }

    pub struct Rumble;
    impl Rumble {
        pub fn start() -> Self {
            Rumble
        }
        pub fn pulse(&self, _strength: f32, _ms: u32) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(kind: &str, rate: u32) -> Vec<f32> {
        let mut voices: Vec<Voice> = sound(kind).into_iter().map(|t| Voice::new(t, rate)).collect();
        let mut out = Vec::new();
        while !voices.is_empty() {
            mix(&mut voices, 256, |s| out.push(s));
        }
        out
    }

    #[test]
    fn every_kind_has_a_sound_and_known_rumble() {
        for k in ["move", "select", "back", "edge", "open", "boot"] {
            assert!(!sound(k).is_empty(), "{k}");
        }
        assert!(sound("nope").is_empty());
        assert_eq!(haptic("select"), Some((0.35, 16)));
        assert_eq!(haptic("back"), None);
    }

    #[test]
    fn tones_are_quiet_short_and_decay() {
        let s = render("select", 48_000);
        // 70 ms + 20 ms tail, rounded up to the mixing block.
        assert!(s.len() >= 4_320 && s.len() < 4_320 + 512, "{}", s.len());
        let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= 0.04 + 1e-4 && peak > 0.03, "{peak}");
        let tail = s[3_400..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(tail < 0.001, "{tail}");
    }

    #[test]
    fn boot_chime_overlaps_its_second_note() {
        let s = render("boot", 44_100);
        // Second note: 170 ms in, 700 ms long, + 20 ms.
        assert!(s.len() as f32 / 44_100.0 >= 0.89);
    }

    #[test]
    fn frequency_slides_up() {
        // Count zero crossings in the first and last 10 ms of select's 700 → 1050 Hz sweep.
        let mut v = Voice::new(sound("select")[0], 48_000);
        let s: Vec<f32> = (0..3_360).map(|_| v.next_sample()).collect();
        let crossings = |w: &[f32]| w.windows(2).filter(|p| (p[0] <= 0.0) != (p[1] <= 0.0)).count();
        assert!(crossings(&s[2_880..3_360]) > crossings(&s[..480]));
    }
}
