// SPDX-License-Identifier: GPL-3.0-or-later
//! DSP building blocks for Refraktal.
//!
//! Everything in this crate is allocation-free after construction and safe
//! to call from the real-time audio thread.

mod drums;
mod filter;
mod noise;

pub use drums::{Hat, Kick, Snare};
pub use filter::OnePoleHighpass;
pub use noise::Noise;

/// A sound source that can be triggered and rendered sample by sample.
pub trait Voice {
    /// Start the sound. `velocity` is expected in `0.0..=1.0`.
    fn trigger(&mut self, velocity: f32);

    /// Render the next mono sample.
    fn next_sample(&mut self) -> f32;
}

/// Per-sample multiplier that makes an envelope fall by 60 dB over `seconds`.
#[must_use]
pub fn decay_coefficient(seconds: f32, sample_rate: f32) -> f32 {
    0.001_f32.powf(1.0 / (seconds * sample_rate).max(1.0))
}
