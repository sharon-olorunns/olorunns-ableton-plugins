//! The reverb engine behind Halo (and the reverb in Rig). This is plain Rust with no
//! plugin-framework types so it can be tested and rendered offline.
//!
//! Signal flow (wet path):
//!
//! ```text
//! in -> pre-delay -> low cut -> 8-channel diffuser (4 steps) -> 8-line feedback delay network
//!                                                                 |  ^
//!                                         damping, decay, mixing  |  |  shimmer (pitch shifted
//!                                                                 v  |  output fed back in)
//!                                                                 out -> width -> ducking -> mix
//! ```
//!
//! The design follows the multichannel diffuser + Householder FDN approach popularised by
//! Geraint Luff (Signalsmith Audio): it gets very dense, smooth tails cheaply.

use crate::filter::{Biquad, BiquadType, OnePole};
use crate::pitch::GrainShifter;
use crate::smooth::{EnvelopeFollower, Smoother};
use crate::{soft_clip, DelayLine, Rng};

const CHANNELS: usize = 8;
const DIFFUSION_STEPS: usize = 4;
/// Longest diffuser step at the largest size, in milliseconds. Later steps halve this.
const MAX_DIFFUSION_MS: f32 = 48.0;
/// Shortest and longest base FDN delay as `size` goes from 0 to 1.
const MIN_ROOM_MS: f32 = 22.0;
const MAX_ROOM_MS: f32 = 150.0;
const MAX_MOD_MS: f32 = 3.0;
const MAX_PREDELAY_MS: f32 = 250.0;
/// Coefficient updates happen every this many samples.
const CONTROL_RATE: u32 = 32;

/// User-facing settings for the engine. Values are targets; the engine smooths them itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Room size, 0..1.
    pub size: f32,
    /// Decay time (RT60) in seconds.
    pub decay: f32,
    pub predelay_ms: f32,
    /// 0 = dark, 1 = bright.
    pub tone: f32,
    /// Shimmer feedback amount, 0..1.
    pub shimmer: f32,
    /// Shimmer pitch interval in semitones.
    pub interval: f32,
    /// Modulation depth, 0..1.
    pub motion: f32,
    /// Low cut frequency for the reverb input, Hz.
    pub low_cut: f32,
    /// Stereo width, 0..2 (1 = natural).
    pub width: f32,
    /// How much the reverb ducks while the input is playing, 0..1.
    pub duck: f32,
    /// Dry/wet mix, 0..1.
    pub mix: f32,
    pub freeze: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: 0.7,
            decay: 4.0,
            predelay_ms: 20.0,
            tone: 0.55,
            shimmer: 0.0,
            interval: 12.0,
            motion: 0.35,
            low_cut: 120.0,
            width: 1.0,
            duck: 0.0,
            mix: 0.35,
            freeze: false,
        }
    }
}

/// Transparent below +-2, then softly limits to +-4. A safety net against runaway feedback that
/// doesn't colour normal signals as they circulate through the loop.
#[inline]
fn safety_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 2.0 {
        x
    } else {
        (2.0 + 2.0 * soft_clip((a - 2.0) * 0.5)).copysign(x)
    }
}

/// Fast in-place Walsh-Hadamard transform for 8 channels, normalised to preserve energy.
#[inline]
fn hadamard8(x: &mut [f32; CHANNELS]) {
    let mut h = 1;
    while h < CHANNELS {
        let mut i = 0;
        while i < CHANNELS {
            for j in i..i + h {
                let a = x[j];
                let b = x[j + h];
                x[j] = a + b;
                x[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    let scale = (1.0 / CHANNELS as f32).sqrt();
    for v in x.iter_mut() {
        *v *= scale;
    }
}

/// Householder reflection: an energy-preserving mix where every channel feeds every other.
#[inline]
fn householder8(x: &mut [f32; CHANNELS]) {
    let sum: f32 = x.iter().sum();
    let k = sum * (2.0 / CHANNELS as f32);
    for v in x.iter_mut() {
        *v -= k;
    }
}

struct DiffuserStep {
    lines: [DelayLine; CHANNELS],
    /// Delay per channel in samples at the largest size.
    delays: [f32; CHANNELS],
    flip: [bool; CHANNELS],
    shuffle: [usize; CHANNELS],
}

impl DiffuserStep {
    fn new(max_ms: f32, sample_rate: f32, rng: &mut Rng) -> Self {
        let max_samples = max_ms * 0.001 * sample_rate;
        let mut delays = [0.0; CHANNELS];
        let mut flip = [false; CHANNELS];
        for i in 0..CHANNELS {
            // Stratified random delays so the channels cover the whole range evenly
            let lo = max_samples * i as f32 / CHANNELS as f32;
            let hi = max_samples * (i + 1) as f32 / CHANNELS as f32;
            delays[i] = (lo + (hi - lo) * rng.next_f32()).max(2.0);
            flip[i] = rng.next_u32() & 1 == 1;
        }
        // A fixed random permutation
        let mut shuffle = [0, 1, 2, 3, 4, 5, 6, 7];
        for i in (1..CHANNELS).rev() {
            let j = (rng.next_u32() as usize) % (i + 1);
            shuffle.swap(i, j);
        }
        Self {
            lines: std::array::from_fn(|_| DelayLine::new(max_samples as usize + 4)),
            delays,
            flip,
            shuffle,
        }
    }

    #[inline]
    fn process(&mut self, x: &mut [f32; CHANNELS], scale: f32) {
        let mut out = [0.0; CHANNELS];
        for i in 0..CHANNELS {
            self.lines[i].push(x[i]);
            let v = self.lines[i].read_linear(self.delays[i] * scale);
            out[self.shuffle[i]] = if self.flip[i] { -v } else { v };
        }
        hadamard8(&mut out);
        *x = out;
    }

    fn clear(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
    }
}

pub struct Reverb {
    sample_rate: f32,
    settings: Settings,

    predelay: [DelayLine; 2],
    low_cut: [Biquad; 2],
    diffusers: Vec<DiffuserStep>,

    lines: [DelayLine; CHANNELS],
    /// Relative length of each FDN line, between 1 and 2.
    line_ratio: [f32; CHANNELS],
    line_gain: [f32; CHANNELS],
    damping: [OnePole; CHANNELS],
    rumble: [OnePole; CHANNELS],
    lfo_phase: [f32; CHANNELS],
    lfo_rate: [f32; CHANNELS],

    shifters: [GrainShifter; 2],
    shimmer_hp: [OnePole; 2],
    shimmer_lp: [OnePole; 2],

    duck_follower: EnvelopeFollower,

    size: Smoother,
    predelay_ms: Smoother,
    shimmer: Smoother,
    motion: Smoother,
    width: Smoother,
    mix: Smoother,
    freeze: Smoother,
    duck: Smoother,

    control_counter: u32,
    damping_mix: f32,
    /// Smoothed wet output level for the GUI.
    wet_level: f32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let mut rng = Rng::new(0x4a10);
        let diffusers = (0..DIFFUSION_STEPS)
            .map(|step| {
                DiffuserStep::new(MAX_DIFFUSION_MS / (1 << step) as f32, sample_rate, &mut rng)
            })
            .collect();

        let max_line = ((MAX_ROOM_MS * 2.0 + MAX_MOD_MS * 2.0) * 0.001 * sample_rate) as usize + 8;
        let line_ratio = std::array::from_fn(|i| {
            let jitter = 1.0 + 0.04 * rng.bipolar();
            (2f32.powf(i as f32 / CHANNELS as f32) * jitter).clamp(1.0, 2.0)
        });
        let lfo_rate = std::array::from_fn(|i| 0.13 + 0.11 * i as f32 + 0.05 * rng.next_f32());
        let lfo_phase = std::array::from_fn(|_| rng.next_f32() * std::f32::consts::TAU);

        let predelay_len = (MAX_PREDELAY_MS * 0.001 * sample_rate) as usize + 8;

        let make_smoother = |ms: f32, value: f32| {
            let mut s = Smoother::new(value);
            s.set_time(ms, sample_rate);
            s
        };
        let defaults = Settings::default();

        let mut engine = Self {
            sample_rate,
            settings: defaults,
            predelay: [DelayLine::new(predelay_len), DelayLine::new(predelay_len)],
            low_cut: [Biquad::default(); 2],
            diffusers,
            lines: std::array::from_fn(|_| DelayLine::new(max_line)),
            line_ratio,
            line_gain: [0.0; CHANNELS],
            damping: [OnePole::default(); CHANNELS],
            rumble: [OnePole::default(); CHANNELS],
            lfo_phase,
            lfo_rate,
            shifters: [
                GrainShifter::new(83.0, sample_rate),
                GrainShifter::new(97.0, sample_rate),
            ],
            shimmer_hp: [OnePole::default(); 2],
            shimmer_lp: [OnePole::default(); 2],
            duck_follower: EnvelopeFollower::new(5.0, 300.0, sample_rate),
            size: make_smoother(400.0, defaults.size),
            predelay_ms: make_smoother(200.0, defaults.predelay_ms),
            shimmer: make_smoother(50.0, 0.0),
            motion: make_smoother(100.0, defaults.motion),
            width: make_smoother(50.0, defaults.width),
            mix: make_smoother(30.0, defaults.mix),
            freeze: make_smoother(80.0, 0.0),
            duck: make_smoother(50.0, 0.0),
            control_counter: 0,
            damping_mix: 0.0,
            wet_level: 0.0,
        };
        for i in 0..2 {
            engine.shimmer_hp[i].set_cutoff(250.0, sample_rate);
            engine.shimmer_lp[i].set_cutoff(6500.0, sample_rate);
        }
        engine.apply_interval();
        engine.apply_low_cut();
        engine.update_coefficients();
        engine
    }

    pub fn reset(&mut self) {
        for line in self.predelay.iter_mut().chain(self.lines.iter_mut()) {
            line.clear();
        }
        for d in &mut self.diffusers {
            d.clear();
        }
        for f in &mut self.low_cut {
            f.reset();
        }
        for f in self.damping.iter_mut().chain(self.rumble.iter_mut()) {
            f.reset();
        }
        for i in 0..2 {
            self.shifters[i].reset();
            self.shimmer_hp[i].reset();
            self.shimmer_lp[i].reset();
        }
        self.duck_follower.reset();
        self.wet_level = 0.0;
    }

    pub fn set(&mut self, settings: &Settings) {
        let interval_changed = settings.interval != self.settings.interval;
        let low_cut_changed = settings.low_cut != self.settings.low_cut;
        self.settings = *settings;

        self.size.set_target(settings.size.clamp(0.0, 1.0));
        self.predelay_ms
            .set_target(settings.predelay_ms.clamp(0.0, MAX_PREDELAY_MS));
        self.shimmer.set_target(settings.shimmer.clamp(0.0, 1.0));
        self.motion.set_target(settings.motion.clamp(0.0, 1.0));
        self.width.set_target(settings.width.clamp(0.0, 2.0));
        self.mix.set_target(settings.mix.clamp(0.0, 1.0));
        self.freeze
            .set_target(if settings.freeze { 1.0 } else { 0.0 });
        self.duck.set_target(settings.duck.clamp(0.0, 1.0));

        if interval_changed {
            self.apply_interval();
        }
        if low_cut_changed {
            self.apply_low_cut();
        }
    }

    fn apply_interval(&mut self) {
        for s in &mut self.shifters {
            s.set_semitones(self.settings.interval);
        }
    }

    fn apply_low_cut(&mut self) {
        for f in &mut self.low_cut {
            f.set(
                BiquadType::Highpass,
                self.settings.low_cut,
                0.6,
                self.sample_rate,
            );
        }
    }

    /// The smoothed output level of the wet signal, for metering.
    pub fn wet_level(&self) -> f32 {
        self.wet_level
    }

    fn room_samples(&self) -> f32 {
        let size = self.size.value();
        // Perceptually even: interpolate exponentially between the smallest and largest room
        MIN_ROOM_MS * (MAX_ROOM_MS / MIN_ROOM_MS).powf(size) * 0.001 * self.sample_rate
    }

    fn update_coefficients(&mut self) {
        let room = self.room_samples();
        let decay = self.settings.decay.max(0.1);
        let freeze = self.freeze.value();
        for i in 0..CHANNELS {
            let delay_seconds = room * self.line_ratio[i] / self.sample_rate;
            let g = 10f32.powf(-3.0 * delay_seconds / decay);
            // Frozen tails sustain (almost) forever
            self.line_gain[i] = g + (0.99995 - g) * freeze;
        }

        // Tone: 0 -> 1.2 kHz damping (dark), 1 -> ~18 kHz (bright)
        let cutoff = 1200.0 * 2f32.powf(self.settings.tone.clamp(0.0, 1.0) * 3.9);
        for f in &mut self.damping {
            f.set_cutoff(cutoff, self.sample_rate);
        }
        // While frozen we bypass damping so the frozen sound keeps its colour, and the rumble
        // filter drops to a near-DC blocker so low notes keep sustaining
        self.damping_mix = 1.0 - freeze;
        let rumble_cutoff = 18.0 + (2.0 - 18.0) * freeze;
        for f in &mut self.rumble {
            f.set_cutoff(rumble_cutoff, self.sample_rate);
        }
    }

    /// Process one stereo sample.
    #[inline]
    #[allow(clippy::needless_range_loop)] // indexes several per-channel arrays in lockstep
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        if self.control_counter == 0 {
            self.update_coefficients();
        }
        self.control_counter = (self.control_counter + 1) % CONTROL_RATE;

        let size = self.size.tick();
        let predelay_ms = self.predelay_ms.tick();
        let shimmer = self.shimmer.tick();
        let motion = self.motion.tick();
        let width = self.width.tick();
        let mix = self.mix.tick();
        let freeze = self.freeze.tick();
        let duck = self.duck.tick();

        // Pre-delay and low cut, wet path only
        let predelay = (predelay_ms * 0.001 * self.sample_rate).max(1.0);
        self.predelay[0].push(in_l);
        self.predelay[1].push(in_r);
        let wet_in_l = self.low_cut[0].process(self.predelay[0].read_linear(predelay));
        let wet_in_r = self.low_cut[1].process(self.predelay[1].read_linear(predelay));

        // Spread to 8 channels and diffuse
        let input_gain = 1.0 - freeze;
        let mut x: [f32; CHANNELS] = std::array::from_fn(|i| {
            if i % 2 == 0 {
                wet_in_l * input_gain
            } else {
                wet_in_r * input_gain
            }
        });
        let diffusion_scale = 0.35 + 0.65 * size;
        for step in &mut self.diffusers {
            step.process(&mut x, diffusion_scale);
        }

        // Feedback delay network
        let room = self.room_samples();
        let mod_depth = motion * MAX_MOD_MS * 0.001 * self.sample_rate;
        let lfo_inc = std::f32::consts::TAU / self.sample_rate;
        let mut y = [0.0; CHANNELS];
        for i in 0..CHANNELS {
            self.lfo_phase[i] += self.lfo_rate[i] * lfo_inc;
            if self.lfo_phase[i] > std::f32::consts::TAU {
                self.lfo_phase[i] -= std::f32::consts::TAU;
            }
            let base = room * self.line_ratio[i];
            let moving = base + mod_depth * (1.0 + self.lfo_phase[i].sin());
            // While frozen, glide to a fixed whole-sample delay: interpolating fractional delays
            // would slowly dull the frozen sound on every trip around the loop
            let delay = moving + (base.round() - moving) * freeze;
            y[i] = self.lines[i].read_cubic(delay.max(2.0));
        }

        // Tap the output before the feedback processing
        let mut wet_l = (y[0] + y[2] + y[4] + y[6]) * 0.5 + (x[0] + x[2]) * 0.15;
        let mut wet_r = (y[1] + y[3] + y[5] + y[7]) * 0.5 + (x[1] + x[3]) * 0.15;

        let mut fb = y;
        for i in 0..CHANNELS {
            let damped = self.damping[i].lowpass(fb[i]);
            fb[i] = damped * self.damping_mix + fb[i] * (1.0 - self.damping_mix);
            fb[i] = self.rumble[i].highpass(fb[i]);
        }
        householder8(&mut fb);

        // Shimmer: pitch shift the output and feed it back in. The band-limiting makes the octave
        // cascade die out naturally, and it's disabled while frozen to keep the loop stable.
        let shimmer_gain = shimmer * shimmer * 0.55 * (1.0 - freeze);
        let mut shimmer_in = [0.0; 2];
        if shimmer_gain > 1e-5 {
            for (c, wet) in [wet_l, wet_r].into_iter().enumerate() {
                let shifted = self.shifters[c].process(wet);
                let band = self.shimmer_lp[c].lowpass(self.shimmer_hp[c].highpass(shifted));
                shimmer_in[c] = band * shimmer_gain;
            }
        } else {
            // Keep the shifters fed so turning shimmer up doesn't start from silence
            self.shifters[0].process(wet_l);
            self.shifters[1].process(wet_r);
        }

        for i in 0..CHANNELS {
            let feed = fb[i] * self.line_gain[i] + x[i] + shimmer_in[i % 2];
            self.lines[i].push(safety_limit(feed));
        }

        // Width
        let mid = (wet_l + wet_r) * 0.5;
        let side = (wet_l - wet_r) * 0.5 * width;
        wet_l = mid + side;
        wet_r = mid - side;

        // Ducking while the input is playing
        let env = self.duck_follower.process(in_l.abs().max(in_r.abs()));
        let duck_amount = duck * (env * 8.0).min(1.0);
        let duck_gain = 1.0 - 0.9 * duck_amount;
        wet_l *= duck_gain;
        wet_r *= duck_gain;

        let level = wet_l.abs().max(wet_r.abs());
        self.wet_level +=
            (level - self.wet_level) * if level > self.wet_level { 0.01 } else { 0.0005 };

        let (dry_gain, wet_gain) = crate::equal_power(mix);
        (
            crate::sanitize(in_l * dry_gain + wet_l * wet_gain),
            crate::sanitize(in_r * dry_gain + wet_r * wet_gain),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn noise_burst(rng: &mut Rng, i: usize, len: usize) -> f32 {
        if i < len {
            rng.bipolar() * 0.5
        } else {
            0.0
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn render(settings: Settings, seconds: f32, burst: usize) -> (Vec<f32>, Vec<f32>) {
        let mut engine = Reverb::new(SR);
        engine.set(&settings);
        let mut rng = Rng::new(7);
        let n = (seconds * SR) as usize;
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let x = noise_burst(&mut rng, i, burst);
            let (a, b) = engine.process(x, x);
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    #[test]
    fn hadamard_preserves_energy() {
        let mut x = [1.0, -2.0, 0.5, 3.0, 0.0, 1.5, -1.0, 2.0];
        let before: f32 = x.iter().map(|v| v * v).sum();
        hadamard8(&mut x);
        let after: f32 = x.iter().map(|v| v * v).sum();
        assert!((before - after).abs() < 1e-4);
        householder8(&mut x);
        let after2: f32 = x.iter().map(|v| v * v).sum();
        assert!((before - after2).abs() < 1e-4);
    }

    #[test]
    fn tail_decays_roughly_at_rt60() {
        let settings = Settings {
            decay: 2.0,
            mix: 1.0,
            predelay_ms: 0.0,
            ..Settings::default()
        };
        let (l, _) = render(settings, 4.0, 4800);
        let early = rms(&l[(0.3 * SR) as usize..(0.5 * SR) as usize]);
        let late = rms(&l[(2.3 * SR) as usize..(2.5 * SR) as usize]);
        let drop_db = 20.0 * (early / late).log10();
        // RT60 of 2 seconds means ~60 dB over 2 seconds. Damping makes it a bit faster.
        assert!(
            (50.0..80.0).contains(&drop_db),
            "decay over 2s was {drop_db} dB"
        );
    }

    #[test]
    fn freeze_sustains_and_stays_bounded() {
        let mut engine = Reverb::new(SR);
        let mut settings = Settings {
            decay: 1.0,
            mix: 1.0,
            ..Settings::default()
        };
        engine.set(&settings);
        let mut rng = Rng::new(3);
        // Feed half a second of noise, then freeze
        for _ in 0..(SR as usize / 2) {
            let x = rng.bipolar() * 0.5;
            engine.process(x, x);
        }
        settings.freeze = true;
        engine.set(&settings);
        let mut out = Vec::new();
        for i in 0..(SR as usize * 20) {
            // Input continues but must not get into the frozen loop
            let x = rng.bipolar() * 0.5;
            let (l, _) = engine.process(x * 0.0, x * 0.0);
            if i > SR as usize {
                out.push(l);
            }
        }
        let first = rms(&out[..SR as usize]);
        let last = rms(&out[out.len() - SR as usize..]);
        assert!(first > 0.01, "frozen tail should be audible, rms {first}");
        let change_db = 20.0 * (last / first).log10();
        assert!(change_db.abs() < 3.0, "frozen level drifted {change_db} dB");
        assert!(out.iter().all(|x| x.is_finite() && x.abs() < 4.0));
    }

    #[test]
    fn max_shimmer_long_decay_does_not_blow_up() {
        let settings = Settings {
            decay: 30.0,
            shimmer: 1.0,
            tone: 1.0,
            mix: 1.0,
            ..Settings::default()
        };
        let (l, r) = render(settings, 30.0, 48_000);
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak.is_finite() && peak < 3.0, "peak {peak}");
        // And it should still be ringing (not choked) after a while
        let late = rms(&l[(10.0 * SR) as usize..(11.0 * SR) as usize]);
        assert!(late > 1e-4, "late rms {late}");
    }

    #[test]
    fn dry_signal_passes_when_mix_is_zero() {
        let settings = Settings {
            mix: 0.0,
            ..Settings::default()
        };
        let mut engine = Reverb::new(SR);
        engine.set(&settings);
        // Let the mix smoother settle
        for _ in 0..4800 {
            engine.process(0.0, 0.0);
        }
        let (l, r) = engine.process(0.25, -0.25);
        assert!((l - 0.25).abs() < 1e-3 && (r + 0.25).abs() < 1e-3);
    }

    /// Prints wet levels for calibration: `cargo test -p halo -- --nocapture levels`
    #[test]
    fn levels_are_reasonable() {
        for decay in [0.5, 2.0, 8.0, 30.0] {
            for size in [0.0, 0.5, 1.0] {
                let settings = Settings {
                    decay,
                    size,
                    mix: 1.0,
                    predelay_ms: 0.0,
                    ..Settings::default()
                };
                // Steady noise input
                let mut engine = Reverb::new(SR);
                engine.set(&settings);
                let mut rng = Rng::new(11);
                let mut out = Vec::new();
                let mut inp = Vec::new();
                for i in 0..(SR as usize * 3) {
                    let x = rng.bipolar() * 0.3;
                    let (l, _) = engine.process(x, x);
                    if i > SR as usize * 2 {
                        out.push(l);
                        inp.push(x);
                    }
                }
                let gain_db = 20.0 * (rms(&out) / rms(&inp)).log10();
                println!("decay {decay:>5} size {size:.1}: steady-state wet gain {gain_db:+.1} dB");
                assert!(gain_db < 24.0, "wet gain too hot: {gain_db}");
                assert!(gain_db > -24.0, "wet gain too quiet: {gain_db}");
            }
        }
    }
}
