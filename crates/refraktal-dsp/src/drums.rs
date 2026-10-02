// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthesized drum voices.

use core::f32::consts::TAU;

use crate::{decay_coefficient, Noise, OnePoleHighpass, Voice};

/// Below this level a voice is considered silent and skips its work.
const SILENCE: f32 = 1.0e-5;

/// Sine kick with a fast downward pitch sweep and soft saturation.
#[derive(Clone, Debug)]
pub struct Kick {
    sample_rate: f32,
    phase: f32,
    amp: f32,
    amp_decay: f32,
    sweep: f32,
    sweep_decay: f32,
    base_hz: f32,
    sweep_hz: f32,
}

impl Kick {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self::tuned(sample_rate, 45.0, 180.0, 0.45, 0.08)
    }

    fn tuned(sample_rate: f32, base_hz: f32, sweep_hz: f32, decay: f32, sweep_time: f32) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            amp: 0.0,
            amp_decay: decay_coefficient(decay, sample_rate),
            sweep: 0.0,
            sweep_decay: decay_coefficient(sweep_time, sample_rate),
            base_hz,
            sweep_hz,
        }
    }
}

/// Mid tom: a higher, shorter relative of the kick.
#[derive(Clone, Debug)]
pub struct Tom(Kick);

impl Tom {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self(Kick::tuned(sample_rate, 120.0, 70.0, 0.32, 0.06))
    }
}

impl Voice for Tom {
    fn trigger(&mut self, velocity: f32) {
        self.0.trigger(velocity);
    }

    fn next_sample(&mut self) -> f32 {
        self.0.next_sample() * 0.8
    }
}

/// Hand clap: three quick noise bursts followed by a short tail.
#[derive(Clone, Debug)]
pub struct Clap {
    sample_rate: f32,
    noise: Noise,
    filter: OnePoleHighpass,
    velocity: f32,
    /// Samples since the trigger; `None` when silent.
    elapsed: Option<u32>,
    tail: f32,
    tail_decay: f32,
}

impl Clap {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            noise: Noise::new(0x5EED_0003),
            filter: OnePoleHighpass::new(1000.0, sample_rate),
            velocity: 0.0,
            elapsed: None,
            tail: 0.0,
            tail_decay: decay_coefficient(0.18, sample_rate),
        }
    }
}

impl Voice for Clap {
    fn trigger(&mut self, velocity: f32) {
        self.velocity = velocity.clamp(0.0, 1.0);
        self.elapsed = Some(0);
        self.tail = 0.0;
    }

    fn next_sample(&mut self) -> f32 {
        let Some(n) = self.elapsed else {
            return 0.0;
        };
        let t = n as f32 / self.sample_rate;
        let burst_len = 0.011;
        let amp = if t < 3.0 * burst_len {
            // Each burst starts loud and decays quickly.
            (-(t % burst_len) / 0.0025).exp()
        } else {
            if self.tail == 0.0 {
                self.tail = 0.8;
            }
            self.tail *= self.tail_decay;
            self.tail
        };
        if t >= 3.0 * burst_len && self.tail < SILENCE {
            self.elapsed = None;
            return 0.0;
        }
        self.elapsed = Some(n + 1);
        self.filter.process(self.noise.sample()) * amp * self.velocity
    }
}

impl Voice for Kick {
    fn trigger(&mut self, velocity: f32) {
        self.amp = velocity.clamp(0.0, 1.0);
        self.sweep = 1.0;
        self.phase = 0.0;
    }

    fn next_sample(&mut self) -> f32 {
        if self.amp < SILENCE {
            return 0.0;
        }
        let hz = self.sweep_hz.mul_add(self.sweep, self.base_hz);
        self.phase = (self.phase + hz / self.sample_rate).fract();
        let s = (self.phase * TAU).sin() * self.amp;
        self.amp *= self.amp_decay;
        self.sweep *= self.sweep_decay;
        (s * 1.5).tanh()
    }
}

/// Noise burst plus a short tonal body.
#[derive(Clone, Debug)]
pub struct Snare {
    sample_rate: f32,
    noise: Noise,
    filter: OnePoleHighpass,
    noise_amp: f32,
    noise_decay: f32,
    tone_phase: f32,
    tone_amp: f32,
    tone_decay: f32,
}

impl Snare {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            noise: Noise::new(0x5EED_0001),
            filter: OnePoleHighpass::new(1500.0, sample_rate),
            noise_amp: 0.0,
            noise_decay: decay_coefficient(0.2, sample_rate),
            tone_phase: 0.0,
            tone_amp: 0.0,
            tone_decay: decay_coefficient(0.08, sample_rate),
        }
    }
}

impl Voice for Snare {
    fn trigger(&mut self, velocity: f32) {
        let v = velocity.clamp(0.0, 1.0);
        self.noise_amp = v;
        self.tone_amp = v;
        self.tone_phase = 0.0;
    }

    fn next_sample(&mut self) -> f32 {
        if self.noise_amp < SILENCE && self.tone_amp < SILENCE {
            return 0.0;
        }
        let noise = self.filter.process(self.noise.sample()) * self.noise_amp;
        self.tone_phase = (self.tone_phase + 190.0 / self.sample_rate).fract();
        let tone = (self.tone_phase * TAU).sin() * self.tone_amp;
        self.noise_amp *= self.noise_decay;
        self.tone_amp *= self.tone_decay;
        noise.mul_add(0.7, tone * 0.5)
    }
}

/// Closed hi-hat: high-passed noise with a very short decay.
#[derive(Clone, Debug)]
pub struct Hat {
    noise: Noise,
    filter: OnePoleHighpass,
    amp: f32,
    decay: f32,
}

impl Hat {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            noise: Noise::new(0x5EED_0002),
            filter: OnePoleHighpass::new(7000.0, sample_rate),
            amp: 0.0,
            decay: decay_coefficient(0.05, sample_rate),
        }
    }
}

impl Voice for Hat {
    fn trigger(&mut self, velocity: f32) {
        self.amp = velocity.clamp(0.0, 1.0);
    }

    fn next_sample(&mut self) -> f32 {
        if self.amp < SILENCE {
            return 0.0;
        }
        let s = self.filter.process(self.noise.sample()) * self.amp;
        self.amp *= self.decay;
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(voice: &mut impl Voice, samples: usize) -> (f32, f32) {
        voice.trigger(1.0);
        let mut peak = 0.0_f32;
        let mut last = 0.0_f32;
        for _ in 0..samples {
            let s = voice.next_sample();
            assert!(s.is_finite(), "voice produced a non-finite sample");
            peak = peak.max(s.abs());
            last = s;
        }
        (peak, last.abs())
    }

    #[test]
    fn voices_are_audible_bounded_and_decay() {
        let sr = 48_000.0;
        let two_seconds = 96_000;
        for (name, (peak, tail)) in [
            ("kick", render(&mut Kick::new(sr), two_seconds)),
            ("snare", render(&mut Snare::new(sr), two_seconds)),
            ("hat", render(&mut Hat::new(sr), two_seconds)),
            ("clap", render(&mut Clap::new(sr), two_seconds)),
            ("tom", render(&mut Tom::new(sr), two_seconds)),
        ] {
            assert!(peak > 0.05, "{name} is too quiet: {peak}");
            assert!(peak <= 1.5, "{name} is too loud: {peak}");
            assert!(tail < 1.0e-3, "{name} does not decay: {tail}");
        }
    }
}
