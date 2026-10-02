// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio output: opens the default device and runs the engine on it.

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use refraktal_engine::{Engine, EngineHandle};

/// Largest block rendered at once; bigger host buffers are split into chunks.
const MAX_BLOCK_FRAMES: usize = 4096;

/// A running audio stream. Audio stops when this is dropped.
pub struct Audio {
    _stream: cpal::Stream,
    pub handle: EngineHandle,
}

/// Open the default output device and start the engine.
///
/// With `verbose`, print the device name and format. It is off by default
/// because the OS reports device names in the system language.
pub fn start(verbose: bool) -> Result<Audio> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no audio output device found")?;
    let supported = device
        .default_output_config()
        .context("could not query the default output config")?;

    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();

    if verbose {
        let name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "unknown device".to_owned());
        println!(
            "Output: {name}, {} Hz, {} ch, {sample_format}",
            config.sample_rate, config.channels
        );
    }

    let (engine, handle) = Engine::new(config.sample_rate as f32);
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, engine)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config, engine)?,
        SampleFormat::U16 => build_stream::<u16>(&device, config, engine)?,
        SampleFormat::I32 => build_stream::<i32>(&device, config, engine)?,
        other => bail!("unsupported sample format: {other}"),
    };
    stream.play().context("could not start the audio stream")?;

    Ok(Audio { _stream: stream, handle })
}

fn build_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut engine: Engine,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    // Allocated here, before the stream starts, never on the audio thread.
    let mut scratch = vec![0.0_f32; MAX_BLOCK_FRAMES * channels];

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _info| {
            assert_no_alloc::assert_no_alloc(|| {
                for chunk in data.chunks_mut(scratch.len()) {
                    let block = &mut scratch[..chunk.len()];
                    engine.process(block, channels);
                    for (out, &sample) in chunk.iter_mut().zip(block.iter()) {
                        *out = T::from_sample(sample);
                    }
                }
            });
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;
    Ok(stream)
}
