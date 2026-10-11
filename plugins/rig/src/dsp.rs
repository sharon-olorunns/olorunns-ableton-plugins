//! Rig's signal chain. Mono guitar in, stereo out:
//!
//! ```text
//! gate -> compressor -> swell -> preamp (4x oversampled) -> tone stack -> style EQ
//!      -> cabinet (or acoustic voicing) -> modulation -> echo -> reverb -> output
//! ```
//!
//! Each [`Style`] supplies the character of every stage; the knobs set how much of each you get.

use crate::voicing::{EchoTime, Knobs, ModKind, Shaper, Style, Voicing};
use olo_dsp::filter::{Biquad, BiquadType, OnePole};
use olo_dsp::oversample::Oversampler4x;
use olo_dsp::reverb::{Reverb, Settings as ReverbSettings};
use olo_dsp::smooth::{EnvelopeFollower, Smoother};
use olo_dsp::{db_to_gain, gain_to_db, soft_clip, DelayLine, Rng};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Level the drive normalisation is calibrated around (a typical guitar DI peak).
const REFERENCE_LEVEL: f32 = 0.2;
const MAX_ECHO_SECONDS: f32 = 2.0;
const MOD_BUFFER_MS: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub style: Style,
    pub knobs: Knobs,
    pub output_db: f32,
}

impl Settings {
    pub fn for_style(style: Style) -> Self {
        Self {
            style,
            knobs: style.voicing().defaults,
            output_db: 0.0,
        }
    }
}

/// The preamp's waveshaper, run at 4x the sample rate.
struct Stage {
    shaper: Shaper,
    gain: f32,
    interstage: OnePole,
}

impl Stage {
    #[inline]
    fn curve(shaper: Shaper, v: f32, interstage: Option<&mut OnePole>) -> f32 {
        match shaper {
            Shaper::Tape => soft_clip(v),
            Shaper::Tube { bias } => soft_clip(v + bias) - soft_clip(bias),
            Shaper::Clean => v / (1.0 + v * v).sqrt(),
            Shaper::TwoStage { bias } => {
                let first = soft_clip(v + bias) - soft_clip(bias);
                let tightened = match interstage {
                    Some(filter) => filter.highpass(first),
                    None => first,
                };
                let a = tightened * 3.0;
                let a2 = a * a;
                a / (1.0 + a2 * a2).sqrt().sqrt()
            }
        }
    }

    #[inline]
    fn shape(&mut self, x: f32) -> f32 {
        Self::curve(self.shaper, x * self.gain, Some(&mut self.interstage))
    }
}

/// Fades each new note in, like rolling the volume knob up after picking.
struct Swell {
    fast: EnvelopeFollower,
    slow: EnvelopeFollower,
    gain: f32,
    step: f32,
    holdoff: u32,
    holdoff_len: u32,
}

impl Swell {
    fn new(sample_rate: f32) -> Self {
        Self {
            fast: EnvelopeFollower::new(1.0, 25.0, sample_rate),
            slow: EnvelopeFollower::new(60.0, 400.0, sample_rate),
            gain: 1.0,
            step: 1.0,
            holdoff: 0,
            holdoff_len: (0.12 * sample_rate) as u32,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let fast = self.fast.process(x);
        let slow = self.slow.process(x);
        if self.holdoff > 0 {
            self.holdoff -= 1;
        } else if fast > 0.003 && fast > slow * 2.0 {
            // A new note: start the fade from silence
            self.gain = 0.0;
            self.holdoff = self.holdoff_len;
        }
        self.gain = (self.gain + self.step).min(1.0);
        self.gain * self.gain
    }
}

pub struct RigEngine {
    sample_rate: f32,
    settings: Settings,
    first_update: bool,

    gate_follower: EnvelopeFollower,
    gate_threshold: f32,
    gate_open: bool,
    gate_gain: f32,
    gate_attack: f32,
    gate_release: f32,

    comp_env: f32,
    comp_attack: f32,
    comp_release: f32,
    comp_threshold_db: f32,
    comp_ratio: f32,
    comp_makeup: f32,

    swell: Swell,

    input_highpass: Biquad,
    oversampler: Oversampler4x,
    stage: Stage,
    drive_gain: Smoother,
    norm: Smoother,

    tone: [Biquad; 3],
    voice: [Biquad; 4],
    voice_count: usize,
    cab: [Biquad; 7],
    cab_count: usize,

    mod_line: DelayLine,
    lfo_phase: f32,
    flutter_phase: f32,
    drift: [f32; 2],
    drift_target: [f32; 2],
    drift_counter: u32,
    mod_amount: Smoother,

    echo_lines: [DelayLine; 2],
    echo_filters: [OnePole; 2],
    echo_time: Smoother,
    echo_amount: Smoother,
    bpm: f64,

    reverb: Reverb,
    reverb_amount: Smoother,

    output_gain: Smoother,
    rng: Rng,

    input_peak: f32,
    output_peak: f32,
    peak_decay: f32,
}

impl RigEngine {
    pub fn new(sample_rate: f32) -> Self {
        let smoother = |ms: f32, value: f32| {
            let mut s = Smoother::new(value);
            s.set_time(ms, sample_rate);
            s
        };
        let coeff = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sample_rate)).exp();
        let mut stage = Stage {
            shaper: Shaper::Tape,
            gain: 1.0,
            interstage: OnePole::default(),
        };
        stage.interstage.set_cutoff(300.0, sample_rate * 4.0);

        let mut engine = Self {
            sample_rate,
            settings: Settings::for_style(Style::Indie),
            first_update: true,
            gate_follower: EnvelopeFollower::new(0.5, 50.0, sample_rate),
            gate_threshold: 0.0,
            gate_open: true,
            gate_gain: 1.0,
            gate_attack: coeff(1.0),
            gate_release: coeff(80.0),
            comp_env: 0.0,
            comp_attack: coeff(10.0),
            comp_release: coeff(120.0),
            comp_threshold_db: 0.0,
            comp_ratio: 1.0,
            comp_makeup: 1.0,
            swell: Swell::new(sample_rate),
            input_highpass: Biquad::default(),
            oversampler: Oversampler4x::new(),
            stage,
            drive_gain: smoother(30.0, 1.0),
            norm: smoother(30.0, 1.0),
            tone: [Biquad::default(); 3],
            voice: [Biquad::default(); 4],
            voice_count: 0,
            cab: [Biquad::default(); 7],
            cab_count: 0,
            mod_line: DelayLine::new((MOD_BUFFER_MS * 0.001 * sample_rate) as usize),
            lfo_phase: 0.0,
            flutter_phase: 0.0,
            drift: [0.0; 2],
            drift_target: [0.0; 2],
            drift_counter: 0,
            mod_amount: smoother(50.0, 0.0),
            echo_lines: [
                DelayLine::new((MAX_ECHO_SECONDS * sample_rate) as usize),
                DelayLine::new((MAX_ECHO_SECONDS * sample_rate) as usize),
            ],
            echo_filters: [OnePole::default(); 2],
            echo_time: smoother(250.0, 0.25 * sample_rate),
            echo_amount: smoother(50.0, 0.0),
            bpm: 120.0,
            reverb: Reverb::new(sample_rate),
            reverb_amount: smoother(50.0, 0.0),
            output_gain: smoother(30.0, 1.0),
            rng: Rng::new(0x7219),
            input_peak: 0.0,
            output_peak: 0.0,
            peak_decay: (-1.0 / (0.3 * sample_rate)).exp(),
        };
        let settings = engine.settings;
        engine.set(&settings);
        engine
    }

    /// Latency added by the oversampled preamp, in samples.
    pub fn latency() -> usize {
        Oversampler4x::latency()
    }

    pub fn reset(&mut self) {
        self.oversampler.reset();
        self.mod_line.clear();
        for line in &mut self.echo_lines {
            line.clear();
        }
        self.reverb.reset();
        for f in self
            .tone
            .iter_mut()
            .chain(self.voice.iter_mut())
            .chain(self.cab.iter_mut())
            .chain(std::iter::once(&mut self.input_highpass))
        {
            f.reset();
        }
        self.comp_env = 0.0;
        self.gate_gain = 1.0;
        self.gate_follower.reset();
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.bpm = if bpm.is_finite() && bpm > 20.0 {
            bpm
        } else {
            120.0
        };
    }

    pub fn input_peak(&self) -> f32 {
        self.input_peak
    }

    pub fn output_peak(&self) -> f32 {
        self.output_peak
    }

    fn voicing(&self) -> &'static Voicing {
        self.settings.style.voicing()
    }

    pub fn set(&mut self, settings: &Settings) {
        if !self.first_update && *settings == self.settings {
            return;
        }
        let style_changed = self.first_update || settings.style != self.settings.style;
        self.settings = *settings;
        self.first_update = false;
        let sr = self.sample_rate;
        let v = self.voicing();
        let k = settings.knobs;

        if style_changed {
            self.input_highpass
                .set(BiquadType::Highpass, v.input_highpass, 0.7, sr);
            self.stage.shaper = v.shaper;

            self.voice_count = v.voice_eq.len().min(self.voice.len());
            for (filter, (kind, freq, q)) in self.voice.iter_mut().zip(v.voice_eq) {
                filter.set(*kind, *freq, *q, sr);
            }

            self.cab_count = 0;
            if let Some(cab) = v.cab {
                let stages = [
                    (BiquadType::Highpass, cab.highpass, 0.7),
                    (
                        BiquadType::Peak(cab.resonance.1),
                        cab.resonance.0,
                        cab.resonance.2,
                    ),
                    (BiquadType::Peak(cab.dip.1), cab.dip.0, cab.dip.2),
                    (
                        BiquadType::Peak(cab.presence.1),
                        cab.presence.0,
                        cab.presence.2,
                    ),
                    // Three lowpasses make the steep top-end roll-off of a guitar speaker
                    (BiquadType::Lowpass, cab.lowpass, 0.6),
                    (BiquadType::Lowpass, cab.lowpass * 1.15, 0.9),
                    (BiquadType::Lowpass, cab.lowpass * 1.3, 0.7),
                ];
                for (filter, (kind, freq, q)) in self.cab.iter_mut().zip(stages) {
                    filter.set(kind, freq, q, sr);
                }
                self.cab_count = stages.len();
            }

            let coeff = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sr)).exp();
            self.comp_attack = coeff(v.comp_attack_ms);
            self.comp_release = coeff(v.comp_release_ms);

            for f in &mut self.echo_filters {
                f.set_cutoff(v.echo.lowpass, sr);
            }

            self.reverb.set(&ReverbSettings {
                size: v.space.size,
                decay: v.space.decay,
                predelay_ms: v.space.predelay,
                tone: v.space.tone,
                shimmer: v.space.shimmer,
                interval: 12.0,
                motion: v.space.motion,
                low_cut: v.space.low_cut,
                width: 1.0,
                duck: 0.0,
                mix: 1.0,
                freeze: false,
            });
        }

        // Tone stack: +-12 dB around the style's frequencies
        self.tone[0].set(
            BiquadType::LowShelf(k.bass.clamp(-1.0, 1.0) * 12.0),
            v.bass_freq,
            0.7,
            sr,
        );
        self.tone[1].set(
            BiquadType::Peak(k.mid.clamp(-1.0, 1.0) * 12.0),
            v.mid_freq,
            0.7,
            sr,
        );
        self.tone[2].set(
            BiquadType::HighShelf(k.treble.clamp(-1.0, 1.0) * 12.0),
            v.treble_freq,
            0.7,
            sr,
        );

        // Drive, with make-up so turning it up adds grit without getting much louder
        let drive = k.drive.clamp(0.0, 1.0);
        let gain = db_to_gain(v.drive_db.0 + (v.drive_db.1 - v.drive_db.0) * drive);
        let shaped = Stage::curve(v.shaper, REFERENCE_LEVEL * gain, None).abs();
        self.drive_gain.set_target(gain);
        self.norm.set_target(REFERENCE_LEVEL / shaped.max(1e-4));

        // Gate: off at 0, otherwise -85 dB to -30 dB
        self.gate_threshold = if k.gate > 0.001 {
            db_to_gain(-85.0 + 55.0 * k.gate.clamp(0.0, 1.0))
        } else {
            0.0
        };

        let comp = k.comp.clamp(0.0, 1.0);
        self.comp_threshold_db = -8.0 - 28.0 * comp;
        self.comp_ratio = 1.0 + 7.0 * comp;
        self.comp_makeup = db_to_gain(9.0 * comp);

        let swell = k.swell.clamp(0.0, 1.0);
        let swell_seconds = 0.03 + swell * swell * 1.5;
        self.swell.step = if swell > 0.001 {
            1.0 / (swell_seconds * sr)
        } else {
            1.0
        };

        self.mod_amount.set_target(k.modulation.clamp(0.0, 1.0));
        self.echo_amount.set_target(k.echo.clamp(0.0, 1.0));
        self.reverb_amount.set_target(k.reverb.clamp(0.0, 1.0));
        self.output_gain
            .set_target(db_to_gain(settings.output_db + v.level_db));
    }

    fn echo_samples(&self) -> f32 {
        let seconds = match self.voicing().echo.time {
            EchoTime::Beats(beats) => beats * 60.0 / self.bpm as f32,
            EchoTime::Ms(ms) => ms * 0.001,
        };
        (seconds * self.sample_rate).clamp(1.0, MAX_ECHO_SECONDS * self.sample_rate - 4.0)
    }

    #[inline]
    fn compress(&mut self, x: f32) -> f32 {
        if self.comp_ratio <= 1.001 {
            return x;
        }
        let level = x.abs();
        let coeff = if level > self.comp_env {
            self.comp_attack
        } else {
            self.comp_release
        };
        self.comp_env += (level - self.comp_env) * coeff;
        let over = gain_to_db(self.comp_env) - self.comp_threshold_db;
        let knee = 6.0;
        let slope = 1.0 / self.comp_ratio - 1.0;
        let reduction = if over <= -knee / 2.0 {
            0.0
        } else if over >= knee / 2.0 {
            over * slope
        } else {
            (over + knee / 2.0).powi(2) / (2.0 * knee) * slope
        };
        x * db_to_gain(reduction) * self.comp_makeup
    }

    #[inline]
    fn modulate(&mut self, x: f32) -> (f32, f32) {
        let amount = self.mod_amount.tick();
        self.mod_line.push(x);
        let ms = 0.001 * self.sample_rate;
        let v = self.voicing();

        // Slowly wandering random values for organic movement
        self.drift_counter += 1;
        if self.drift_counter >= (self.sample_rate * 0.25) as u32 {
            self.drift_counter = 0;
            self.drift_target = [self.rng.bipolar(), self.rng.bipolar()];
        }
        for i in 0..2 {
            self.drift[i] += (self.drift_target[i] - self.drift[i]) * 0.00005;
        }

        if amount < 1e-4 {
            return (x, x);
        }

        match v.modulation {
            ModKind::Chorus { rate, depth, base } => {
                self.lfo_phase = (self.lfo_phase + rate / self.sample_rate).fract();
                let a = TAU * self.lfo_phase;
                let left = (base + depth * 0.5 * (1.0 + a.sin())) * ms;
                let right = (base + depth * 0.5 * (1.0 + (a + FRAC_PI_2).sin())) * ms;
                let wet_l = self.mod_line.read_cubic(left);
                let wet_r = self.mod_line.read_cubic(right);
                let mix = amount * 0.6;
                (
                    x * (1.0 - mix * 0.5) + wet_l * mix,
                    x * (1.0 - mix * 0.5) + wet_r * mix,
                )
            }
            ModKind::Doubler => {
                let left = (11.0 + 0.5 * self.drift[0]) * ms;
                let right = (17.0 + 0.5 * self.drift[1]) * ms;
                let wet_l = self.mod_line.read_cubic(left);
                let wet_r = self.mod_line.read_cubic(right);
                let mix = amount * 0.5;
                (x * (1.0 - mix) + wet_l * mix, x * (1.0 - mix) + wet_r * mix)
            }
            ModKind::Warble => {
                // Wow (slow) and flutter (fast) pitch wobble, like a worn tape machine
                self.lfo_phase = (self.lfo_phase + 0.55 / self.sample_rate).fract();
                self.flutter_phase = (self.flutter_phase + 6.5 / self.sample_rate).fract();
                let wow = (TAU * self.lfo_phase).sin() * 0.7 + self.drift[0] * 0.3;
                let flutter = (TAU * self.flutter_phase).sin();
                let delay = (4.0 + amount * (2.0 * (1.0 + wow) + 0.12 * flutter)) * ms;
                let wet = self.mod_line.read_cubic(delay);
                // Fade from dry to fully wobbly over the first bit of the knob
                let blend = (amount * 8.0).min(1.0);
                let out = x * (1.0 - blend) + wet * blend;
                (out, out)
            }
        }
    }

    #[inline]
    fn echo(&mut self, l: f32, r: f32) -> (f32, f32) {
        let amount = self.echo_amount.tick();
        self.echo_time.set_target(self.echo_samples());
        let time = self.echo_time.tick();
        let v = self.voicing().echo;

        let out_l = self.echo_filters[0].lowpass(self.echo_lines[0].read_linear(time));
        let out_r = self.echo_filters[1].lowpass(self.echo_lines[1].read_linear(time));
        let fb = v.feedback;
        if v.ping_pong {
            self.echo_lines[0].push((l + r) * 0.5 + out_r * fb);
            self.echo_lines[1].push(out_l * fb);
        } else {
            self.echo_lines[0].push(l + out_l * fb);
            self.echo_lines[1].push(r + out_r * fb);
        }
        let mix = amount * 0.7;
        (l + out_l * mix, r + out_r * mix)
    }

    /// Process one sample. The guitar is treated as mono; the output is stereo.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let x = (in_l + in_r) * 0.5;
        self.input_peak = (self.input_peak * self.peak_decay).max(x.abs());

        // Noise gate
        let mut s = x;
        if self.gate_threshold > 0.0 {
            let env = self.gate_follower.process(x);
            if env > self.gate_threshold {
                self.gate_open = true;
            } else if env < self.gate_threshold * 0.5 {
                self.gate_open = false;
            }
            let target = if self.gate_open { 1.0 } else { 0.0 };
            let coeff = if target > self.gate_gain {
                self.gate_attack
            } else {
                self.gate_release
            };
            self.gate_gain += (target - self.gate_gain) * coeff;
            s *= self.gate_gain;
        }

        s = self.compress(s);
        if self.swell.step < 1.0 {
            s *= self.swell.process(s);
        }

        // Preamp
        s = self.input_highpass.process(s);
        self.stage.gain = self.drive_gain.tick();
        let norm = self.norm.tick();
        let stage = &mut self.stage;
        s = self.oversampler.process(s, |v| stage.shape(v)) * norm;

        for f in &mut self.tone {
            s = f.process(s);
        }
        for f in self.voice.iter_mut().take(self.voice_count) {
            s = f.process(s);
        }
        for f in self.cab.iter_mut().take(self.cab_count) {
            s = f.process(s);
        }

        let (l, r) = self.modulate(s);
        let (l, r) = self.echo(l, r);

        let amount = self.reverb_amount.tick();
        let (wl, wr) = self.reverb.process(l, r);
        let mix = amount * 0.9;
        let gain = self.output_gain.tick();
        let out_l = olo_dsp::sanitize((l + wl * mix) * gain);
        let out_r = olo_dsp::sanitize((r + wr * mix) * gain);
        self.output_peak = (self.output_peak * self.peak_decay).max(out_l.abs().max(out_r.abs()));
        (out_l, out_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A Karplus-Strong plucked string: close enough to a guitar DI for testing.
    fn pluck(freq: f32, seconds: f32, level: f32, seed: u32) -> Vec<f32> {
        let period = (SR / freq) as usize;
        let mut rng = Rng::new(seed);
        let mut buf: Vec<f32> = (0..period).map(|_| rng.bipolar() * level).collect();
        let n = (seconds * SR) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let a = buf[i % period];
            let b = buf[(i + 1) % period];
            buf[i % period] = 0.4985 * (a + b);
            out.push(a);
        }
        out
    }

    fn strum() -> Vec<f32> {
        let mut out = vec![0.0; (SR * 4.0) as usize];
        for (k, f) in [82.4, 110.0, 146.8, 196.0, 246.9, 329.6].iter().enumerate() {
            let p = pluck(*f, 3.5, 0.25, k as u32 + 1);
            for (i, v) in p.iter().enumerate() {
                out[i + k * 480] += v;
            }
        }
        out
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    fn render(settings: Settings, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut engine = RigEngine::new(SR);
        engine.set(&settings);
        let mut l = Vec::with_capacity(input.len());
        let mut r = Vec::with_capacity(input.len());
        for x in input {
            let (a, b) = engine.process(*x, *x);
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    fn dry(style: Style) -> Settings {
        let mut s = Settings::for_style(style);
        s.knobs.modulation = 0.0;
        s.knobs.echo = 0.0;
        s.knobs.reverb = 0.0;
        s.knobs.swell = 0.0;
        s.knobs.gate = 0.0;
        s
    }

    #[test]
    fn every_style_sounds_sensible_with_its_defaults() {
        let input = strum();
        let input_db = 20.0 * rms(&input[..SR as usize]).log10();
        for style in Style::ALL {
            let (l, r) = render(Settings::for_style(style), &input);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()));
            let peak = l.iter().chain(&r).fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak < 2.5, "{style:?} peak {peak}");
            let out_db = 20.0 * rms(&l[..SR as usize]).log10();
            println!("{style:?}: input {input_db:.1} dB, output {out_db:.1} dB, peak {peak:.2}");
            assert!(
                (out_db - input_db).abs() < 12.0,
                "{style:?} level changed by {} dB",
                out_db - input_db
            );
        }
    }

    #[test]
    fn more_drive_means_more_distortion() {
        let sine: Vec<f32> = (0..SR as usize)
            .map(|i| (TAU * 220.0 * i as f32 / SR).sin() * 0.2)
            .collect();
        // Energy at the 3rd harmonic relative to the fundamental
        let harmonic_ratio = |s: &[f32]| {
            let probe = |hz: f32| {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, v) in s.iter().enumerate().skip(4800) {
                    let p = (TAU * hz * i as f32 / SR) as f64;
                    re += *v as f64 * p.cos();
                    im += *v as f64 * p.sin();
                }
                (re * re + im * im).sqrt()
            };
            probe(660.0) / probe(220.0)
        };
        for style in [Style::Indie, Style::PopRock, Style::LoFi] {
            let mut clean = dry(style);
            clean.knobs.drive = 0.0;
            clean.knobs.comp = 0.0;
            let mut dirty = clean;
            dirty.knobs.drive = 1.0;
            let a = harmonic_ratio(&render(clean, &sine).0);
            let b = harmonic_ratio(&render(dirty, &sine).0);
            assert!(b > a * 2.0, "{style:?}: {a} -> {b}");
        }
    }

    #[test]
    fn cabinets_roll_off_fizz_but_acoustic_stays_open() {
        let tone = |hz: f32| -> Vec<f32> {
            (0..SR as usize)
                .map(|i| (TAU * hz * i as f32 / SR).sin() * 0.05)
                .collect()
        };
        let mut results = Vec::new();
        for style in Style::ALL {
            let mut s = dry(style);
            s.knobs.drive = 0.0;
            s.knobs.comp = 0.0;
            let low = rms(&render(s, &tone(500.0)).0[4800..]);
            let high = rms(&render(s, &tone(9000.0)).0[4800..]);
            let db = 20.0 * (high / low).log10();
            println!("{style:?}: 9 kHz is {db:.1} dB relative to 500 Hz");
            results.push((style, db));
        }
        for (style, db) in results {
            // Clean styles keep some sparkle, the dirtier ones need fizz tamed hard
            let limit = match style {
                Style::Acoustic => {
                    assert!(db > -6.0, "acoustic 9 kHz at {db} dB");
                    continue;
                }
                Style::Indie | Style::MathRock => -12.0,
                _ => -20.0,
            };
            assert!(db < limit, "{style:?} 9 kHz only {db} dB down");
        }
    }

    #[test]
    fn gate_silences_hum_between_notes() {
        let mut rng = Rng::new(3);
        let hum: Vec<f32> = (0..SR as usize).map(|_| rng.bipolar() * 0.0005).collect();
        let mut open = dry(Style::PopRock);
        open.knobs.gate = 0.0;
        let mut gated = open;
        gated.knobs.gate = 0.6;
        let a = rms(&render(open, &hum).0[24_000..]);
        let b = rms(&render(gated, &hum).0[24_000..]);
        assert!(b < a * 0.05, "gate only reduced noise from {a} to {b}");
    }

    #[test]
    fn swell_fades_notes_in() {
        let mut input = vec![0.0; 24_000];
        input.extend((0..48_000).map(|i| (TAU * 196.0 * i as f32 / SR).sin() * 0.2));
        let mut s = dry(Style::Atmospheric);
        s.knobs.swell = 0.6;
        let (l, _) = render(s, &input);
        let start = rms(&l[24_000..24_000 + 1440]);
        let later = rms(&l[24_000 + 36_000..24_000 + 38_000]);
        assert!(start < later * 0.3, "swell start {start} vs later {later}");
    }

    #[test]
    fn atmospheric_echo_is_a_dotted_eighth() {
        let mut s = dry(Style::Atmospheric);
        s.knobs.echo = 1.0;
        s.knobs.drive = 0.0;
        s.knobs.comp = 0.0;
        let mut engine = RigEngine::new(SR);
        engine.set_tempo(120.0);
        engine.set(&s);
        // Let the echo time smoother settle on the tempo before sending a click
        for _ in 0..48_000 {
            engine.process(0.0, 0.0);
        }
        let mut out = Vec::new();
        for i in 0..30_000 {
            let x = if i == 0 { 0.5 } else { 0.0 };
            out.push(engine.process(x, x).0);
        }
        // 0.75 beats at 120 bpm = 18000 samples, plus the preamp latency
        let window = &out[17_000..20_000];
        let pos = window
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0
            + 17_000;
        let expected = 18_000 + RigEngine::latency();
        assert!((pos as i64 - expected as i64).abs() < 150, "echo at {pos}");
    }

    #[test]
    fn lofi_warble_wobbles_the_pitch() {
        let sine: Vec<f32> = (0..(SR * 3.0) as usize)
            .map(|i| (TAU * 440.0 * i as f32 / SR).sin() * 0.1)
            .collect();
        let spread = |amount: f32| {
            let mut s = dry(Style::LoFi);
            s.knobs.modulation = amount;
            s.knobs.drive = 0.0;
            s.knobs.comp = 0.0;
            let (l, _) = render(s, &sine);
            // Measure frequency in 50 ms windows by counting zero crossings
            let freqs: Vec<f32> = l[SR as usize..]
                .chunks(2400)
                .map(|c| {
                    let crossings = c.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
                    crossings as f32 * SR / c.len() as f32
                })
                .collect();
            let max = freqs.iter().cloned().fold(f32::MIN, f32::max);
            let min = freqs.iter().cloned().fold(f32::MAX, f32::min);
            max - min
        };
        let steady = spread(0.0);
        let wobbly = spread(1.0);
        assert!(
            wobbly > steady + 6.0,
            "steady {steady} Hz vs wobbly {wobbly} Hz"
        );
    }

    #[test]
    fn switching_styles_while_playing_is_safe() {
        let input = strum();
        let mut engine = RigEngine::new(SR);
        for (i, x) in input.iter().enumerate() {
            if i % 20_000 == 0 {
                let style = Style::ALL[(i / 20_000) % Style::ALL.len()];
                let mut s = Settings::for_style(style);
                s.knobs.drive = 1.0;
                s.knobs.echo = 1.0;
                s.knobs.reverb = 1.0;
                s.knobs.modulation = 1.0;
                engine.set(&s);
            }
            let (l, r) = engine.process(*x, *x);
            assert!(l.is_finite() && r.is_finite() && l.abs() < 4.0 && r.abs() < 4.0);
        }
    }
}
