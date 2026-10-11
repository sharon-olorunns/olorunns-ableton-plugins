use crate::delay::DelayLine;
use std::f32::consts::PI;

/// A classic two-tap granular pitch shifter: two read heads sweep through a short delay line at
/// the shifted rate and crossfade with complementary windows. It smears transients a little, which
/// is exactly what we want inside reverb and texture effects.
#[derive(Debug, Clone, Default)]
pub struct GrainShifter {
    line: DelayLine,
    window: f32,
    phase: f32,
    ratio: f32,
}

impl GrainShifter {
    /// `window_ms` is the grain size. 50-100 ms works well for pads and reverb tails.
    pub fn new(window_ms: f32, sample_rate: f32) -> Self {
        let window = (window_ms * 0.001 * sample_rate).max(64.0);
        Self {
            line: DelayLine::new(window as usize + 8),
            window,
            phase: 0.0,
            ratio: 1.0,
        }
    }

    /// Set the pitch ratio, e.g. `2.0` for an octave up.
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(0.25, 4.0);
    }

    pub fn set_semitones(&mut self, semitones: f32) {
        self.set_ratio(2f32.powf(semitones / 12.0));
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.line.push(x);

        self.phase += (1.0 - self.ratio) / self.window;
        self.phase -= self.phase.floor();
        let phase_b = {
            let p = self.phase + 0.5;
            p - p.floor()
        };

        let tap = |phase: f32| -> (f32, f32) {
            let gain = (PI * phase).sin();
            (2.0 + phase * self.window, gain * gain)
        };
        let (delay_a, gain_a) = tap(self.phase);
        let (delay_b, gain_b) = tap(phase_b);

        self.line.read_linear(delay_a) * gain_a + self.line.read_linear(delay_b) * gain_b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Estimate frequency by counting positive-going zero crossings.
    fn estimate_freq(samples: &[f32], sr: f32) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        crossings as f32 * sr / samples.len() as f32
    }

    #[test]
    fn octave_up_doubles_frequency() {
        let sr = 48_000.0;
        let mut shifter = GrainShifter::new(60.0, sr);
        shifter.set_semitones(12.0);
        let out: Vec<f32> = (0..96_000)
            .map(|i| shifter.process((TAU * 220.0 * i as f32 / sr).sin()))
            .collect();
        let f = estimate_freq(&out[48_000..], sr);
        assert!((f - 440.0).abs() < 20.0, "got {f}");
    }

    #[test]
    fn unity_ratio_is_transparent_in_level() {
        let sr = 48_000.0;
        let mut shifter = GrainShifter::new(60.0, sr);
        let out: Vec<f32> = (0..48_000)
            .map(|i| shifter.process((TAU * 220.0 * i as f32 / sr).sin()))
            .collect();
        let peak = out[24_000..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((peak - 1.0).abs() < 0.05, "got {peak}");
    }
}
