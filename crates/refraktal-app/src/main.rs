// SPDX-License-Identifier: GPL-3.0-or-later
//! Refraktal command-line prototype.
//!
//! Plays a 16-step drum pattern and lets you edit it from the terminal.
//! This is a temporary front end for testing the engine; the real UI comes
//! later.

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use refraktal_engine::{
    Command, Engine, EngineHandle, Pattern, STEPS, TRACK_NAMES, TRACKS, default_pattern,
};

// In debug builds, abort if anything allocates inside the audio callback.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Largest block rendered at once; bigger host buffers are split into chunks.
const MAX_BLOCK_FRAMES: usize = 4096;

fn main() -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no audio output device found")?;
    let supported = device
        .default_output_config()
        .context("could not query the default output config")?;

    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let device_name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "unknown device".to_owned());

    println!("Refraktal prototype");
    println!(
        "Output: {device_name}, {} Hz, {} ch, {sample_format}",
        config.sample_rate, config.channels
    );

    let (engine, mut handle) = Engine::new(config.sample_rate as f32);
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config, engine)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config, engine)?,
        SampleFormat::U16 => build_stream::<u16>(&device, config, engine)?,
        SampleFormat::I32 => build_stream::<i32>(&device, config, engine)?,
        other => bail!("unsupported sample format: {other}"),
    };
    stream.play().context("could not start the audio stream")?;

    run_repl(&mut handle)
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

fn run_repl(handle: &mut EngineHandle) -> Result<()> {
    // The UI keeps its own copy of the pattern and mirrors every change
    // it sends to the engine.
    let mut pattern = default_pattern();
    print_help();
    print_pattern(&pattern);

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // end of input
        }
        // Step events are not shown in the CLI yet; drain them so the
        // queue stays fresh.
        while handle.poll_event().is_some() {}

        let mut words = line.split_whitespace();
        match words.next() {
            None => {}
            Some("q" | "quit" | "exit") => break,
            Some("p" | "play") => send(handle, Command::Play),
            Some("s" | "stop") => send(handle, Command::Stop),
            Some("b" | "bpm") => match words.next().and_then(|w| w.parse::<f32>().ok()) {
                Some(bpm) => send(handle, Command::SetBpm(bpm)),
                None => println!("usage: bpm <number>, e.g. bpm 140"),
            },
            Some("t" | "toggle") => {
                let track = words.next().and_then(parse_track);
                let step = words
                    .next()
                    .and_then(|w| w.parse::<usize>().ok())
                    .filter(|s| (1..=STEPS).contains(s))
                    .map(|s| s - 1);
                match (track, step) {
                    (Some(track), Some(step)) => {
                        pattern[track][step] = !pattern[track][step];
                        send(handle, Command::ToggleStep { track, step });
                        print_pattern(&pattern);
                    }
                    _ => println!("usage: t <kick|snare|hat|1-{TRACKS}> <1-{STEPS}>"),
                }
            }
            Some("show") => print_pattern(&pattern),
            Some("h" | "help") => print_help(),
            Some(other) => println!("unknown command: {other} (type 'help')"),
        }
    }
    Ok(())
}

fn send(handle: &mut EngineHandle, cmd: Command) {
    if handle.send(cmd).is_err() {
        eprintln!("engine queue is full, command dropped");
    }
}

fn parse_track(word: &str) -> Option<usize> {
    TRACK_NAMES.iter().position(|&name| name == word).or_else(|| {
        word.parse::<usize>()
            .ok()
            .filter(|n| (1..=TRACKS).contains(n))
            .map(|n| n - 1)
    })
}

fn print_pattern(pattern: &Pattern) {
    for (name, row) in TRACK_NAMES.iter().zip(pattern.iter()) {
        let cells: String = row
            .chunks(4)
            .map(|beat| beat.iter().map(|&on| if on { 'x' } else { '.' }).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ");
        println!("  {name:<6}| {cells}");
    }
}

fn print_help() {
    println!("Commands:");
    println!("  p / play          start the pattern");
    println!("  s / stop          stop");
    println!("  b / bpm <n>       set tempo, e.g. 'bpm 140'");
    println!("  t <track> <step>  toggle a step, e.g. 't snare 8' or 't 1 3'");
    println!("  show              print the pattern");
    println!("  q / quit          exit");
}
