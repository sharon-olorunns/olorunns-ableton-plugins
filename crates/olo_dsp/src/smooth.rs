/// A one-pole parameter smoother that reaches ~99% of its target after `time_ms`.
#[derive(Debug, Clone, Copy)]
pub struct Smoother {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Default for Smoother {
    fn default() -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            coeff: 1.0,
        }
    }
}

impl Smoother {
    pub fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            coeff: 1.0,
        }
    }

    pub fn set_time(&mut self, time_ms: f32, sample_rate: f32) {
        let samples = (time_ms * 0.001 * sample_rate).max(1.0);
        // ln(100) ~= 4.6, so after `samples` steps we're within 1% of the target
        self.coeff = 1.0 - (-4.6 / samples).exp();
    }

    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn reset(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.current += (self.target - self.current) * self.coeff;
        self.current
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.current
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }
}

/// An attack/release envelope follower working on absolute sample values.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvelopeFollower {
    attack: f32,
    release: f32,
    env: f32,
}

impl EnvelopeFollower {
    pub fn new(attack_ms: f32, release_ms: f32, sample_rate: f32) -> Self {
        let coeff = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sample_rate).max(1.0)).exp();
        Self {
            attack: coeff(attack_ms),
            release: coeff(release_ms),
            env: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x.abs();
        let c = if x > self.env {
            self.attack
        } else {
            self.release
        };
        self.env += (x - self.env) * c;
        self.env
    }

    pub fn value(&self) -> f32 {
        self.env
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoother_reaches_target() {
        let mut s = Smoother::new(0.0);
        s.set_time(10.0, 48_000.0);
        s.set_target(1.0);
        for _ in 0..480 {
            s.tick();
        }
        assert!(s.value() > 0.98);
    }

    #[test]
    fn follower_tracks_level() {
        let mut f = EnvelopeFollower::new(1.0, 100.0, 48_000.0);
        for _ in 0..4800 {
            f.process(0.5);
        }
        assert!((f.value() - 0.5).abs() < 0.01);
    }
}
