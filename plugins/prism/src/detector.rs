//! Monophonic pitch detection using the YIN algorithm on a decimated copy of the input.
//!
//! Voices sit well below 1 kHz, so running the detector at ~12 kHz keeps it cheap without losing
//! accuracy (parabolic interpolation recovers the sub-sample period).

use olo_dsp::filter::{Biquad, BiquadType};

pub const MIN_F0: f32 = 70.0;
pub const MAX_F0: f32 = 1000.0;
/// YIN's "absolute threshold" for picking the first dip.
const THRESHOLD: f32 = 0.15;
/// Frames with a best dip above this are treated as unvoiced (noise, breaths, consonants).
const VOICED_LIMIT: f32 = 0.35;
/// Frames quieter than this RMS level are treated as silence.
const SILENCE_RMS: f32 = 0.0015;

/// The result of analysing one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchFrame {
    /// Absolute input sample index at the centre of the analysed frame.
    pub center: i64,
    /// Period in input samples. Only meaningful when `voiced`.
    pub period: f32,
    pub voiced: bool,
    /// Lower is more periodic.
    pub aperiodicity: f32,
}

pub struct PitchDetector {
    factor: usize,
    filters: [Biquad; 2],
    decimated: Vec<f32>,
    write: usize,
    phase: usize,
    hop: usize,
    hop_counter: usize,

    window: usize,
    max_lag: usize,
    min_lag: usize,
    diff: Vec<f32>,
    frame: Vec<f32>,

    /// Absolute index of the most recent input sample.
    now: i64,
    history: [f32; 3],
}

impl PitchDetector {
    pub fn new(sample_rate: f32) -> Self {
        let factor = ((sample_rate / 12_000.0).round() as usize).max(1);
        let rate = sample_rate / factor as f32;
        let cutoff = (rate * 0.4).min(4000.0);
        let filters = [
            Biquad::new(BiquadType::Lowpass, cutoff, 0.54, sample_rate),
            Biquad::new(BiquadType::Lowpass, cutoff, 1.31, sample_rate),
        ];
        let max_lag = (rate / MIN_F0).ceil() as usize;
        let min_lag = ((rate / MAX_F0).floor() as usize).max(2);
        let window = max_lag;
        let len = (window + max_lag + 2).next_power_of_two() * 2;
        let hop = ((sample_rate * 0.005) as usize / factor).max(1) * factor;

        Self {
            factor,
            filters,
            decimated: vec![0.0; len],
            write: 0,
            phase: 0,
            hop,
            hop_counter: 0,
            window,
            max_lag,
            min_lag,
            diff: vec![0.0; max_lag + 2],
            frame: vec![0.0; window + max_lag + 1],
            now: -1,
            history: [0.0; 3],
        }
    }

    /// How far behind the newest input sample a frame's centre lies, in input samples.
    pub fn center_delay(&self) -> usize {
        (self.window + self.max_lag) * self.factor / 2
    }

    pub fn reset(&mut self) {
        self.decimated.fill(0.0);
        for f in &mut self.filters {
            f.reset();
        }
        self.write = 0;
        self.phase = 0;
        self.hop_counter = 0;
        self.history = [0.0; 3];
    }

    /// Feed one input sample. Returns a new analysis frame every few milliseconds.
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<PitchFrame> {
        self.now += 1;
        let mut y = x;
        if self.factor > 1 {
            for f in &mut self.filters {
                y = f.process(y);
            }
        }
        self.phase += 1;
        if self.phase >= self.factor {
            self.phase = 0;
            let mask = self.decimated.len() - 1;
            self.decimated[self.write & mask] = y;
            self.write = self.write.wrapping_add(1);
        }

        self.hop_counter += 1;
        if self.hop_counter >= self.hop {
            self.hop_counter = 0;
            Some(self.analyse())
        } else {
            None
        }
    }

    fn analyse(&mut self) -> PitchFrame {
        let n = self.window + self.max_lag;
        let mask = self.decimated.len() - 1;
        let start = self.write.wrapping_sub(n);
        let mut energy = 0.0;
        for i in 0..n {
            let v = self.decimated[(start.wrapping_add(i)) & mask];
            self.frame[i] = v;
            energy += v * v;
        }
        let rms = (energy / n as f32).sqrt();
        let center = self.now - self.center_delay() as i64;

        if rms < SILENCE_RMS {
            return PitchFrame {
                center,
                period: 0.0,
                voiced: false,
                aperiodicity: 1.0,
            };
        }

        // Difference function and cumulative mean normalisation
        let w = self.window;
        self.diff[0] = 1.0;
        let mut running = 0.0;
        for tau in 1..=self.max_lag {
            let mut sum = 0.0;
            for j in 0..w {
                let d = self.frame[j] - self.frame[j + tau];
                sum += d * d;
            }
            running += sum;
            self.diff[tau] = if running > 0.0 {
                sum * tau as f32 / running
            } else {
                1.0
            };
        }

        // First dip under the threshold, followed down to its minimum
        let mut best = None;
        let mut tau = self.min_lag;
        while tau < self.max_lag {
            if self.diff[tau] < THRESHOLD {
                while tau + 1 < self.max_lag && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let tau = best.unwrap_or_else(|| {
            (self.min_lag..self.max_lag)
                .min_by(|a, b| self.diff[*a].total_cmp(&self.diff[*b]))
                .unwrap_or(self.min_lag)
        });
        let aperiodicity = self.diff[tau];

        // Parabolic interpolation for a sub-sample period
        let refined = if tau > 1 && tau < self.max_lag {
            let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                tau as f32 + 0.5 * (a - c) / denom
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };

        let voiced = aperiodicity < VOICED_LIMIT;
        let period = refined * self.factor as f32;

        // Median of the last three periods removes single-frame octave glitches
        self.history = [
            self.history[1],
            self.history[2],
            if voiced { period } else { 0.0 },
        ];
        let period = if voiced && self.history.iter().all(|p| *p > 0.0) {
            let mut h = self.history;
            h.sort_by(|a, b| a.total_cmp(b));
            h[1]
        } else {
            period
        };

        PitchFrame {
            center,
            period,
            voiced,
            aperiodicity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// A buzzy, voice-like test signal: a band-limited pulse train with a few harmonics.
    pub fn voice_like(f0: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let mut s = 0.0;
                for h in 1..12 {
                    let f = f0 * h as f32;
                    if f > sr * 0.45 {
                        break;
                    }
                    // A crude formant bump around 700 Hz
                    let formant = 1.0 / (1.0 + ((f - 700.0) / 400.0).powi(2));
                    s += (TAU * f * t).sin() * (0.3 + formant) / h as f32;
                }
                s * 0.3
            })
            .collect()
    }

    fn detect(signal: &[f32], sr: f32) -> Vec<PitchFrame> {
        let mut d = PitchDetector::new(sr);
        signal.iter().filter_map(|x| d.push(*x)).collect()
    }

    #[test]
    fn detects_voice_pitches_across_the_range() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for f0 in [82.0, 110.0, 220.0, 330.0, 523.0, 880.0] {
                let frames = detect(&voice_like(f0, sr, sr as usize / 2), sr);
                let settled = &frames[frames.len() / 2..];
                for frame in settled {
                    assert!(frame.voiced, "{f0} Hz at {sr} should be voiced");
                    let detected = sr / frame.period;
                    let cents = 1200.0 * (detected / f0).log2();
                    assert!(cents.abs() < 10.0, "{f0} Hz at {sr}: got {detected} Hz");
                }
            }
        }
    }

    #[test]
    fn silence_and_noise_are_unvoiced() {
        let sr = 48_000.0;
        let frames = detect(&vec![0.0; 24_000], sr);
        assert!(frames.iter().all(|f| !f.voiced));

        let mut rng = olo_dsp::Rng::new(5);
        let noise: Vec<f32> = (0..24_000).map(|_| rng.bipolar() * 0.3).collect();
        let frames = detect(&noise, sr);
        let voiced = frames.iter().filter(|f| f.voiced).count();
        assert!(
            voiced * 10 < frames.len(),
            "{voiced} of {} noise frames voiced",
            frames.len()
        );
    }
}
