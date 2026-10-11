use std::f32::consts::{PI, TAU};

/// A one-pole filter providing both lowpass and highpass outputs.
#[derive(Debug, Clone, Copy, Default)]
pub struct OnePole {
    coeff: f32,
    state: f32,
}

impl OnePole {
    /// Set the cutoff frequency in Hz.
    #[inline]
    pub fn set_cutoff(&mut self, cutoff: f32, sample_rate: f32) {
        let fc = cutoff.clamp(1.0, sample_rate * 0.49);
        self.coeff = 1.0 - (-TAU * fc / sample_rate).exp();
    }

    /// Lowpass output.
    #[inline]
    pub fn lowpass(&mut self, x: f32) -> f32 {
        self.state += self.coeff * (x - self.state);
        self.state
    }

    /// Highpass output (input minus lowpass).
    #[inline]
    pub fn highpass(&mut self, x: f32) -> f32 {
        x - self.lowpass(x)
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// The response type for a [`Biquad`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BiquadType {
    Lowpass,
    Highpass,
    Bandpass,
    Peak(f32),
    LowShelf(f32),
    HighShelf(f32),
}

/// A transposed direct form II biquad using the RBJ cookbook formulas.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        // Passthrough
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    pub fn new(kind: BiquadType, freq: f32, q: f32, sample_rate: f32) -> Self {
        let mut f = Self::default();
        f.set(kind, freq, q, sample_rate);
        f
    }

    /// Recompute coefficients. Keeps the filter state so it can be called while running.
    pub fn set(&mut self, kind: BiquadType, freq: f32, q: f32, sample_rate: f32) {
        let freq = freq.clamp(5.0, sample_rate * 0.49);
        let q = q.max(0.05);
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match kind {
            BiquadType::Lowpass => {
                let b1 = 1.0 - cos_w0;
                (
                    b1 / 2.0,
                    b1,
                    b1 / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            BiquadType::Highpass => {
                let b1 = -(1.0 + cos_w0);
                (
                    -b1 / 2.0,
                    b1,
                    -b1 / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            BiquadType::Bandpass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            BiquadType::Peak(gain_db) => {
                let a = 10f32.powf(gain_db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos_w0,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos_w0,
                    1.0 - alpha / a,
                )
            }
            BiquadType::LowShelf(gain_db) => {
                let a = 10f32.powf(gain_db / 40.0);
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - sq),
                    (a + 1.0) + (a - 1.0) * cos_w0 + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - sq,
                )
            }
            BiquadType::HighShelf(gain_db) => {
                let a = 10f32.powf(gain_db / 40.0);
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - sq),
                    (a + 1.0) - (a - 1.0) * cos_w0 + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - sq,
                )
            }
        };

        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Removes DC offset with a gentle ~10 Hz highpass.
#[derive(Debug, Clone, Copy, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlocker {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            x1: 0.0,
            y1: 0.0,
            r: 1.0 - (TAU * 10.0 / sample_rate),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_gain(filter: &mut Biquad, freq: f32, sr: f32) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..(sr as usize) {
            let x = (TAU * freq * i as f32 / sr).sin();
            let y = filter.process(x);
            if i > sr as usize / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn lowpass_passes_lows_and_cuts_highs() {
        let sr = 48_000.0;
        let mut lp = Biquad::new(BiquadType::Lowpass, 1000.0, 0.707, sr);
        assert!((sine_gain(&mut lp, 100.0, sr) - 1.0).abs() < 0.05);
        lp.reset();
        assert!(sine_gain(&mut lp, 10_000.0, sr) < 0.05);
    }

    #[test]
    fn highpass_cuts_lows() {
        let sr = 48_000.0;
        let mut hp = Biquad::new(BiquadType::Highpass, 1000.0, 0.707, sr);
        assert!(sine_gain(&mut hp, 50.0, sr) < 0.01);
    }

    #[test]
    fn one_pole_lowpass_converges() {
        let mut f = OnePole::default();
        f.set_cutoff(100.0, 48_000.0);
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = f.lowpass(1.0);
        }
        assert!((y - 1.0).abs() < 1e-3);
    }
}
