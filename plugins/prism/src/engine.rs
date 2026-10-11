//! Prism's harmonizer engine.
//!
//! 1. A YIN pitch detector tracks the singer's pitch.
//! 2. Pitch marks are placed once per vocal period, snapped to the waveform's peaks.
//! 3. Each harmony voice re-synthesises the voice with TD-PSOLA: two-period grains around the
//!    marks are overlap-added at the *target* pitch's spacing. Because grains keep their shape, the
//!    voice's formants (its "vowel" character) stay put, which is what makes it sound like a choir
//!    of the same singer rather than chipmunks.
//!
//! The output is delayed by a fixed latency so grains can look a little into the "future"; the
//! plugin reports this to the host so everything stays in time.

use crate::detector::{PitchDetector, PitchFrame, MIN_F0};
use olo_dsp::delay::HistoryBuffer;
use olo_dsp::filter::OnePole;
use olo_dsp::{freq_to_note, note_to_freq, Rng};

pub const MAX_VOICES: usize = 6;
const MAX_GRAINS: usize = 24;
const MAX_MARKS: usize = 256;
const MAX_FRAMES: usize = 32;
const HISTORY: usize = 1 << 15;
/// Formant shift is limited so grains never need more look-ahead than the latency provides.
pub const MAX_FORMANT_SEMITONES: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Harmony voices follow the notes held on a MIDI keyboard.
    Keys,
    /// Harmony voices are generated automatically in a key.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Major,
    Minor,
}

impl Scale {
    fn steps(self) -> [i32; 7] {
        match self {
            Scale::Major => [0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => [0, 2, 3, 5, 7, 8, 10],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voicing {
    ThirdUp,
    ThirdFifthUp,
    ThirdDown,
    Choir,
    Octaves,
}

impl Voicing {
    /// Harmony intervals in scale steps (2 = a third, 4 = a fifth, 7 = an octave).
    fn intervals(self) -> &'static [i32] {
        match self {
            Voicing::ThirdUp => &[2],
            Voicing::ThirdFifthUp => &[2, 4],
            Voicing::ThirdDown => &[-2],
            Voicing::Choir => &[-7, 2, 4],
            Voicing::Octaves => &[-7, 7],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    /// Key root as a pitch class, 0 = C.
    pub key: i32,
    pub scale: Scale,
    pub voicing: Voicing,
    /// 1 = snap harmonies perfectly to the target notes (robotic), 0 = keep the singer's
    /// natural pitch movement.
    pub tune: f32,
    pub formant: f32,
    /// Stereo spread and detune, 0..1.
    pub spread: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub dry: f32,
    pub harmony: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Keys,
            key: 0,
            scale: Scale::Major,
            voicing: Voicing::ThirdFifthUp,
            tune: 1.0,
            formant: 0.0,
            spread: 0.5,
            attack_ms: 15.0,
            release_ms: 250.0,
            dry: 0.8,
            harmony: 0.8,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Mark {
    pos: i64,
    /// Local period in samples (or the hop size when unvoiced).
    period: f32,
    /// A slowly smoothed period, used to separate vibrato from the underlying note.
    smooth_period: f32,
    voiced: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    alive: bool,
    in_center: f64,
    out_center: f64,
    half: f32,
    rate: f32,
}

/// A raised-cosine window lookup: `hann(x)` for `x` in `[-1, 1]`.
struct Window {
    table: [f32; 513],
}

impl Window {
    fn new() -> Self {
        let mut table = [0.0; 513];
        for (i, v) in table.iter_mut().enumerate() {
            let x = i as f32 / 512.0;
            *v = 0.5 + 0.5 * (std::f32::consts::PI * x).cos();
        }
        Self { table }
    }

    #[inline]
    fn at(&self, x: f32) -> f32 {
        let p = x.abs().min(1.0) * 512.0;
        let i = (p as usize).min(511);
        let frac = p - i as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * frac
    }
}

/// The mark closest to `pos`. Marks are stored oldest to newest in a ring buffer.
fn nearest_mark(marks: &[Mark; MAX_MARKS], head: usize, count: usize, pos: f64) -> Option<Mark> {
    let mut best: Option<Mark> = None;
    let mut best_dist = f64::MAX;
    for i in 0..count {
        let m = marks[(head + MAX_MARKS - 1 - i) % MAX_MARKS];
        let dist = (m.pos as f64 - pos).abs();
        if dist < best_dist {
            best_dist = dist;
            best = Some(m);
        } else if (m.pos as f64) < pos {
            // Walking backwards in time and already past the target: it only gets further away
            break;
        }
    }
    best
}

struct Voice {
    /// The MIDI note holding this voice, in keys mode.
    midi_note: Option<u8>,
    /// Scale-step interval, in auto mode.
    interval: i32,
    in_use: bool,
    /// Target note (MIDI note number) and the glided value actually used.
    target: f32,
    note: f32,
    velocity: f32,
    env: f32,
    env_target: f32,
    pan: f32,
    detune: f32,
    next_s: f64,
    est_half: f64,
    grains: [Grain; MAX_GRAINS],
    age: u64,
}

impl Voice {
    fn new() -> Self {
        Self {
            midi_note: None,
            interval: 0,
            in_use: false,
            target: 60.0,
            note: 60.0,
            velocity: 1.0,
            env: 0.0,
            env_target: 0.0,
            pan: 0.0,
            detune: 0.0,
            next_s: 0.0,
            est_half: 0.0,
            grains: [Grain::default(); MAX_GRAINS],
            age: 0,
        }
    }

    fn is_silent(&self) -> bool {
        self.env_target == 0.0 && self.env < 1e-4
    }

    fn clear(&mut self) {
        for g in &mut self.grains {
            g.alive = false;
        }
        self.in_use = false;
        self.midi_note = None;
        self.env = 0.0;
        self.env_target = 0.0;
    }
}

pub struct PrismEngine {
    sample_rate: f32,
    settings: Settings,
    latency: usize,
    max_period: f32,
    unvoiced_step: f32,

    now: i64,
    input: HistoryBuffer,
    peaks: HistoryBuffer,
    peak_filters: [OnePole; 2],
    dry: [HistoryBuffer; 2],

    detector: PitchDetector,
    frames: [PitchFrame; MAX_FRAMES],
    frame_head: usize,
    frame_count: usize,

    marks: [Mark; MAX_MARKS],
    mark_head: usize,
    mark_count: usize,
    last_mark: i64,
    last_voiced: bool,
    smooth_period: f32,
    smooth_coeff_time: f32,

    voices: [Voice; MAX_VOICES],
    voice_clock: u64,
    window: Window,
    rng: Rng,

    /// The singer's current note snapped to the key, for auto mode.
    snapped: Option<i32>,
    /// The singer's current (smoothed) note, for the GUI. Negative when unvoiced.
    sung: f32,

    attack_coeff: f32,
    release_coeff: f32,
    glide_coeff: f32,
    norm: f32,
}

impl PrismEngine {
    pub fn new(sample_rate: f32) -> Self {
        let detector = PitchDetector::new(sample_rate);
        let max_period = sample_rate / MIN_F0;
        let hop = (sample_rate * 0.005).ceil();
        let latency = (max_period * 3.3 + hop).ceil() as usize;
        let mut peak_filters = [OnePole::default(); 2];
        for f in &mut peak_filters {
            f.set_cutoff(900.0, sample_rate);
        }

        let mut engine = Self {
            sample_rate,
            settings: Settings::default(),
            latency,
            max_period,
            unvoiced_step: (sample_rate * 0.005).round(),
            now: 0,
            input: HistoryBuffer::new(HISTORY),
            peaks: HistoryBuffer::new(HISTORY),
            peak_filters,
            dry: [HistoryBuffer::new(HISTORY), HistoryBuffer::new(HISTORY)],
            detector,
            frames: [PitchFrame {
                center: 0,
                period: 0.0,
                voiced: false,
                aperiodicity: 1.0,
            }; MAX_FRAMES],
            frame_head: 0,
            frame_count: 0,
            marks: [Mark::default(); MAX_MARKS],
            mark_head: 0,
            mark_count: 0,
            last_mark: 0,
            last_voiced: false,
            smooth_period: 0.0,
            smooth_coeff_time: 0.15 * sample_rate,
            voices: std::array::from_fn(|_| Voice::new()),
            voice_clock: 0,
            window: Window::new(),
            rng: Rng::new(0x9215),
            snapped: None,
            sung: -1.0,
            attack_coeff: 1.0,
            release_coeff: 1.0,
            glide_coeff: 1.0,
            norm: 1.0,
        };
        engine.glide_coeff = 1.0 - (-1.0 / (0.025 * sample_rate)).exp();
        engine.set(&Settings::default());
        engine
    }

    /// Processing latency in samples. Report this to the host.
    pub fn latency(&self) -> usize {
        self.latency
    }

    pub fn reset(&mut self) {
        self.input.clear();
        self.peaks.clear();
        self.dry[0].clear();
        self.dry[1].clear();
        self.detector.reset();
        for f in &mut self.peak_filters {
            f.reset();
        }
        self.frame_count = 0;
        self.mark_count = 0;
        self.last_mark = self.now;
        self.last_voiced = false;
        self.snapped = None;
        self.sung = -1.0;
        for v in &mut self.voices {
            v.clear();
        }
    }

    pub fn set(&mut self, settings: &Settings) {
        let previous = self.settings;
        self.settings = *settings;
        self.settings.formant = settings
            .formant
            .clamp(-MAX_FORMANT_SEMITONES, MAX_FORMANT_SEMITONES);

        let coeff = |ms: f32, sr: f32| 1.0 - (-1.0 / (ms.max(0.5) * 0.001 * sr)).exp();
        self.attack_coeff = coeff(settings.attack_ms, self.sample_rate);
        self.release_coeff = coeff(settings.release_ms, self.sample_rate);

        if previous.mode != settings.mode {
            for v in &mut self.voices {
                v.env_target = 0.0;
                v.midi_note = None;
                v.in_use = false;
            }
        }
        if settings.mode == Mode::Auto
            && (previous.mode != Mode::Auto
                || previous.voicing != settings.voicing
                || previous.spread != settings.spread)
        {
            self.setup_auto_voices();
        }
        if previous.spread != settings.spread {
            self.update_pans();
        }
    }

    fn setup_auto_voices(&mut self) {
        let intervals = self.settings.voicing.intervals();
        let t_out = (self.now - self.latency as i64) as f64;
        for (i, voice) in self.voices.iter_mut().enumerate() {
            if let Some(&interval) = intervals.get(i) {
                if !voice.in_use || voice.is_silent() {
                    voice.next_s = t_out;
                }
                voice.in_use = true;
                voice.interval = interval;
                voice.midi_note = None;
                voice.env_target = 1.0;
                voice.velocity = 1.0;
            } else {
                voice.env_target = 0.0;
                voice.in_use = false;
            }
        }
        self.update_pans();
    }

    fn update_pans(&mut self) {
        // Spread voices out from the centre: alternate left and right
        const POSITIONS: [f32; MAX_VOICES] = [-0.7, 0.7, -0.35, 0.35, -1.0, 1.0];
        let spread = self.settings.spread;
        let single =
            self.settings.mode == Mode::Auto && self.settings.voicing.intervals().len() == 1;
        for (i, voice) in self.voices.iter_mut().enumerate() {
            voice.pan = if single { 0.0 } else { POSITIONS[i] * spread };
        }
    }

    /// Start a harmony voice for a MIDI note (keys mode).
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        if self.settings.mode != Mode::Keys {
            return;
        }
        self.voice_clock += 1;
        let t_out = (self.now - self.latency as i64) as f64;
        let index = self
            .voices
            .iter()
            .position(|v| v.midi_note == Some(note))
            .or_else(|| {
                self.voices
                    .iter()
                    .position(|v| v.midi_note.is_none() && v.is_silent())
            })
            .or_else(|| self.voices.iter().position(|v| v.midi_note.is_none()))
            .unwrap_or_else(|| {
                // Steal the oldest voice
                (0..MAX_VOICES)
                    .min_by_key(|&i| self.voices[i].age)
                    .unwrap_or(0)
            });
        let detune = self.rng.bipolar();
        let voice = &mut self.voices[index];
        if voice.is_silent() {
            for g in &mut voice.grains {
                g.alive = false;
            }
            voice.next_s = t_out;
            voice.note = note as f32;
        }
        voice.in_use = true;
        voice.midi_note = Some(note);
        voice.target = note as f32;
        voice.velocity = 0.55 + 0.45 * velocity.clamp(0.0, 1.0);
        voice.env_target = 1.0;
        voice.detune = detune;
        voice.age = self.voice_clock;
    }

    pub fn note_off(&mut self, note: u8) {
        for v in &mut self.voices {
            if v.midi_note == Some(note) {
                v.midi_note = None;
                v.env_target = 0.0;
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.midi_note.is_some() {
                v.midi_note = None;
                v.env_target = 0.0;
            }
        }
    }

    /// The singer's current note (fractional MIDI note), or `None` when not singing.
    pub fn sung_note(&self) -> Option<f32> {
        (self.sung >= 0.0).then_some(self.sung)
    }

    /// The note each harmony voice is currently singing, for display.
    pub fn voice_note(&self, index: usize) -> Option<f32> {
        let v = self.voices.get(index)?;
        if v.env_target > 0.0 && v.in_use {
            match self.settings.mode {
                Mode::Keys => Some(v.target),
                Mode::Auto => self.snapped.map(|_| v.target),
            }
        } else {
            None
        }
    }

    fn push_frame(&mut self, frame: PitchFrame) {
        self.frames[self.frame_head] = frame;
        self.frame_head = (self.frame_head + 1) % MAX_FRAMES;
        self.frame_count = (self.frame_count + 1).min(MAX_FRAMES);
    }

    fn frame_near(&self, pos: i64) -> Option<PitchFrame> {
        (0..self.frame_count)
            .map(|i| self.frames[(self.frame_head + MAX_FRAMES - 1 - i) % MAX_FRAMES])
            .min_by_key(|f| (f.center - pos).abs())
    }

    fn push_mark(&mut self, mark: Mark) {
        self.marks[self.mark_head] = mark;
        self.mark_head = (self.mark_head + 1) % MAX_MARKS;
        self.mark_count = (self.mark_count + 1).min(MAX_MARKS);
    }

    /// Place pitch marks for all input that has enough look-ahead available.
    fn generate_marks(&mut self) {
        let limit = self.now - self.max_period as i64 - 2;
        if self.mark_count == 0 {
            self.last_mark = limit - self.unvoiced_step as i64;
        }
        loop {
            let Some(frame) = self.frame_near(self.last_mark + self.unvoiced_step as i64) else {
                return;
            };
            if frame.voiced {
                let period = frame.period;
                let candidate = self.last_mark + period.round() as i64;
                // Look for the waveform peak near where the next period should start. When we
                // were unvoiced before, search wider to lock on to the first peak.
                let reach = if self.last_voiced {
                    (period * 0.25) as i64
                } else {
                    (period * 0.5) as i64
                };
                if candidate + reach > limit {
                    return;
                }
                let lo = (candidate - reach).max(self.last_mark + (period * 0.5) as i64);
                let hi = candidate + reach;
                let mut best = candidate;
                let mut best_value = f32::MIN;
                for p in lo..=hi {
                    let v = self.peaks.get(p);
                    if v > best_value {
                        best_value = v;
                        best = p;
                    }
                }
                let step = (best - self.last_mark) as f32;
                self.smooth_period = if self.last_voiced && self.smooth_period > 0.0 {
                    let k = 1.0 - (-step / self.smooth_coeff_time).exp();
                    self.smooth_period + (period - self.smooth_period) * k
                } else {
                    period
                };
                self.push_mark(Mark {
                    pos: best,
                    period,
                    smooth_period: self.smooth_period,
                    voiced: true,
                });
                self.last_mark = best;
                self.last_voiced = true;
            } else {
                let next = self.last_mark + self.unvoiced_step as i64;
                if next > limit {
                    return;
                }
                self.push_mark(Mark {
                    pos: next,
                    period: self.unvoiced_step,
                    smooth_period: self.smooth_period,
                    voiced: false,
                });
                self.last_mark = next;
                self.last_voiced = false;
            }
        }
    }

    fn update_sung_note(&mut self, frame: &PitchFrame) {
        if !frame.voiced {
            self.sung = -1.0;
            return;
        }
        let note = freq_to_note(self.sample_rate / frame.period);
        self.sung = note;

        let key = self.settings.key;
        let steps = self.settings.scale.steps();
        if let Some(current) = self.snapped {
            // Hysteresis so the harmony doesn't flicker between two notes
            if (note - current as f32).abs() < 0.7
                && steps.contains(&(current - key).rem_euclid(12))
            {
                return;
            }
        }
        let rounded = note.round() as i32;
        let candidates = [rounded, rounded - 1, rounded + 1, rounded - 2, rounded + 2];
        self.snapped = candidates
            .iter()
            .copied()
            .filter(|n| steps.contains(&(n - key).rem_euclid(12)))
            .min_by(|a, b| {
                (*a as f32 - note)
                    .abs()
                    .total_cmp(&(*b as f32 - note).abs())
            });
    }

    /// The note `interval` scale steps away from `base` (which must be in the scale).
    fn scale_offset(&self, base: i32, interval: i32) -> i32 {
        let key = self.settings.key;
        let steps = self.settings.scale.steps();
        let rel = base - key;
        let octave = rel.div_euclid(12);
        let degree = steps
            .iter()
            .position(|s| *s == rel.rem_euclid(12))
            .unwrap_or(0) as i32;
        let target = degree + interval;
        key + (octave + target.div_euclid(7)) * 12 + steps[target.rem_euclid(7) as usize]
    }

    /// Process one stereo input sample and return the stereo output.
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        self.now += 1;
        let mono = (in_l + in_r) * 0.5;
        self.input.write(self.now, mono);
        let stage = self.peak_filters[0].lowpass(mono);
        let peak = self.peak_filters[1].lowpass(stage);
        self.peaks.write(self.now, peak);
        self.dry[0].write(self.now, in_l);
        self.dry[1].write(self.now, in_r);

        if let Some(frame) = self.detector.push(mono) {
            self.push_frame(frame);
            self.update_sung_note(&frame);
        }
        self.generate_marks();

        let t_out = self.now - self.latency as i64;
        let t = t_out as f64;
        let formant_rate = 2f32.powf(self.settings.formant / 12.0);
        let tune = self.settings.tune.clamp(0.0, 1.0);
        let spread = self.settings.spread;
        let mode = self.settings.mode;

        let mut out_l = 0.0;
        let mut out_r = 0.0;
        let mut total_env = 0.0;

        for vi in 0..MAX_VOICES {
            // Update the target note for auto voices
            if mode == Mode::Auto && self.voices[vi].in_use {
                if let Some(base) = self.snapped {
                    let interval = self.voices[vi].interval;
                    self.voices[vi].target = self.scale_offset(base, interval) as f32;
                    if self.voices[vi].is_silent() {
                        self.voices[vi].note = self.voices[vi].target;
                    }
                }
            }

            let voice = &mut self.voices[vi];
            let coeff = if voice.env_target > voice.env {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            voice.env += (voice.env_target - voice.env) * coeff;
            if voice.is_silent() {
                if voice.grains.iter().any(|g| g.alive) {
                    for g in &mut voice.grains {
                        g.alive = false;
                    }
                }
                voice.next_s = t;
                continue;
            }
            voice.note += (voice.target - voice.note) * self.glide_coeff;
            total_env += voice.env;

            // Spawn grains that are due
            let mut guard = 0;
            while t >= voice.next_s - voice.est_half && guard < 8 {
                guard += 1;
                let next_s = voice.next_s;
                let mark = nearest_mark(&self.marks, self.mark_head, self.mark_count, next_s)
                    .filter(|m| (m.pos as f64 - next_s).abs() <= self.max_period as f64);

                let Some(mark) = mark else {
                    voice.next_s += self.unvoiced_step as f64;
                    voice.est_half = self.unvoiced_step as f64;
                    continue;
                };

                let rate = formant_rate;
                if let Some(slot) = voice.grains.iter_mut().find(|g| !g.alive) {
                    *slot = Grain {
                        alive: true,
                        in_center: mark.pos as f64,
                        out_center: next_s,
                        half: mark.period,
                        rate,
                    };
                }

                let step = if mark.voiced {
                    let f0 = self.sample_rate / mark.period;
                    let f0_smooth = self.sample_rate / mark.smooth_period.max(1.0);
                    let wobble = (f0 / f0_smooth).powf(1.0 - tune);
                    let have_target = mode == Mode::Keys || self.snapped.is_some();
                    let target = if have_target {
                        note_to_freq(voice.note + voice.detune * spread * 0.12) * wobble
                    } else {
                        f0
                    };
                    let ratio = (target / f0).clamp(0.25, 4.0);
                    (mark.period / ratio) as f64
                } else {
                    mark.period as f64
                };
                voice.next_s += step.max(8.0);
                voice.est_half = (mark.period / rate) as f64;
            }

            // Overlap-add the active grains
            let mut acc = 0.0;
            let mut wsum = 0.0;
            for g in voice.grains.iter_mut().filter(|g| g.alive) {
                let u = ((t - g.out_center) * g.rate as f64) as f32;
                if u >= g.half {
                    g.alive = false;
                    continue;
                }
                if u <= -g.half {
                    continue;
                }
                let w = self.window.at(u / g.half);
                let pos = (g.in_center + u as f64).min(self.now as f64);
                acc += self.input.get_linear(pos) * w;
                wsum += w;
            }
            let sample = acc / wsum.max(1.0) * voice.env * voice.velocity;

            let angle = (voice.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
            out_l += sample * angle.cos();
            out_r += sample * angle.sin();
        }

        // Keep the overall harmony level steady regardless of how many voices are playing
        let target_norm = 1.0 / total_env.max(1.0).sqrt();
        self.norm += (target_norm - self.norm) * 0.001;

        let harmony = self.settings.harmony * self.norm * std::f32::consts::SQRT_2;
        let dry_l = self.dry[0].get(t_out);
        let dry_r = self.dry[1].get(t_out);
        (
            olo_dsp::sanitize(dry_l * self.settings.dry + out_l * harmony),
            olo_dsp::sanitize(dry_r * self.settings.dry + out_r * harmony),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::PitchDetector;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn voice_like(f0: impl Fn(usize) -> f32, n: usize) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                phase = (phase + f0(i) / SR).fract();
                let mut s = 0.0;
                for h in 1..14 {
                    let f = f0(i) * h as f32;
                    if f > SR * 0.45 {
                        break;
                    }
                    let formant = 1.0 / (1.0 + ((f - 700.0) / 400.0).powi(2))
                        + 0.5 / (1.0 + ((f - 1800.0) / 500.0).powi(2));
                    s += (TAU * phase * h as f32).sin() * (0.2 + formant) / h as f32;
                }
                s * 0.25
            })
            .collect()
    }

    /// Median detected frequency of a signal, ignoring unvoiced frames.
    fn median_pitch(signal: &[f32]) -> f32 {
        let mut d = PitchDetector::new(SR);
        let mut freqs: Vec<f32> = signal
            .iter()
            .filter_map(|x| d.push(*x))
            .filter(|f| f.voiced)
            .map(|f| SR / f.period)
            .collect();
        assert!(!freqs.is_empty(), "no voiced frames");
        freqs.sort_by(|a, b| a.total_cmp(b));
        freqs[freqs.len() / 2]
    }

    fn render(engine: &mut PrismEngine, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut l = Vec::with_capacity(input.len());
        let mut r = Vec::with_capacity(input.len());
        for x in input {
            let (a, b) = engine.process(*x, *x);
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    fn harmony_only() -> Settings {
        Settings {
            dry: 0.0,
            harmony: 1.0,
            spread: 0.0,
            ..Settings::default()
        }
    }

    #[test]
    fn midi_note_sets_harmony_pitch() {
        let input = voice_like(|_| 220.0, SR as usize * 2);
        for (note, expected) in [
            (61u8, 277.18f32),
            (57, 220.0),
            (45, 110.0),
            (69, 440.0),
            (64, 329.63),
        ] {
            let mut engine = PrismEngine::new(SR);
            engine.set(&harmony_only());
            engine.note_on(note, 1.0);
            let (l, _) = render(&mut engine, &input);
            let got = median_pitch(&l[SR as usize / 2..]);
            let cents = 1200.0 * (got / expected).log2();
            assert!(
                cents.abs() < 15.0,
                "note {note}: expected {expected} Hz, got {got} Hz"
            );
        }
    }

    #[test]
    fn auto_mode_adds_a_third_in_key() {
        // Singing E4 in C major: a third above is G4
        let input = voice_like(|_| 329.63, SR as usize * 2);
        let mut engine = PrismEngine::new(SR);
        engine.set(&Settings {
            mode: Mode::Auto,
            voicing: Voicing::ThirdUp,
            ..harmony_only()
        });
        let (l, _) = render(&mut engine, &input);
        let got = median_pitch(&l[SR as usize..]);
        let cents = 1200.0 * (got / 392.0).log2();
        assert!(cents.abs() < 15.0, "expected G4, got {got} Hz");
        assert_eq!(engine.snapped, Some(64));
    }

    #[test]
    fn scale_offsets_are_musical() {
        let engine = PrismEngine::new(SR);
        // C major: E4 + third = G4, E4 - third = C4, B4 + third = D5, C4 - octave = C3
        assert_eq!(engine.scale_offset(64, 2), 67);
        assert_eq!(engine.scale_offset(64, -2), 60);
        assert_eq!(engine.scale_offset(71, 2), 74);
        assert_eq!(engine.scale_offset(60, -7), 48);
    }

    #[test]
    fn dry_path_is_delayed_by_the_reported_latency() {
        let mut engine = PrismEngine::new(SR);
        engine.set(&Settings {
            dry: 1.0,
            harmony: 0.0,
            ..Settings::default()
        });
        let latency = engine.latency();
        let mut out = Vec::new();
        for i in 0..latency + 10 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            out.push(engine.process(x, x).0);
        }
        let peak = out.iter().position(|x| *x > 0.5).unwrap();
        assert_eq!(peak, latency);
        // Keep latency reasonable for a vocal effect
        assert!(latency as f32 / SR < 0.06, "latency {latency}");
    }

    #[test]
    fn noise_and_silence_stay_finite_and_bounded() {
        let mut engine = PrismEngine::new(SR);
        engine.set(&Settings {
            formant: 6.0,
            ..Settings::default()
        });
        for n in [48, 52, 55, 60, 64, 67] {
            engine.note_on(n, 1.0);
        }
        let mut rng = Rng::new(2);
        let mut peak = 0.0f32;
        for i in 0..(SR as usize * 3) {
            let x = if i < SR as usize {
                rng.bipolar() * 0.7
            } else if i < 2 * SR as usize {
                0.0
            } else {
                (i as f32 * 0.03).sin() * 0.7
            };
            let (l, r) = engine.process(x, x);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak < 3.0, "peak {peak}");
    }

    #[test]
    fn harmony_level_is_close_to_input_level() {
        let input = voice_like(|_| 220.0, SR as usize * 2);
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let mut engine = PrismEngine::new(SR);
        engine.set(&harmony_only());
        engine.note_on(64, 1.0);
        let (l, r) = render(&mut engine, &input);
        let out: Vec<f32> = l.iter().zip(&r).map(|(a, b)| (a + b) * 0.5).collect();
        let ratio_db = 20.0 * (rms(&out[SR as usize..]) / rms(&input[SR as usize..])).log10();
        assert!(ratio_db.abs() < 6.0, "harmony level {ratio_db} dB");
    }

    #[test]
    fn natural_tuning_keeps_vibrato() {
        // 5.5 Hz vibrato of +-30 cents around A3
        let vibrato = |i: usize| 220.0 * 2f32.powf(0.3 / 12.0 * (TAU * 5.5 * i as f32 / SR).sin());
        let input = voice_like(vibrato, SR as usize * 2);
        let spread_of = |tune: f32| {
            let mut engine = PrismEngine::new(SR);
            engine.set(&Settings {
                tune,
                ..harmony_only()
            });
            engine.note_on(64, 1.0);
            let (l, _) = render(&mut engine, &input);
            let mut d = PitchDetector::new(SR);
            let notes: Vec<f32> = l[SR as usize / 2..]
                .iter()
                .filter_map(|x| d.push(*x))
                .filter(|f| f.voiced)
                .map(|f| freq_to_note(SR / f.period))
                .collect();
            let max = notes.iter().cloned().fold(f32::MIN, f32::max);
            let min = notes.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        let robotic = spread_of(1.0);
        let natural = spread_of(0.0);
        assert!(
            natural > robotic + 0.25,
            "natural {natural} vs robotic {robotic}"
        );
    }
}
