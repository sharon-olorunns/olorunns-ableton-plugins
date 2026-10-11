//! Small, allocation-free DSP building blocks shared by the Olorunns plugins.
//!
//! Everything here allocates only in constructors / `resize` calls, never while processing, so it
//! is safe to use from the audio thread once set up in `initialize()`.

pub mod delay;
pub mod filter;
pub mod pitch;
pub mod smooth;

pub use delay::DelayLine;
pub use filter::{Biquad, OnePole};
pub use smooth::Smoother;

/// Convert decibels to linear gain.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Convert linear gain to decibels, clamped at -120 dB.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-6).log10()
}

/// MIDI note number to frequency in Hz (A4 = 69 = 440 Hz).
#[inline]
pub fn note_to_freq(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Frequency in Hz to a (fractional) MIDI note number.
#[inline]
pub fn freq_to_note(freq: f32) -> f32 {
    69.0 + 12.0 * (freq.max(1e-3) / 440.0).log2()
}

/// Musical note name for a MIDI note number, e.g. `60 -> "C4"`.
pub fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = note.div_euclid(12) - 1;
    format!("{}{}", NAMES[note.rem_euclid(12) as usize], octave)
}

/// A cheap soft clipper that is transparent at low levels and never exceeds +-1.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    if x <= -3.0 {
        -1.0
    } else if x >= 3.0 {
        1.0
    } else {
        (x * (27.0 + x * x) / (27.0 + 9.0 * x * x)).clamp(-1.0, 1.0)
    }
}

/// Equal-power crossfade gains for `t` in `[0, 1]`, returned as `(from, to)`.
#[inline]
pub fn equal_power(t: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (t.cos(), t.sin())
}

/// Replace NaN/inf with silence. Used as a last line of defence before writing to the host.
#[inline]
pub fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// A tiny, deterministic xorshift random number generator suitable for the audio thread.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform float in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform float in `[-1, 1)`.
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_conversions_round_trip() {
        assert!((note_to_freq(69.0) - 440.0).abs() < 1e-3);
        assert!((freq_to_note(261.6256) - 60.0).abs() < 1e-3);
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(70), "A#4");
    }

    #[test]
    fn soft_clip_is_bounded_and_monotonic() {
        let mut prev = -2.0;
        for i in -1000..=1000 {
            let y = soft_clip(i as f32 / 100.0);
            assert!((-1.0..=1.0).contains(&y));
            assert!(y >= prev);
            prev = y;
        }
    }

    #[test]
    fn rng_stays_in_range() {
        let mut rng = Rng::new(1234);
        for _ in 0..10_000 {
            let x = rng.next_f32();
            assert!((0.0..1.0).contains(&x));
        }
    }
}
