/// A mono circular delay line with a power-of-two buffer and fractional reads.
#[derive(Debug, Clone, Default)]
pub struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Create a delay line that can hold at least `max_delay` samples of history.
    pub fn new(max_delay: usize) -> Self {
        let mut line = Self::default();
        line.resize(max_delay);
        line
    }

    /// Reallocate for at least `max_delay` samples. Only call this outside of `process()`.
    pub fn resize(&mut self, max_delay: usize) {
        let size = (max_delay + 4).next_power_of_two();
        self.buffer = vec![0.0; size];
        self.mask = size - 1;
        self.write = 0;
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }

    /// Number of samples the buffer can hold.
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Push a new sample. Afterwards `read(1)` returns it.
    #[inline]
    pub fn push(&mut self, x: f32) {
        self.buffer[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// Read the sample written `delay` pushes ago (`delay >= 1`).
    #[inline]
    pub fn read(&self, delay: usize) -> f32 {
        self.buffer[(self.write.wrapping_sub(delay)) & self.mask]
    }

    /// Linearly interpolated read, `delay >= 1.0`.
    #[inline]
    pub fn read_linear(&self, delay: f32) -> f32 {
        let d = delay.max(1.0);
        let i = d as usize;
        let frac = d - i as f32;
        let a = self.read(i);
        let b = self.read(i + 1);
        a + (b - a) * frac
    }

    /// Cubic (Hermite) interpolated read, `delay >= 2.0` for best results.
    #[inline]
    pub fn read_cubic(&self, delay: f32) -> f32 {
        let d = delay.max(2.0);
        let i = d as usize;
        let t = d - i as f32;
        let xm1 = self.read(i - 1);
        let x0 = self.read(i);
        let x1 = self.read(i + 1);
        let x2 = self.read(i + 2);
        let c0 = x0;
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + c0
    }
}

/// A circular buffer addressed by absolute sample index rather than by delay. This is handy for
/// effects that need to jump around in recent history (stutters, grains, reverse playback).
#[derive(Debug, Clone, Default)]
pub struct HistoryBuffer {
    buffer: Vec<f32>,
    mask: usize,
}

impl HistoryBuffer {
    pub fn new(min_len: usize) -> Self {
        let size = min_len.next_power_of_two().max(4);
        Self {
            buffer: vec![0.0; size],
            mask: size - 1,
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    #[inline]
    pub fn write(&mut self, index: i64, x: f32) {
        self.buffer[(index as usize) & self.mask] = x;
    }

    #[inline]
    pub fn get(&self, index: i64) -> f32 {
        self.buffer[(index as usize) & self.mask]
    }

    /// Linearly interpolated read at a fractional absolute position.
    #[inline]
    pub fn get_linear(&self, pos: f64) -> f32 {
        let i = pos.floor();
        let frac = (pos - i) as f32;
        let i = i as i64;
        let a = self.get(i);
        let b = self.get(i + 1);
        a + (b - a) * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_delay_reads_back_samples() {
        let mut d = DelayLine::new(16);
        for i in 0..10 {
            d.push(i as f32);
        }
        assert_eq!(d.read(1), 9.0);
        assert_eq!(d.read(5), 5.0);
        assert!((d.read_linear(1.5) - 8.5).abs() < 1e-6);
        assert!((d.read_cubic(3.0) - 7.0).abs() < 1e-6);
    }

    #[test]
    fn history_buffer_wraps() {
        let mut h = HistoryBuffer::new(8);
        for i in 0..20i64 {
            h.write(i, i as f32);
        }
        assert_eq!(h.get(19), 19.0);
        assert_eq!(h.get(12), 12.0);
        assert!((h.get_linear(15.25) - 15.25).abs() < 1e-6);
    }
}
