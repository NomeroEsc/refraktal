// SPDX-License-Identifier: GPL-3.0-or-later
//! Decoding audio files into [`Sample`]s.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, bail};
use refraktal_dsp::Sample;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

/// Longer files are rejected to keep memory use reasonable.
pub const MAX_SAMPLE_SECONDS: f32 = 60.0;

/// Decode a WAV, FLAC, MP3 or OGG/Vorbis file.
pub fn load_sample(path: &Path) -> Result<Sample> {
    let file = File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .context("unsupported or damaged audio file")?;

    let track = format
        .default_track(TrackType::Audio)
        .context("the file has no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .context("the file has no audio parameters")?
        .clone();
    let sample_rate = params.sample_rate.context("unknown sample rate")?;
    let max_frames = (MAX_SAMPLE_SECONDS * sample_rate as f32) as usize;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .context("unsupported audio codec")?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut packet_buf: Vec<f32> = Vec::new();
    let mut channels = 0;

    while let Some(packet) = format.next_packet().context("could not read the file")? {
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // A corrupt packet is skipped; the rest of the file may still play.
            Err(DecodeError::DecodeError(_)) => continue,
            Err(err) => return Err(err).context("could not decode the file"),
        };
        channels = decoded.spec().channels().count();
        decoded.copy_to_vec_interleaved(&mut packet_buf);
        interleaved.extend_from_slice(&packet_buf);

        if channels > 0 && interleaved.len() / channels > max_frames {
            bail!("the file is longer than {MAX_SAMPLE_SECONDS} seconds");
        }
    }

    if channels == 0 || interleaved.is_empty() {
        bail!("the file contains no audio");
    }
    Ok(Sample::from_interleaved(&interleaved, channels, sample_rate as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, channels: u16, frames: usize) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames * channels as usize {
            writer.write_sample(((i % 100) as i16 - 50) * 300).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn loads_mono_and_stereo_wav() {
        let dir = std::env::temp_dir();
        for channels in [1_u16, 2] {
            let path = dir.join(format!("refraktal-test-{channels}ch-{}.wav", std::process::id()));
            write_wav(&path, channels, 4410);
            let sample = load_sample(&path).unwrap();
            std::fs::remove_file(&path).ok();
            assert_eq!(sample.len(), 4410);
            assert!((sample.sample_rate() - 44_100.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn rejects_non_audio() {
        let path = std::env::temp_dir().join(format!("refraktal-test-{}.wav", std::process::id()));
        std::fs::write(&path, b"definitely not audio").unwrap();
        assert!(load_sample(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
