// SPDX-License-Identifier: GPL-3.0-or-later
//! Text interface, kept for debugging the engine without a GPU.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use refraktal_engine::{
    Command, DEFAULT_TRACKS, DrumKind, EngineHandle, MAX_TRACKS, Pattern, STEPS, default_pattern,
};

/// The CLI's mirror of the project; every change is also sent to the engine.
struct State {
    pattern: Pattern,
    tracks: Vec<DrumKind>,
}

pub fn run(handle: &mut EngineHandle) -> Result<()> {
    let mut state = State { pattern: default_pattern(), tracks: DEFAULT_TRACKS.to_vec() };
    print_help();
    state.print();

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // end of input
        }
        // Step events are not shown here; drain them so the queue stays fresh.
        while handle.poll_event().is_some() {}
        handle.collect_garbage();

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
                let track = words.next().and_then(|w| state.parse_track(w));
                let step = words
                    .next()
                    .and_then(|w| w.parse::<usize>().ok())
                    .filter(|s| (1..=STEPS).contains(s))
                    .map(|s| s - 1);
                match (track, step) {
                    (Some(track), Some(step)) => {
                        state.pattern[track][step] = !state.pattern[track][step];
                        send(handle, Command::ToggleStep { track, step });
                        state.print();
                    }
                    _ => println!("usage: t <track> <1-{STEPS}>, e.g. 't snare 8' or 't 2 8'"),
                }
            }
            Some("add") => {
                let kind = words.next().and_then(DrumKind::from_name).unwrap_or(DrumKind::Clap);
                if state.tracks.len() < MAX_TRACKS {
                    state.tracks.push(kind);
                    state.pattern[state.tracks.len() - 1] = [false; STEPS];
                    send(handle, Command::AddTrack(kind));
                    state.print();
                } else {
                    println!("at most {MAX_TRACKS} tracks");
                }
            }
            Some("remove") => match words.next().and_then(|w| state.parse_track(w)) {
                Some(track) if state.tracks.len() > 1 => {
                    let count = state.tracks.len();
                    state.tracks.remove(track);
                    state.pattern[track..count].rotate_left(1);
                    state.pattern[count - 1] = [false; STEPS];
                    send(handle, Command::RemoveTrack(track));
                    state.print();
                }
                Some(_) => println!("the last track cannot be removed"),
                None => println!("usage: remove <track>"),
            },
            Some("sound") => {
                let track = words.next().and_then(|w| state.parse_track(w));
                let kind = words.next().and_then(DrumKind::from_name);
                match (track, kind) {
                    (Some(track), Some(kind)) => {
                        state.tracks[track] = kind;
                        send(handle, Command::SetDrum { track, kind });
                        send(handle, Command::Trigger(track));
                        state.print();
                    }
                    _ => println!("usage: sound <track> <{}>", kind_names()),
                }
            }
            Some("load") => {
                let track = words.next().and_then(|w| state.parse_track(w));
                let path = words.collect::<Vec<_>>().join(" ");
                match track {
                    Some(track) if !path.is_empty() => match refraktal_io::load_sample(Path::new(&path)) {
                        Ok(sample) => {
                            send(handle, Command::SetSample { track, sample: Some(Arc::new(sample)) });
                            println!("loaded on track {}", track + 1);
                        }
                        Err(err) => println!("could not load: {err:#}"),
                    },
                    _ => println!("usage: load <track> <file>, e.g. load 1 C:/samples/kick.wav"),
                }
            }
            Some("unload") => match words.next().and_then(|w| state.parse_track(w)) {
                Some(track) => send(handle, Command::SetSample { track, sample: None }),
                None => println!("usage: unload <track>"),
            },
            Some("show") => state.print(),
            Some("h" | "help") => print_help(),
            Some(other) => println!("unknown command: {other} (type 'help')"),
        }
    }
    Ok(())
}

impl State {
    /// A track by number (1-based) or by the name of its sound.
    fn parse_track(&self, word: &str) -> Option<usize> {
        word.parse::<usize>()
            .ok()
            .filter(|n| (1..=self.tracks.len()).contains(n))
            .map(|n| n - 1)
            .or_else(|| self.tracks.iter().position(|k| k.name() == word))
    }

    fn print(&self) {
        for (i, (kind, row)) in self.tracks.iter().zip(self.pattern.iter()).enumerate() {
            let cells: String = row
                .chunks(4)
                .map(|beat| beat.iter().map(|&on| if on { 'x' } else { '.' }).collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            println!("  {} {:<6}| {cells}", i + 1, kind.name());
        }
    }
}

fn send(handle: &mut EngineHandle, cmd: Command) {
    if handle.send(cmd).is_err() {
        eprintln!("engine queue is full, command dropped");
    }
}

fn kind_names() -> String {
    DrumKind::ALL.iter().map(|k| k.name()).collect::<Vec<_>>().join("|")
}

fn print_help() {
    println!("Commands:");
    println!("  p / play             start the pattern");
    println!("  s / stop             stop");
    println!("  b / bpm <n>          set tempo, e.g. 'bpm 140'");
    println!("  t <track> <step>     toggle a step, e.g. 't snare 8' or 't 2 8'");
    println!("  add [sound]          add a track ({})", kind_names());
    println!("  remove <track>       remove a track");
    println!("  sound <track> <s>    change a track's built-in sound");
    println!("  load <track> <file>  play a WAV/FLAC/MP3/OGG file on a track");
    println!("  unload <track>       back to the built-in sound");
    println!("  show                 print the pattern");
    println!("  q / quit             exit");
}
