//! Glitch's engine: a continuously recording history buffer plus "voices" that play it back in
//! musically useful ways (repeats, reverse, tape stop, ...). Everything is tempo-synced.
//!
//! Only past and present audio is ever read, so the effect adds no latency.

use olo_dsp::delay::HistoryBuffer;
use olo_dsp::filter::{Biquad, BiquadType};
use olo_dsp::Rng;

/// About 21 seconds at 48 kHz, which bounds how long Half Speed can run before it wraps.
const HISTORY_LEN: usize = 1 << 20;
/// Crossfade time when switching between effects and back to the dry signal.
const XFADE_MS: f32 = 4.0;
/// Short fades applied where a voice jumps to a new read position, to avoid clicks.
const EDGE_FADE_MS: f32 = 1.5;

/// The twelve effects, one per key of the octave starting at C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Repeat4,
    Repeat8,
    Repeat16,
    Repeat32,
    Repeat64,
    Roll,
    Reverse,
    TapeStop,
    HalfSpeed,
    Chipmunk,
    Gate,
    Scatter,
}

impl Effect {
    pub const ALL: [Effect; 12] = [
        Effect::Repeat4,
        Effect::Repeat8,
        Effect::Repeat16,
        Effect::Repeat32,
        Effect::Repeat64,
        Effect::Roll,
        Effect::Reverse,
        Effect::TapeStop,
        Effect::HalfSpeed,
        Effect::Chipmunk,
        Effect::Gate,
        Effect::Scatter,
    ];

    /// The effect for a MIDI note. Every octave works the same way, so any keyboard will do.
    pub fn from_note(note: u8) -> Effect {
        Self::ALL[(note % 12) as usize]
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|e| *e == self).unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Effect::Repeat4 => "1/4",
            Effect::Repeat8 => "1/8",
            Effect::Repeat16 => "1/16",
            Effect::Repeat32 => "1/32",
            Effect::Repeat64 => "1/64",
            Effect::Roll => "Roll",
            Effect::Reverse => "Reverse",
            Effect::TapeStop => "Tape Stop",
            Effect::HalfSpeed => "Half Speed",
            Effect::Chipmunk => "Chipmunk",
            Effect::Gate => "Gate",
            Effect::Scatter => "Scatter",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Effect::Repeat4 => "Repeat a quarter note",
            Effect::Repeat8 => "Repeat an eighth note",
            Effect::Repeat16 => "Repeat a sixteenth note",
            Effect::Repeat32 => "Fast stutter",
            Effect::Repeat64 => "Buzzy machine-gun stutter",
            Effect::Roll => "Repeats that speed up every beat, like a build-up",
            Effect::Reverse => "Play what just happened backwards",
            Effect::TapeStop => "Slow down to a stop like a tape machine",
            Effect::HalfSpeed => "Slowed down an octave lower",
            Effect::Chipmunk => "Sped up an octave higher, looping",
            Effect::Gate => "Chop the sound on and off in sixteenths",
            Effect::Scatter => "Randomly rearrange the last couple of beats",
        }
    }
}

/// One playing effect.
struct Voice {
    effect: Effect,
    /// Absolute sample index of the trigger.
    start: i64,
    /// Samples since the trigger.
    t: i64,
    /// Samples per beat when the voice started.
    spb: f64,
    /// Tape stop read position.
    pos: f64,
    /// Scatter state: current chunk's source start and direction.
    scatter_src: i64,
    scatter_reverse: bool,
}

impl Voice {
    fn new(effect: Effect, start: i64, spb: f64) -> Self {
        Self {
            effect,
            start,
            t: 0,
            spb,
            pos: start as f64,
            scatter_src: start,
            scatter_reverse: false,
        }
    }

    /// Fade gain near the edges of a segment of `len` samples at `phase`.
    #[inline]
    fn edge(phase: f64, len: f64, fade: f64, fade_in: bool) -> f32 {
        let fade = fade.min(len * 0.25).max(1.0);
        let out = ((len - phase) / fade).min(1.0);
        let inn = if fade_in {
            (phase / fade).min(1.0)
        } else {
            1.0
        };
        (out.min(inn)).max(0.0) as f32
    }

    /// Render one stereo sample. `now` is the index of the most recently written sample.
    fn process(
        &mut self,
        history: &[HistoryBuffer; 2],
        now: i64,
        fade: f64,
        rng: &mut Rng,
    ) -> (f32, f32) {
        let t = self.t as f64;
        let read = |pos: f64| (history[0].get_linear(pos), history[1].get_linear(pos));
        let out = match self.effect {
            Effect::Repeat4
            | Effect::Repeat8
            | Effect::Repeat16
            | Effect::Repeat32
            | Effect::Repeat64
            | Effect::Roll => {
                let len = match self.effect {
                    Effect::Repeat4 => self.spb,
                    Effect::Repeat8 => self.spb / 2.0,
                    Effect::Repeat16 => self.spb / 4.0,
                    Effect::Repeat32 => self.spb / 8.0,
                    Effect::Repeat64 => self.spb / 16.0,
                    // Halve the repeat length every beat: 1/4, 1/8, 1/16, then 1/32
                    _ => {
                        let beat = (t / self.spb).floor().min(3.0);
                        self.spb / 2f64.powf(beat)
                    }
                }
                .max(16.0);
                let phase = t % len;
                // The first pass is the live input being recorded, so it needs no fade in
                let gain = Self::edge(phase, len, fade, t >= len);
                let (l, r) = read(self.start as f64 + phase);
                (l * gain, r * gain)
            }
            Effect::Reverse => {
                let len = (self.spb / 2.0).max(16.0);
                let chunk = (t / len).floor();
                let phase = t - chunk * len;
                // Each chunk plays the chunk before it backwards
                let chunk_end = self.start as f64 + chunk * len;
                let gain = Self::edge(phase, len, fade, true);
                let (l, r) = read(chunk_end - phase - 1.0);
                (l * gain, r * gain)
            }
            Effect::TapeStop => {
                let stop_time = self.spb;
                let progress = (t / stop_time).min(1.0);
                let rate = (1.0 - progress).powf(1.6);
                self.pos += rate;
                let gain = (rate * 6.0).min(1.0) as f32;
                let (l, r) = read(self.pos.min(now as f64));
                (l * gain, r * gain)
            }
            Effect::HalfSpeed => {
                let mut pos = self.start as f64 + t * 0.5;
                // Stay within the recorded history if someone holds this for a very long time
                let max_lag = (HISTORY_LEN as f64) * 0.9;
                if now as f64 - pos > max_lag {
                    pos = now as f64 - max_lag;
                }
                read(pos)
            }
            Effect::Chipmunk => {
                // Loop the last half beat at double speed
                let len = (self.spb / 2.0).max(32.0);
                let phase = (t * 2.0) % len;
                let gain = Self::edge(phase, len, fade * 2.0, true);
                let (l, r) = read(self.start as f64 - len + phase);
                (l * gain, r * gain)
            }
            Effect::Gate => {
                let step = (self.spb / 4.0).max(16.0);
                let phase = t % step;
                let open = step * 0.5;
                let gain = if phase < open {
                    Self::edge(phase, open, fade, true)
                } else {
                    0.0
                };
                let (l, r) = read(now as f64);
                (l * gain, r * gain)
            }
            Effect::Scatter => {
                let len = (self.spb / 4.0).max(16.0);
                let phase = t % len;
                if self.t == 0 || phase < 1.0 {
                    // Pick a random sixteenth from the last two beats
                    let chunk_start = self.start + (t - phase) as i64;
                    let back = 1 + (rng.next_u32() % 8) as i64;
                    self.scatter_src = chunk_start - back * len as i64;
                    self.scatter_reverse = rng.next_f32() < 0.3;
                }
                let gain = Self::edge(phase, len, fade, true);
                let offset = if self.scatter_reverse {
                    len - 1.0 - phase
                } else {
                    phase
                };
                let (l, r) = read(self.scatter_src as f64 + offset);
                (l * gain, r * gain)
            }
        };
        self.t += 1;
        out
    }
}

/// User settings, applied once per block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Bit crushing and sample-rate reduction of the glitched signal, 0..1.
    pub crush: f32,
    /// -1 = lowpass (dark), 0 = off, 1 = highpass (thin). Applies to the glitched signal.
    pub tone: f32,
    /// How much of the glitched signal replaces the dry signal while an effect is active.
    pub mix: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            crush: 0.0,
            tone: 0.0,
            mix: 1.0,
        }
    }
}

pub struct GlitchEngine {
    sample_rate: f32,
    settings: Settings,
    history: [HistoryBuffer; 2],
    /// Index of the most recently written sample.
    now: i64,
    spb: f64,

    current: Option<Voice>,
    previous: Option<Voice>,
    previous_gain: f32,
    wet: f32,
    wet_target: f32,
    xfade_step: f32,
    edge_fade: f64,

    filters: [Biquad; 2],
    filter_active: bool,
    crush_hold: [f32; 2],
    crush_counter: f32,

    rng: Rng,
}

impl GlitchEngine {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            settings: Settings::default(),
            history: [
                HistoryBuffer::new(HISTORY_LEN),
                HistoryBuffer::new(HISTORY_LEN),
            ],
            now: 0,
            spb: 0.5 * sample_rate as f64,
            current: None,
            previous: None,
            previous_gain: 0.0,
            wet: 0.0,
            wet_target: 0.0,
            xfade_step: 1.0 / (XFADE_MS * 0.001 * sample_rate),
            edge_fade: (EDGE_FADE_MS * 0.001 * sample_rate) as f64,
            filters: [Biquad::default(); 2],
            filter_active: false,
            crush_hold: [0.0; 2],
            crush_counter: 0.0,
            rng: Rng::new(0x611c),
        }
    }

    pub fn reset(&mut self) {
        for h in &mut self.history {
            h.clear();
        }
        self.current = None;
        self.previous = None;
        self.wet = 0.0;
        self.wet_target = 0.0;
        for f in &mut self.filters {
            f.reset();
        }
    }

    /// Set the tempo in beats per minute.
    pub fn set_tempo(&mut self, bpm: f64) {
        let bpm = if bpm.is_finite() && bpm > 1.0 {
            bpm
        } else {
            120.0
        };
        self.spb = 60.0 / bpm * self.sample_rate as f64;
    }

    pub fn samples_per_beat(&self) -> f64 {
        self.spb
    }

    pub fn set(&mut self, settings: &Settings) {
        if settings.tone != self.settings.tone || !self.filter_active && settings.tone != 0.0 {
            let tone = settings.tone.clamp(-1.0, 1.0);
            let was_active = self.filter_active;
            self.filter_active = tone.abs() > 0.01;
            for f in &mut self.filters {
                if !was_active {
                    f.reset();
                }
                if tone < 0.0 {
                    let cutoff = 18_000.0 * (250.0f32 / 18_000.0).powf(-tone);
                    f.set(BiquadType::Lowpass, cutoff, 0.9, self.sample_rate);
                } else {
                    let cutoff = 20.0 * (4000.0f32 / 20.0).powf(tone);
                    f.set(BiquadType::Highpass, cutoff, 0.9, self.sample_rate);
                }
            }
        }
        self.settings = *settings;
    }

    /// Start an effect now. Replaces whatever is currently playing with a quick crossfade.
    pub fn trigger(&mut self, effect: Effect) {
        let start = self.now + 1;
        if let Some(old) = self.current.take() {
            if self.wet > 0.0 {
                self.previous = Some(old);
                self.previous_gain = 1.0;
            }
        }
        self.current = Some(Voice::new(effect, start, self.spb));
        self.wet_target = 1.0;
    }

    /// Stop the current effect and fade back to the dry signal.
    pub fn release(&mut self) {
        self.wet_target = 0.0;
    }

    /// The effect currently audible, if any.
    pub fn active_effect(&self) -> Option<Effect> {
        if self.wet_target > 0.0 {
            self.current.as_ref().map(|v| v.effect)
        } else {
            None
        }
    }

    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        self.now += 1;
        self.history[0].write(self.now, in_l);
        self.history[1].write(self.now, in_r);

        if self.current.is_none() && self.previous.is_none() {
            return (in_l, in_r);
        }

        let mut wet = (0.0, 0.0);
        if let Some(voice) = &mut self.current {
            wet = voice.process(&self.history, self.now, self.edge_fade, &mut self.rng);
        }
        if let Some(voice) = &mut self.previous {
            let (pl, pr) = voice.process(&self.history, self.now, self.edge_fade, &mut self.rng);
            let g = self.previous_gain;
            wet = (wet.0 * (1.0 - g) + pl * g, wet.1 * (1.0 - g) + pr * g);
            self.previous_gain -= self.xfade_step;
            if self.previous_gain <= 0.0 {
                self.previous = None;
            }
        }

        if self.wet < self.wet_target {
            self.wet = (self.wet + self.xfade_step).min(self.wet_target);
        } else if self.wet > self.wet_target {
            self.wet = (self.wet - self.xfade_step).max(self.wet_target);
            if self.wet <= 0.0 {
                self.current = None;
                self.previous = None;
            }
        }

        let (mut wl, mut wr) = wet;
        if self.filter_active {
            wl = self.filters[0].process(wl);
            wr = self.filters[1].process(wr);
        }
        let crush = self.settings.crush;
        if crush > 0.001 {
            // Sample-rate reduction: hold samples for up to 16 samples
            self.crush_counter += 1.0;
            if self.crush_counter >= 1.0 + crush * 15.0 {
                self.crush_counter = 0.0;
                self.crush_hold = [wl, wr];
            }
            let bits = 16.0 - 12.0 * crush;
            let steps = 2f32.powf(bits - 1.0);
            let drive = 1.0 + crush * 2.0;
            wl = olo_dsp::soft_clip((self.crush_hold[0] * steps).round() / steps * drive);
            wr = olo_dsp::soft_clip((self.crush_hold[1] * steps).round() / steps * drive);
        }

        let amount = self.wet * self.settings.mix;
        (
            olo_dsp::sanitize(in_l * (1.0 - amount) + wl * amount),
            olo_dsp::sanitize(in_r * (1.0 - amount) + wr * amount),
        )
    }
}

/// Decides when to fire effects automatically while the song is playing.
pub struct AutoGlitch {
    rng: Rng,
    last_step: i64,
    /// Step index at which the running auto effect should stop.
    end_step: Option<i64>,
}

/// Effects auto mode picks from, with duplicates acting as weights.
const AUTO_POOL: [Effect; 14] = [
    Effect::Repeat8,
    Effect::Repeat16,
    Effect::Repeat16,
    Effect::Repeat32,
    Effect::Repeat32,
    Effect::Repeat64,
    Effect::Reverse,
    Effect::Reverse,
    Effect::HalfSpeed,
    Effect::Chipmunk,
    Effect::Gate,
    Effect::Scatter,
    Effect::Scatter,
    Effect::TapeStop,
];

/// What the auto mode wants to happen at this sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoAction {
    Trigger(Effect),
    Release,
}

impl Default for AutoGlitch {
    fn default() -> Self {
        Self {
            rng: Rng::new(0xa070),
            last_step: i64::MIN,
            end_step: None,
        }
    }
}

impl AutoGlitch {
    pub fn reset(&mut self) {
        self.last_step = i64::MIN;
        self.end_step = None;
    }

    pub fn is_running(&self) -> bool {
        self.end_step.is_some()
    }

    /// Forget about the running auto effect, e.g. because the player took over.
    pub fn cancel(&mut self) {
        self.end_step = None;
    }

    /// Call once per sample with the song position in beats. Works on an eighth-note grid.
    pub fn tick(&mut self, beats: f64, chance: f32, allowed: bool) -> Option<AutoAction> {
        let step = (beats * 2.0).floor() as i64;
        if step == self.last_step {
            return None;
        }
        let jumped = step != self.last_step.wrapping_add(1);
        self.last_step = step;

        if let Some(end) = self.end_step {
            if step >= end || jumped || !allowed {
                self.end_step = None;
                return Some(AutoAction::Release);
            }
            return None;
        }

        if allowed && chance > 0.0 && self.rng.next_f32() < chance {
            let effect = AUTO_POOL[(self.rng.next_u32() as usize) % AUTO_POOL.len()];
            let length = [1, 2, 2, 4][(self.rng.next_u32() % 4) as usize];
            self.end_step = Some(step + length);
            return Some(AutoAction::Trigger(effect));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn ramp(i: i64) -> f32 {
        // A slow ramp makes read positions easy to check
        ((i % 48_000) as f32) / 48_000.0
    }

    #[test]
    fn dry_passthrough_when_idle() {
        let mut e = GlitchEngine::new(SR);
        for i in 0..1000 {
            let x = (i as f32 * 0.01).sin();
            let (l, r) = e.process(x, -x);
            assert_eq!(l, x);
            assert_eq!(r, -x);
        }
    }

    #[test]
    fn repeat_loops_the_captured_slice() {
        let mut e = GlitchEngine::new(SR);
        e.set_tempo(120.0); // 24000 samples per beat, 1/16 = 6000
        for i in 0..10_000 {
            e.process(ramp(i), ramp(i));
        }
        e.trigger(Effect::Repeat16);
        let mut out = Vec::new();
        for i in 10_000..40_000 {
            out.push(e.process(ramp(i), ramp(i)).0);
        }
        // Halfway through the 2nd and 3rd repeats we should hear the same audio as the first
        let a = out[3000];
        let b = out[6000 + 3000];
        let c = out[12000 + 3000];
        assert!((a - b).abs() < 1e-3 && (a - c).abs() < 1e-3, "{a} {b} {c}");
        // And it's not just the live input
        assert!((c - ramp(10_000 + 12_000 + 3000)).abs() > 0.1);
    }

    #[test]
    fn release_returns_to_dry() {
        let mut e = GlitchEngine::new(SR);
        e.trigger(Effect::Gate);
        for i in 0..5000 {
            e.process(ramp(i), ramp(i));
        }
        e.release();
        for i in 5000..6000 {
            e.process(ramp(i), ramp(i));
        }
        let (l, _) = e.process(0.3, 0.3);
        assert_eq!(l, 0.3);
        assert!(e.active_effect().is_none());
    }

    #[test]
    fn half_speed_halves_the_frequency() {
        let mut e = GlitchEngine::new(SR);
        let sine = |i: i64| (TAU * 440.0 * i as f32 / SR).sin();
        for i in 0..1000 {
            e.process(sine(i), sine(i));
        }
        e.trigger(Effect::HalfSpeed);
        let out: Vec<f32> = (1000..49_000)
            .map(|i| e.process(sine(i), sine(i)).0)
            .collect();
        let crossings = out[1000..]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let freq = crossings as f32 * SR / (out.len() - 1000) as f32;
        assert!((freq - 220.0).abs() < 5.0, "got {freq}");
    }

    #[test]
    fn all_effects_are_finite_and_bounded() {
        let mut e = GlitchEngine::new(SR);
        e.set(&Settings {
            crush: 0.7,
            tone: -0.5,
            mix: 1.0,
        });
        let mut rng = Rng::new(9);
        let mut i = 0i64;
        for effect in Effect::ALL {
            e.trigger(effect);
            for _ in 0..60_000 {
                let x = rng.bipolar() * 0.8;
                let (l, r) = e.process(x, x);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.5 && r.abs() <= 1.5, "{effect:?} {l}");
                i += 1;
            }
        }
        assert!(i > 0);
    }

    #[test]
    fn effects_map_by_pitch_class() {
        assert_eq!(Effect::from_note(60), Effect::Repeat4);
        assert_eq!(Effect::from_note(48), Effect::Repeat4);
        assert_eq!(Effect::from_note(66), Effect::Reverse);
        assert_eq!(Effect::from_note(71), Effect::Scatter);
    }

    #[test]
    fn auto_mode_triggers_and_releases_on_the_grid() {
        let mut auto = AutoGlitch::default();
        let mut triggers = 0i32;
        let mut releases = 0i32;
        let spb = 24_000.0;
        for i in 0..(spb as i64 * 64) {
            match auto.tick(i as f64 / spb, 0.5, true) {
                Some(AutoAction::Trigger(_)) => triggers += 1,
                Some(AutoAction::Release) => releases += 1,
                None => {}
            }
        }
        assert!(triggers > 10, "{triggers}");
        assert!((triggers - releases).abs() <= 1);

        // Zero chance never fires
        let mut auto = AutoGlitch::default();
        for i in 0..(spb as i64 * 16) {
            assert!(auto.tick(i as f64 / spb, 0.0, true).is_none());
        }
    }
}
