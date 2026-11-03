//! Closer's engine: a small, warm vocal space that adds body without pushing the voice back.
//!
//! - **Close and present:** the dry voice stays at full level and the reverb is added on top, so
//!   turning the reverb up never turns the vocal down. A short pre-delay lets consonants land dry
//!   before the room, and the dry voice ducks the tail so it only blooms between phrases.
//! - **Body, not echo:** four series all-passes smear the input into a dense cloud before an
//!   8-line Hadamard feedback delay network with short, mutually prime lines and slow modulation.
//!   There are no discrete early reflections, so no slap or flutter.
//! - **Warm, not washy:** the tail is high-passed (no mud), gets a low-mid "body" bell around
//!   350 Hz, and is damped inside the loop (the tail darkens as it decays) and after it.
//!
//! A small voice stage (body shelf, presence bell, air shelf, soft saturation and a stereo
//! spreader for backing vocals) sits on the dry path.

use olo_dsp::filter::BiquadType;
use olo_dsp::smooth::EnvelopeFollower;
use olo_dsp::{db_to_gain, Biquad, DelayLine, OnePole, Smoother};
use std::f32::consts::TAU;

const LINES: usize = 8;
/// FDN line lengths in ms at size scale 1.0.
const LINE_MS: [f32; LINES] = [29.7, 37.1, 41.1, 43.7, 53.3, 59.9, 67.1, 73.3];
/// Input diffuser lengths in ms.
const DIFFUSER_MS: [f32; 4] = [4.7, 3.6, 12.7, 9.3];
const MIN_SCALE: f32 = 0.25;
const MAX_SCALE: f32 = 1.25;
/// Mix 100% puts the tail at roughly the dry level.
const WET_MAKEUP: f32 = 5.0;
const MAX_PREDELAY_MS: f32 = 120.0;
/// Rates of the slow LFOs: two for the spreader, four for the FDN line modulation.
const LFO_HZ: [f32; 6] = [0.651, 0.527, 0.31, 0.350, 0.391, 0.431];

/// Everything the engine needs from the plugin's parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Reverb level added on top of the dry voice, 0..1.
    pub mix: f32,
    /// Room size, 0..1.
    pub size: f32,
    /// RT60 in seconds.
    pub decay: f32,
    pub predelay_ms: f32,
    /// Tail high cut in Hz.
    pub damping: f32,
    /// Tail high-pass in Hz.
    pub low_cut: f32,
    /// Low-mid bell on the tail, dB.
    pub body_db: f32,
    /// Stereo width of the tail, 0..1.
    pub width: f32,
    /// How much the dry voice turns the tail down, 0..1.
    pub duck: f32,
    /// Low shelf on the dry voice (180 Hz), dB.
    pub voice_body_db: f32,
    /// Presence bell on the dry voice (3.2 kHz), dB.
    pub presence_db: f32,
    /// Air shelf on the dry voice (11 kHz), dB.
    pub air_db: f32,
    /// Soft saturation, 0..1.
    pub warmth: f32,
    /// Stereo spreader for doubles and stacks, 0..1.
    pub spread: f32,
    /// Output the reverb only, for return tracks.
    pub wet_only: bool,
    pub output_db: f32,
}

impl Default for Settings {
    fn default() -> Self {
        crate::presets::Preset::LeadClosePlate.settings()
    }
}

#[derive(Debug, Clone, Default)]
struct Allpass {
    line: DelayLine,
    delay: usize,
    gain: f32,
}

impl Allpass {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.line.read(self.delay);
        let v = x - self.gain * delayed;
        self.line.push(v);
        delayed + self.gain * v
    }
}

/// Normalised 8-point fast Walsh-Hadamard transform, used as the FDN's mixing matrix.
#[inline]
fn hadamard(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    const NORM: f32 = 0.353_553_4; // 1 / sqrt(8)
    for s in v.iter_mut() {
        *s *= NORM;
    }
}

pub struct CloserEngine {
    sample_rate: f32,
    settings: Settings,
    configured: bool,

    predelay: DelayLine,
    spread_l: DelayLine,
    spread_r: DelayLine,
    lines: [DelayLine; LINES],
    diffusers: [Allpass; 4],
    loop_damping: [OnePole; LINES],
    line_len: [f32; LINES],
    feedback: [f32; LINES],

    wet_hp: Biquad,
    wet_body: [Biquad; 2],
    wet_lp: [Biquad; 2],
    wet_lp2: [Biquad; 2],
    voice_body: [Biquad; 2],
    voice_presence: [Biquad; 2],
    voice_air: [Biquad; 2],

    size_scale: f32,
    mod_depth: f32,
    predelay_samples: f32,
    drive: f32,
    warmth_comp: f32,

    mix: Smoother,
    width: Smoother,
    spread: Smoother,
    output: Smoother,
    predelay_smooth: Smoother,
    scale: Smoother,
    duck_gain: Smoother,
    follower: EnvelopeFollower,

    lfo_phase: [f32; 6],
    lfo_inc: [f32; 6],
    wet_level: f32,
    meter_release: f32,
}

impl CloserEngine {
    pub fn new(sample_rate: f32) -> Self {
        let ms = |m: f32| (m * 0.001 * sample_rate).ceil() as usize;
        let smoother = |time_ms: f32| {
            let mut s = Smoother::new(0.0);
            s.set_time(time_ms, sample_rate);
            s
        };

        let mut engine = Self {
            sample_rate,
            settings: Settings::default(),
            configured: false,

            predelay: DelayLine::new(ms(MAX_PREDELAY_MS + 10.0)),
            spread_l: DelayLine::new(ms(30.0)),
            spread_r: DelayLine::new(ms(30.0)),
            lines: std::array::from_fn(|i| DelayLine::new(ms(LINE_MS[i] * MAX_SCALE + 4.0))),
            diffusers: std::array::from_fn(|i| Allpass {
                line: DelayLine::new(ms(DIFFUSER_MS[i] * MAX_SCALE + 1.0)),
                delay: 1,
                gain: 0.7,
            }),
            loop_damping: [OnePole::default(); LINES],
            line_len: [1.0; LINES],
            feedback: [0.0; LINES],

            wet_hp: Biquad::default(),
            wet_body: [Biquad::default(); 2],
            wet_lp: [Biquad::default(); 2],
            wet_lp2: [Biquad::default(); 2],
            voice_body: [Biquad::default(); 2],
            voice_presence: [Biquad::default(); 2],
            voice_air: [Biquad::default(); 2],

            size_scale: 1.0,
            mod_depth: 0.0,
            predelay_samples: 0.0,
            drive: 1.0,
            warmth_comp: 1.0,

            mix: smoother(60.0),
            width: smoother(60.0),
            spread: smoother(60.0),
            output: smoother(60.0),
            predelay_smooth: smoother(60.0),
            scale: smoother(60.0),
            duck_gain: smoother(90.0),
            follower: EnvelopeFollower::new(4.0, 220.0, sample_rate),

            lfo_phase: [0.0, 1.3, 0.0, 1.7, 3.4, 5.1],
            lfo_inc: LFO_HZ.map(|hz| TAU * hz / sample_rate),
            wet_level: 0.0,
            meter_release: (-1.0 / (0.3 * sample_rate)).exp(),
        };
        engine.set(&Settings::default());
        engine
    }

    /// Update the settings. Cheap when nothing changed, and never allocates.
    pub fn set(&mut self, s: &Settings) {
        if self.configured && *s == self.settings {
            return;
        }
        let sr = self.sample_rate;
        let ms = 0.001 * sr;
        let s = Settings {
            mix: s.mix.clamp(0.0, 1.0),
            size: s.size.clamp(0.0, 1.0),
            decay: s.decay.clamp(0.15, 6.0),
            predelay_ms: s.predelay_ms.clamp(0.0, MAX_PREDELAY_MS),
            width: s.width.clamp(0.0, 1.0),
            duck: s.duck.clamp(0.0, 1.0),
            warmth: s.warmth.clamp(0.0, 1.0),
            spread: s.spread.clamp(0.0, 1.0),
            ..*s
        };

        self.size_scale = MIN_SCALE + (MAX_SCALE - MIN_SCALE) * s.size;
        #[allow(clippy::needless_range_loop)] // indexes several per-line arrays in lockstep
        for i in 0..LINES {
            self.line_len[i] = LINE_MS[i] * self.size_scale * ms;
            // Per-line feedback for the requested RT60
            let seconds = self.line_len[i] / sr;
            self.feedback[i] = 10f32.powf(-3.0 * seconds / s.decay);
            // In-loop damping sits above the post filter so the tail darkens gradually
            self.loop_damping[i].set_cutoff((s.damping * 1.6).min(16_000.0), sr);
        }
        for (i, d) in self.diffusers.iter_mut().enumerate() {
            d.delay = ((DIFFUSER_MS[i] * (0.5 + 0.5 * self.size_scale) * ms) as usize).max(1);
            d.gain = if i < 2 { 0.72 } else { 0.62 };
        }
        self.mod_depth = 0.25 * ms * (0.5 + s.size);
        self.predelay_samples = s.predelay_ms * ms;

        self.wet_hp.set(BiquadType::Highpass, s.low_cut, 0.707, sr);
        for c in 0..2 {
            self.wet_body[c].set(BiquadType::Peak(s.body_db), 350.0, 0.8, sr);
            self.wet_lp[c].set(BiquadType::Lowpass, s.damping, 0.707, sr);
            self.wet_lp2[c].set(
                BiquadType::Lowpass,
                (s.damping * 2.2).min(18_000.0),
                0.6,
                sr,
            );
            self.voice_body[c].set(BiquadType::LowShelf(s.voice_body_db), 180.0, 0.707, sr);
            self.voice_presence[c].set(BiquadType::Peak(s.presence_db), 3200.0, 0.9, sr);
            self.voice_air[c].set(BiquadType::HighShelf(s.air_db), 11_000.0, 0.707, sr);
        }
        self.drive = 1.0 + 4.0 * s.warmth;
        // A touch of make-up so adding warmth doesn't feel quieter
        self.warmth_comp = 1.0 + 0.15 * s.warmth;

        self.mix.set_target(s.mix);
        self.width.set_target(s.width);
        self.spread.set_target(s.spread);
        self.output.set_target(db_to_gain(s.output_db));
        self.predelay_smooth.set_target(self.predelay_samples);
        self.scale.set_target(self.size_scale);

        if !self.configured {
            self.mix.reset(s.mix);
            self.width.reset(s.width);
            self.spread.reset(s.spread);
            self.output.reset(db_to_gain(s.output_db));
            self.predelay_smooth.reset(self.predelay_samples);
            self.scale.reset(self.size_scale);
            self.duck_gain.reset(1.0);
            self.configured = true;
        }
        self.settings = s;
    }

    pub fn reset(&mut self) {
        self.predelay.clear();
        self.spread_l.clear();
        self.spread_r.clear();
        for line in &mut self.lines {
            line.clear();
        }
        for d in &mut self.diffusers {
            d.line.clear();
        }
        for f in &mut self.loop_damping {
            f.reset();
        }
        self.wet_hp.reset();
        for c in 0..2 {
            self.wet_body[c].reset();
            self.wet_lp[c].reset();
            self.wet_lp2[c].reset();
            self.voice_body[c].reset();
            self.voice_presence[c].reset();
            self.voice_air[c].reset();
        }
        self.follower.reset();
        self.duck_gain.reset(1.0);
        self.wet_level = 0.0;
    }

    /// Recent peak level of the reverb on its own, for the GUI.
    pub fn wet_level(&self) -> f32 {
        self.wet_level
    }

    /// Current ducking gain (1 = no ducking), for the GUI.
    pub fn duck_gain(&self) -> f32 {
        self.duck_gain.value()
    }

    #[inline]
    fn tick_lfos(&mut self) {
        for (phase, inc) in self.lfo_phase.iter_mut().zip(self.lfo_inc) {
            *phase += inc;
            if *phase >= TAU {
                *phase -= TAU;
            }
        }
    }

    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let mix = self.mix.tick();
        let width = self.width.tick();
        let spread = self.spread.tick();
        let output = self.output.tick();
        let predelay = self.predelay_smooth.tick();
        let scale = self.scale.tick();
        self.tick_lfos();

        // ---- Dry voice
        let mut dry_l = self.voice_air[0]
            .process(self.voice_presence[0].process(self.voice_body[0].process(in_l)));
        let mut dry_r = self.voice_air[1]
            .process(self.voice_presence[1].process(self.voice_body[1].process(in_r)));
        if self.drive > 1.0001 {
            dry_l = (self.drive * dry_l).tanh() / self.drive * self.warmth_comp;
            dry_r = (self.drive * dry_r).tanh() / self.drive * self.warmth_comp;
        }

        // Spreader: each side gets the other's short, slowly wobbling delayed copy
        self.spread_l.push(dry_l);
        self.spread_r.push(dry_r);
        if spread > 0.001 {
            let ms = 0.001 * self.sample_rate;
            let wl = self
                .spread_l
                .read_linear((9.0 + 1.4 * self.lfo_phase[0].sin()) * ms);
            let wr = self
                .spread_r
                .read_linear((13.0 - 1.4 * self.lfo_phase[1].sin()) * ms);
            let amount = spread * 0.55;
            let comp = 1.0 / (1.0 + 0.25 * spread);
            dry_l = (dry_l + amount * wr) * comp;
            dry_r = (dry_r + amount * wl) * comp;
        }

        // ---- Ducking, driven by the incoming voice
        let level = self.follower.process(in_l.abs().max(in_r.abs()));
        self.duck_gain
            .set_target(1.0 / (1.0 + self.settings.duck * 9.0 * level));
        let duck = self.duck_gain.tick();

        // ---- Space
        let mut x = 0.5 * (in_l + in_r);
        self.predelay.push(x);
        if predelay > 1.0 {
            x = self.predelay.read_linear(predelay);
        }
        x = self.wet_hp.process(x);
        for d in &mut self.diffusers {
            x = d.process(x);
        }

        let mut taps = [0.0; LINES];
        #[allow(clippy::needless_range_loop)] // indexes several per-line arrays in lockstep
        for i in 0..LINES {
            let mut len = self.line_len[i] * scale / self.size_scale;
            if i < 4 {
                // Slow modulation on half the lines stops any metallic ringing
                len += self.mod_depth * self.lfo_phase[2 + i].sin();
            }
            taps[i] = self.loop_damping[i].lowpass(self.lines[i].read_linear(len.max(2.0)));
        }

        // Decorrelated stereo taps
        let mut wet_l = (taps[0] - taps[2] + taps[4] - taps[6] + 0.5 * (taps[1] - taps[5])) * 0.42;
        let mut wet_r = (taps[1] - taps[3] + taps[5] - taps[7] + 0.5 * (taps[2] - taps[6])) * 0.42;

        hadamard(&mut taps);
        for (i, tap) in taps.iter().enumerate() {
            let inject = if i % 2 == 0 { x } else { -x };
            self.lines[i].push(tap * self.feedback[i] + inject * 0.35);
        }

        wet_l = self.wet_lp2[0].process(self.wet_lp[0].process(self.wet_body[0].process(wet_l)));
        wet_r = self.wet_lp2[1].process(self.wet_lp[1].process(self.wet_body[1].process(wet_r)));

        let mid = 0.5 * (wet_l + wet_r);
        let side = 0.5 * (wet_l - wet_r) * width;
        let gain = mix * duck * WET_MAKEUP;
        wet_l = (mid + side) * gain;
        wet_r = (mid - side) * gain;

        let peak = wet_l.abs().max(wet_r.abs());
        self.wet_level = peak.max(self.wet_level * self.meter_release);

        let (out_l, out_r) = if self.settings.wet_only {
            (wet_l * output, wet_r * output)
        } else {
            ((dry_l + wet_l) * output, (dry_r + wet_r) * output)
        };

        if !(out_l.is_finite() && out_r.is_finite()) {
            self.reset();
            return (0.0, 0.0);
        }
        (out_l, out_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Preset;

    const SR: f32 = 48_000.0;

    fn impulse_response(settings: Settings, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let mut e = CloserEngine::new(SR);
        e.set(&settings);
        let n = (SR * seconds) as usize;
        let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..n {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (a, b) = e.process(x, x);
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    fn tail_only(p: Preset) -> Settings {
        Settings {
            wet_only: true,
            duck: 0.0,
            mix: 1.0,
            ..p.settings()
        }
    }

    /// RT60 from Schroeder backward integration (T20, extrapolated).
    fn measure_rt60(l: &[f32], r: &[f32]) -> f32 {
        let mut edc = vec![0.0f64; l.len()];
        let mut acc = 0.0f64;
        for i in (0..l.len()).rev() {
            acc += (l[i] as f64).powi(2) + (r[i] as f64).powi(2);
            edc[i] = acc;
        }
        let time_at = |db: f64| {
            edc.iter()
                .position(|&e| 10.0 * (e / edc[0] + 1e-30).log10() <= db)
                .unwrap_or(l.len()) as f32
                / SR
        };
        (time_at(-25.0) - time_at(-5.0)) * 3.0
    }

    #[test]
    fn every_preset_is_stable_and_decays_as_set() {
        for preset in Preset::ALL {
            let s = tail_only(preset);
            let (l, r) = impulse_response(s, 4.0);
            assert!(l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 4.0));
            let rt60 = measure_rt60(&l, &r);
            assert!(
                rt60 > s.decay * 0.6 && rt60 < s.decay * 1.5,
                "{}: RT60 {rt60:.2}s for decay {:.2}s",
                preset.info().name,
                s.decay
            );
        }
    }

    #[test]
    fn predelay_keeps_the_start_dry() {
        for preset in Preset::ALL {
            let s = tail_only(preset);
            let (l, _) = impulse_response(s, 0.5);
            let peak = l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let onset = l.iter().position(|x| x.abs() > peak * 0.01).unwrap() as f32 / SR;
            assert!(
                onset * 1000.0 >= s.predelay_ms,
                "{}: tail starts at {:.1} ms, pre-delay {} ms",
                preset.info().name,
                onset * 1000.0,
                s.predelay_ms
            );
        }
    }

    #[test]
    fn tail_is_stereo() {
        let (l, r) = impulse_response(tail_only(Preset::BvStackedHarmonies), 1.0);
        let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
        for i in (SR as usize / 20)..l.len() {
            lr += (l[i] * r[i]) as f64;
            ll += (l[i] * l[i]) as f64;
            rr += (r[i] * r[i]) as f64;
        }
        assert!(lr / (ll * rr).sqrt() < 0.3);
    }

    #[test]
    fn singing_ducks_the_tail() {
        let wet_energy = |duck: f32| {
            let mut e = CloserEngine::new(SR);
            e.set(&Settings {
                duck,
                ..tail_only(Preset::LeadClosePlate)
            });
            let mut energy = 0.0f64;
            for i in 0..SR as usize {
                let x = 0.3 * (TAU * 220.0 * i as f32 / SR).sin();
                let (l, _) = e.process(x, x);
                if i > SR as usize / 2 {
                    energy += (l * l) as f64;
                }
            }
            10.0 * energy.log10()
        };
        let reduction = wet_energy(0.0) - wet_energy(1.0);
        assert!(reduction > 6.0, "ducking only {reduction:.1} dB");
    }

    #[test]
    fn neutral_settings_pass_the_voice_untouched() {
        let mut e = CloserEngine::new(SR);
        e.set(&Settings {
            mix: 0.0,
            voice_body_db: 0.0,
            presence_db: 0.0,
            air_db: 0.0,
            warmth: 0.0,
            spread: 0.0,
            output_db: 0.0,
            wet_only: false,
            ..Settings::default()
        });
        for i in 0..SR as usize {
            let x = 0.5 * (TAU * 1000.0 * i as f32 / SR).sin();
            let (l, r) = e.process(x, x);
            assert!((l - x).abs() < 1e-3 && (r - x).abs() < 1e-3);
        }
    }

    #[test]
    fn lead_reverb_sits_well_under_the_voice() {
        // Steady noise in, wet-only out: the lead presets should put the tail roughly
        // 14-22 dB under the dry voice, the BV presets a little higher.
        let mut rng = olo_dsp::Rng::new(9);
        for preset in [Preset::LeadClosePlate, Preset::BvBehindTheLead] {
            let mut e = CloserEngine::new(SR);
            e.set(&Settings {
                wet_only: true,
                duck: 0.0,
                ..preset.settings()
            });
            let (mut input, mut wet) = (0.0f64, 0.0f64);
            for i in 0..(2.0 * SR) as usize {
                let x = rng.bipolar() * 0.1;
                let (l, r) = e.process(x, x);
                if i > SR as usize {
                    input += (x * x) as f64;
                    wet += (0.5 * (l * l + r * r)) as f64;
                }
            }
            let db = 10.0 * (wet / input).log10();
            assert!(
                (-24.0..-10.0).contains(&db),
                "{}: {db:.1} dB",
                preset.info().name
            );
        }
    }

    #[test]
    fn backing_vocal_presets_sit_behind_their_lead() {
        for preset in Preset::ALL {
            let info = preset.info();
            let Some(partner) = info.pairs_with else {
                continue;
            };
            let (lead, bv) = if info.is_backing {
                (partner.settings(), preset.settings())
            } else {
                (preset.settings(), partner.settings())
            };
            assert!(bv.decay > lead.decay, "{}", info.name);
            assert!(bv.damping < lead.damping, "{}", info.name);
            assert!(bv.presence_db < lead.presence_db, "{}", info.name);
            assert!(bv.width >= lead.width, "{}", info.name);
        }
    }
}
