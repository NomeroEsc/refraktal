// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing rendered audio as 16-bit PCM WAV.
//!
//! The file carries no metadata: no software name, dates or paths.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use refraktal_dsp::Noise;

/// Encode interleaved stereo `f32` audio as a 16-bit stereo WAV file.
///
/// Values outside `-1.0..=1.0` are clipped. TPDF dither is added before
/// rounding so quiet tails fade out smoothly instead of turning into
/// distortion. The dither uses a fixed seed: the same audio always gives
/// the same file.
pub fn encode_wav(interleaved: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    ensure!(interleaved.len().is_multiple_of(2), "stereo audio must have an even number of samples");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // 44-byte header plus two bytes per sample.
    let mut bytes = Cursor::new(Vec::with_capacity(44 + interleaved.len() * 2));
    {
        let mut writer = hound::WavWriter::new(&mut bytes, spec).context("could not start the WAV file")?;
        let mut a = Noise::new(0x5EED_0001);
        let mut b = Noise::new(0x5EED_0002);
        let mut out = writer.get_i16_writer(interleaved.len() as u32);
        for &x in interleaved {
            // Two uniform values of ±0.5 LSB add up to triangular ±1 LSB noise.
            let dither = 0.5 * (a.sample() + b.sample());
            let x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            let value = (x * f32::from(i16::MAX) + dither).round();
            out.write_sample(value.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16);
        }
        out.flush().context("could not write the audio data")?;
        writer.finalize().context("could not finish the WAV file")?;
    }
    Ok(bytes.into_inner())
}

/// Encode and write a WAV file. Like project saving, this writes a
/// temporary file first so a crash never leaves a half-written file.
pub fn save_wav(path: &Path, interleaved: &[f32], sample_rate: u32) -> Result<()> {
    let bytes = encode_wav(interleaved, sample_rate)?;
    let tmp = path.with_extension("wav.tmp");
    fs::write(&tmp, bytes).with_context(|| format!("could not write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("could not save {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> (hound::WavSpec, Vec<i16>) {
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        let samples = reader.into_samples::<i16>().map(Result::unwrap).collect();
        (spec, samples)
    }

    #[test]
    fn header_and_length() {
        let audio = vec![0.25_f32; 2 * 1000];
        let bytes = encode_wav(&audio, 48_000).unwrap();
        let (spec, samples) = decode(&bytes);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(samples.len(), 2000);
        // 0.25 of full scale, give or take the dither.
        assert!(samples.iter().all(|&s| (i32::from(s) - 8192).abs() <= 1), "{:?}", &samples[..8]);
    }

    #[test]
    fn clips_and_survives_bad_values() {
        let audio = [2.0, -2.0, f32::NAN, f32::INFINITY];
        let (_, samples) = decode(&encode_wav(&audio, 44_100).unwrap());
        assert!(samples[0] >= i16::MAX - 1);
        assert!(samples[1] <= i16::MIN + 2);
        assert!(samples[2].abs() <= 1);
        assert!(samples[3].abs() <= 1);
    }

    #[test]
    fn same_audio_same_file() {
        let audio: Vec<f32> = (0..4000).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        assert_eq!(encode_wav(&audio, 48_000).unwrap(), encode_wav(&audio, 48_000).unwrap());
    }

    #[test]
    fn rejects_odd_sample_counts() {
        assert!(encode_wav(&[0.0; 3], 48_000).is_err());
    }
}
