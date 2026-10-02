// SPDX-License-Identifier: GPL-3.0-or-later
//! Sample playback.

/// Decoded audio, always stored as stereo frames.
#[derive(Debug)]
pub struct Sample {
    frames: Box<[[f32; 2]]>,
    sample_rate: f32,
}

impl Sample {
    /// Build from interleaved samples with `channels` channels.
    /// Mono is duplicated to both sides; channels beyond two are dropped.
    #[must_use]
    pub fn from_interleaved(data: &[f32], channels: usize, sample_rate: f32) -> Self {
        let channels = channels.max(1);
        let frames = data
            .chunks_exact(channels)
            .map(|f| if channels == 1 { [f[0], f[0]] } else { [f[0], f[1]] })
            .collect();
        Self { frames, sample_rate }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    #[must_use]
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Linearly interpolated stereo frame at a fractional position.
    #[inline]
    fn frame_at(&self, pos: f64) -> Option<[f32; 2]> {
        let i = pos as usize;
        let a = *self.frames.get(i)?;
        let b = self.frames.get(i + 1).copied().unwrap_or([0.0, 0.0]);
        let t = (pos - i as f64) as f32;
        Some([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
    }
}

/// Plays one sample from start to end. Allocation-free.
///
/// The voice does not own the sample; the caller passes it to [`render`]
/// so that swapping samples never happens inside the voice.
///
/// [`render`]: SampleVoice::render
#[derive(Clone, Debug, Default)]
pub struct SampleVoice {
    pos: f64,
    rate: f64,
    gain: f32,
    fade_step: f32,
    active: bool,
}

impl SampleVoice {
    /// Start playing from the beginning.
    pub fn trigger(&mut self, sample: &Sample, output_rate: f32, velocity: f32) {
        self.pos = 0.0;
        self.rate = f64::from(sample.sample_rate()) / f64::from(output_rate.max(1.0));
        self.gain = velocity.clamp(0.0, 1.0);
        self.fade_step = 0.0;
        self.active = true;
    }

    /// Fade out over `seconds` instead of stopping abruptly (avoids clicks).
    pub fn release(&mut self, seconds: f32, output_rate: f32) {
        if self.active {
            self.fade_step = self.gain / (seconds * output_rate).max(1.0);
        }
    }

    /// Stop immediately.
    pub fn stop(&mut self) {
        self.active = false;
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Render the next stereo frame.
    #[inline]
    pub fn render(&mut self, sample: &Sample) -> [f32; 2] {
        if !self.active {
            return [0.0, 0.0];
        }
        let Some(frame) = sample.frame_at(self.pos) else {
            self.active = false;
            return [0.0, 0.0];
        };
        let out = [frame[0] * self.gain, frame[1] * self.gain];
        self.pos += self.rate;
        if self.fade_step > 0.0 {
            self.gain -= self.fade_step;
            if self.gain <= 0.0 {
                self.active = false;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_duplicated() {
        let s = Sample::from_interleaved(&[0.5, -0.5], 1, 48_000.0);
        assert_eq!(s.len(), 2);
        assert_eq!(s.frame_at(0.0), Some([0.5, 0.5]));
    }

    #[test]
    fn plays_to_the_end_then_stops() {
        let s = Sample::from_interleaved(&[1.0, 1.0, 1.0, 1.0], 2, 48_000.0);
        let mut v = SampleVoice::default();
        v.trigger(&s, 48_000.0, 1.0);
        assert_eq!(v.render(&s), [1.0, 1.0]);
        assert!(v.is_active());
        let _ = v.render(&s);
        assert_eq!(v.render(&s), [0.0, 0.0]);
        assert!(!v.is_active());
    }

    #[test]
    fn resamples_by_rate() {
        // A 24 kHz sample at 48 kHz output advances half a frame per output frame.
        let s = Sample::from_interleaved(&[0.0, 0.0, 1.0, 1.0], 2, 24_000.0);
        let mut v = SampleVoice::default();
        v.trigger(&s, 48_000.0, 1.0);
        assert_eq!(v.render(&s), [0.0, 0.0]);
        assert_eq!(v.render(&s), [0.5, 0.5]);
    }

    #[test]
    fn release_fades_out() {
        let s = Sample::from_interleaved(&vec![1.0; 2000], 2, 1000.0);
        let mut v = SampleVoice::default();
        v.trigger(&s, 1000.0, 1.0);
        v.release(0.01, 1000.0); // 10 frames
        for _ in 0..12 {
            let _ = v.render(&s);
        }
        assert!(!v.is_active());
    }
}
