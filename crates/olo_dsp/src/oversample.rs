//! 4x oversampling with linear-phase FIR filters, for running distortion without the harsh
//! aliasing it would otherwise create.

const FACTOR: usize = 4;
const TAPS: usize = 96;
const TAPS_PER_PHASE: usize = TAPS / FACTOR;

pub struct Oversampler4x {
    taps: [f32; TAPS],
    /// Recent input samples for the polyphase interpolator.
    input: [f32; TAPS_PER_PHASE],
    input_pos: usize,
    /// Recent oversampled samples for the decimator.
    output: [f32; TAPS],
    output_pos: usize,
}

impl Default for Oversampler4x {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler4x {
    pub fn new() -> Self {
        // Blackman-windowed sinc lowpass at ~80% of the original Nyquist frequency
        let cutoff = 0.1f64; // cycles per oversampled sample
        let mut taps = [0.0f32; TAPS];
        let centre = (TAPS - 1) as f64 / 2.0;
        let mut sum = 0.0;
        for (i, tap) in taps.iter_mut().enumerate() {
            let n = i as f64 - centre;
            let sinc = if n.abs() < 1e-9 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * n).sin() / (std::f64::consts::PI * n)
            };
            let x = i as f64 / (TAPS - 1) as f64;
            let window = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * x).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * x).cos();
            *tap = (sinc * window) as f32;
            sum += sinc * window;
        }
        for tap in &mut taps {
            *tap /= sum as f32;
        }
        Self {
            taps,
            input: [0.0; TAPS_PER_PHASE],
            input_pos: 0,
            output: [0.0; TAPS],
            output_pos: 0,
        }
    }

    /// Latency in original-rate samples (both filters are linear phase).
    pub fn latency() -> usize {
        // (TAPS - 1) / 2 oversampled samples per filter, two filters
        ((TAPS - 1) as f32 / FACTOR as f32).round() as usize
    }

    pub fn reset(&mut self) {
        self.input = [0.0; TAPS_PER_PHASE];
        self.output = [0.0; TAPS];
    }

    /// Upsample one sample, run `f` on each of the four oversampled values, and downsample the
    /// result back to one sample.
    #[inline]
    pub fn process(&mut self, x: f32, mut f: impl FnMut(f32) -> f32) -> f32 {
        self.input_pos = (self.input_pos + 1) % TAPS_PER_PHASE;
        self.input[self.input_pos] = x;

        for phase in 0..FACTOR {
            // Polyphase interpolation: zero-stuffing means only every 4th tap sees input
            let mut up = 0.0;
            for k in 0..TAPS_PER_PHASE {
                let idx = (self.input_pos + TAPS_PER_PHASE - k) % TAPS_PER_PHASE;
                up += self.taps[k * FACTOR + phase] * self.input[idx];
            }
            let shaped = f(up * FACTOR as f32);
            self.output_pos = (self.output_pos + 1) % TAPS;
            self.output[self.output_pos] = shaped;
        }

        // Decimate: only the one output sample we keep needs computing
        let mut down = 0.0;
        for (j, tap) in self.taps.iter().enumerate() {
            down += tap * self.output[(self.output_pos + TAPS - j) % TAPS];
        }
        down
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn passes_audio_through_unchanged_in_level() {
        let sr = 48_000.0;
        let mut os = Oversampler4x::new();
        let out: Vec<f32> = (0..9600)
            .map(|i| os.process((TAU * 1000.0 * i as f32 / sr).sin(), |v| v))
            .collect();
        let peak = out[4800..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((peak - 1.0).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn latency_matches_impulse_response() {
        let mut os = Oversampler4x::new();
        let out: Vec<f32> = (0..64)
            .map(|i| os.process(if i == 0 { 1.0 } else { 0.0 }, |v| v))
            .collect();
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert!((peak as i32 - Oversampler4x::latency() as i32).abs() <= 1);
    }

    #[test]
    fn hard_clipping_aliases_less_than_without_oversampling() {
        // Clip a high sine hard and measure energy that isn't a harmonic of it
        let sr = 48_000.0;
        let f = 3_700.0;
        let n = 48_000;
        let clip = |v: f32| (v * 8.0).clamp(-1.0, 1.0);
        let mut os = Oversampler4x::new();
        let over: Vec<f32> = (0..n)
            .map(|i| os.process((TAU * f * i as f32 / sr).sin(), clip))
            .collect();
        let naive: Vec<f32> = (0..n)
            .map(|i| clip((TAU * f * i as f32 / sr).sin()))
            .collect();

        // Energy at 1 kHz, which is not a harmonic of 3.7 kHz, comes only from aliasing
        let probe = |s: &[f32], hz: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in s.iter().enumerate().skip(1000) {
                let p = (TAU * hz * i as f32 / sr) as f64;
                re += *v as f64 * p.cos();
                im += *v as f64 * p.sin();
            }
            (re * re + im * im).sqrt()
        };
        let alias_over: f64 = [500.0, 1000.0, 1300.0]
            .iter()
            .map(|h| probe(&over, *h))
            .sum();
        let alias_naive: f64 = [500.0, 1000.0, 1300.0]
            .iter()
            .map(|h| probe(&naive, *h))
            .sum();
        assert!(
            alias_over < alias_naive * 0.3,
            "{alias_over} vs {alias_naive}"
        );
    }
}
