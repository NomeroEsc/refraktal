// SPDX-License-Identifier: GPL-3.0-or-later

/// Fast xorshift32 white noise generator.
#[derive(Clone, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    #[must_use]
    pub fn new(seed: u32) -> Self {
        // xorshift must never be seeded with zero.
        Self { state: seed.max(1) }
    }

    /// Next sample in `-1.0..=1.0`.
    #[inline]
    pub fn sample(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        (x as f32 / u32::MAX as f32).mul_add(2.0, -1.0)
    }
}
