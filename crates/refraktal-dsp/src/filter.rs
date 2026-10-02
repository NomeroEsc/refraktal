// SPDX-License-Identifier: GPL-3.0-or-later

use core::f32::consts::TAU;

/// Simple one-pole high-pass filter.
#[derive(Clone, Debug)]
pub struct OnePoleHighpass {
    a: f32,
    x1: f32,
    y1: f32,
}

impl OnePoleHighpass {
    #[must_use]
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let rc = 1.0 / (TAU * cutoff_hz);
        let dt = 1.0 / sample_rate;
        Self { a: rc / (rc + dt), x1: 0.0, y1: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.a * (self.y1 + x - self.x1);
        self.x1 = x;
        self.y1 = y;
        y
    }
}
