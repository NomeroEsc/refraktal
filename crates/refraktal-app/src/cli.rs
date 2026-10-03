// SPDX-License-Identifier: GPL-3.0-or-later
//! Text interface, kept for debugging the engine and the project model
//! without a GPU.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use refraktal_engine::{Command, DrumKind, EngineHandle, STEPS};
use refraktal_io::{Instrument, MAX_PATTERN_TRACKS, Project, Sharing, TrackData};

use crate::export;

pub fn run(handle: &mut EngineHandle) -> Result<()> {
    let mut cli = Cli { project: Project::demo(), handle };
    cli.apply_project();
    print_help();
    cli.print();

    let stdin = io::stdin();
    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // end of input
        }
        // Step events are not shown here; drain them so the queue stays fresh.
        while cli.handle.poll_event().is_some() {}
        cli.handle.collect_garbage();

        let words: Vec<&str> = line.split_whitespace().collect();
        if matches!(words.first(), Some(&("q" | "quit" | "exit"))) {
            break;
        }
        cli.command(&words);
    }
    Ok(())
}

/// The CLI's project; every change is also sent to the engine.
struct Cli<'a> {
    project: Project,
    handle: &'a mut EngineHandle,
}

impl Cli<'_> {
    fn send(&mut self, cmd: Command) {
        if self.handle.send(cmd).is_err() {
            eprintln!("engine queue is full, command dropped");
        }
    }

    fn pattern(&self) -> usize {
        self.project.selected_pattern
    }

    /// Rebuild the engine from the project, including samples.
    fn apply_project(&mut self) {
        for cmd in export::project_commands(&self.project) {
            self.send(cmd);
        }
        let paths: Vec<_> = self.project.tracks.iter().map(|t| t.sample.clone()).collect();
        for (track, path) in paths.into_iter().enumerate() {
            if let Some(path) = path {
                self.load(track, &path);
            }
        }
    }

    fn load(&mut self, track: usize, path: &Path) {
        match refraktal_io::load_sample(path) {
            Ok(sample) => {
                self.project.tracks[track].sample = Some(path.to_path_buf());
                self.send(Command::SetSample { track, sample: Some(Arc::new(sample)) });
                println!("loaded on {}", self.project.tracks[track].instrument.name());
            }
            Err(err) => println!("could not load: {err:#}"),
        }
    }

    /// A track of the current pattern by row number (1-based) or sound name.
    fn parse_track(&self, word: &str) -> Option<usize> {
        let rows = self.project.rows(self.pattern());
        word.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|row| rows.get(row).copied())
            .or_else(|| rows.into_iter().find(|&t| self.project.tracks[t].instrument.name() == word))
    }

    fn command(&mut self, words: &[&str]) {
        let arg = |i: usize| words.get(i).copied();
        match words.first().copied() {
            None => {}
            Some("p" | "play") => self.send(Command::Play),
            Some("s" | "stop") => self.send(Command::Stop),
            Some("b" | "bpm") => match arg(1).and_then(|w| w.parse::<f32>().ok()) {
                Some(bpm) => {
                    self.project.bpm = bpm;
                    self.project.bpm = export::project_bpm(&self.project);
                    self.send(Command::SetBpm(self.project.bpm));
                }
                None => println!("usage: bpm <number>, e.g. bpm 140"),
            },
            Some("t" | "toggle") => {
                let track = arg(1).and_then(|w| self.parse_track(w));
                let step = arg(2)
                    .and_then(|w| w.parse::<usize>().ok())
                    .filter(|s| (1..=STEPS).contains(s))
                    .map(|s| s - 1);
                match (track, step) {
                    (Some(track), Some(step)) => {
                        let pattern = self.pattern();
                        self.project.toggle_step(pattern, track, step);
                        self.send(Command::ToggleStep { pattern, track, step });
                        self.print();
                    }
                    _ => println!("usage: t <track> <1-{STEPS}>, e.g. 't snare 8' or 't 2 8'"),
                }
            }
            Some("add") => {
                let kind = arg(1).and_then(DrumKind::from_name).unwrap_or(DrumKind::Clap);
                let color = (self.project.tracks.len() % 8) as u8;
                let pattern = self.pattern();
                match self.project.add_track(pattern, TrackData::new(Instrument::drum(kind.name()), color)) {
                    Some(_) => {
                        self.send(Command::AddTrack(kind));
                        self.print();
                    }
                    None => println!("no room: a pattern shows at most {MAX_PATTERN_TRACKS} tracks"),
                }
            }
            Some("remove") => match arg(1).and_then(|w| self.parse_track(w)) {
                Some(track) => {
                    let pattern = self.pattern();
                    if self.project.remove_row(pattern, track) {
                        self.send(Command::RemoveTrack(track));
                    } else {
                        self.send(Command::SetRow { pattern, track, steps: [false; STEPS] });
                        println!("removed from this pattern; other patterns still use it");
                    }
                    self.print();
                }
                None => println!("usage: remove <track>"),
            },
            Some("sound") => {
                let track = arg(1).and_then(|w| self.parse_track(w));
                let kind = arg(2).and_then(DrumKind::from_name);
                match (track, kind) {
                    (Some(track), Some(kind)) => {
                        self.project.tracks[track].instrument = Instrument::drum(kind.name());
                        self.send(Command::SetDrum { track, kind });
                        self.send(Command::Trigger(track));
                        self.print();
                    }
                    _ => println!("usage: sound <track> <{}>", kind_names()),
                }
            }
            Some("load") => {
                let track = arg(1).and_then(|w| self.parse_track(w));
                let path = words.get(2..).map(|w| w.join(" ")).unwrap_or_default();
                match track {
                    Some(track) if !path.is_empty() => self.load(track, Path::new(&path)),
                    _ => println!("usage: load <track> <file>, e.g. load 1 C:/samples/kick.wav"),
                }
            }
            Some("unload") => match arg(1).and_then(|w| self.parse_track(w)) {
                Some(track) => {
                    self.project.tracks[track].sample = None;
                    self.send(Command::SetSample { track, sample: None });
                }
                None => println!("usage: unload <track>"),
            },
            Some("pat") => self.pattern_command(arg(1), arg(2)),
            Some("share") => match arg(1) {
                None => println!("tracks are {}", sharing_name(self.project.sharing)),
                Some(word) => {
                    let sharing = match word {
                        "shared" | "all" => Sharing::Shared,
                        "own" | "per-pattern" => Sharing::PerPattern,
                        _ => return println!("usage: share [shared|own]"),
                    };
                    // Switching never changes what plays, so the engine needs no update.
                    match self.project.set_sharing(sharing) {
                        Ok(()) => self.print(),
                        Err(err) => println!("{err}"),
                    }
                }
            },
            Some("save") => match arg(1) {
                Some(file) => match self.project.save(Path::new(file)) {
                    Ok(()) => println!("saved"),
                    Err(err) => println!("could not save: {err:#}"),
                },
                None => println!("usage: save <file.refraktal>"),
            },
            Some("open") => match arg(1).map(|f| Project::load(Path::new(f))) {
                Some(Ok(project)) => {
                    self.send(Command::Stop);
                    self.project = project;
                    self.apply_project();
                    self.print();
                }
                Some(Err(err)) => println!("could not open: {err:#}"),
                None => println!("usage: open <file.refraktal>"),
            },
            Some("export") => match arg(1) {
                Some(file) => match export::load_samples(&self.project) {
                    Ok(samples) => {
                        let rendered = export::render(&self.project, &samples);
                        match refraktal_io::save_wav(Path::new(file), &rendered.audio, export::EXPORT_SAMPLE_RATE) {
                            Ok(()) => println!("exported {:.1} s", rendered.seconds()),
                            Err(err) => println!("could not export: {err:#}"),
                        }
                    }
                    Err(err) => println!("could not load a sample: {err:#}"),
                },
                None => println!("usage: export <file.wav>"),
            },
            Some("show") => self.print(),
            Some("h" | "help") => print_help(),
            Some(other) => println!("unknown command: {other} (type 'help')"),
        }
    }

    fn pattern_command(&mut self, first: Option<&str>, second: Option<&str>) {
        let number = |w: Option<&str>| {
            w.and_then(|w| w.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1))
                .filter(|&i| i < self.project.patterns.len())
        };
        match first {
            None => {
                for (i, p) in self.project.patterns.iter().enumerate() {
                    let mark = if i == self.pattern() { '*' } else { ' ' };
                    println!(" {mark}{} {} ({} tracks)", i + 1, p.name, self.project.rows(i).len());
                }
            }
            Some("new") => match self.project.add_pattern() {
                Some(index) => self.select_pattern(index),
                None => println!("no room for another pattern"),
            },
            Some("del") => match number(second) {
                Some(index) => match self.project.remove_pattern(index) {
                    Some(removed) => {
                        self.send(Command::RemovePattern(index));
                        for track in removed {
                            self.send(Command::RemoveTrack(track));
                        }
                        self.select_pattern(self.pattern());
                    }
                    None => println!("the last pattern cannot be deleted"),
                },
                None => println!("usage: pat del <number>"),
            },
            Some(_) => match number(first) {
                Some(index) => self.select_pattern(index),
                None => println!("usage: pat [<number>|new|del <number>]"),
            },
        }
    }

    fn select_pattern(&mut self, index: usize) {
        self.project.selected_pattern = index;
        self.send(Command::SelectPattern(index));
        self.print();
    }

    fn print(&self) {
        let pattern = self.pattern();
        println!(
            "{} ({}/{}, tracks {})",
            self.project.patterns[pattern].name,
            pattern + 1,
            self.project.patterns.len(),
            sharing_name(self.project.sharing)
        );
        for (row, track) in self.project.rows(pattern).into_iter().enumerate() {
            let steps = self.project.steps(pattern, track).copied().unwrap_or_default();
            let cells: String = steps
                .chunks(4)
                .map(|beat| beat.iter().map(|&on| if on { 'x' } else { '.' }).collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            println!("  {} {:<6}| {cells}", row + 1, self.project.tracks[track].instrument.name());
        }
    }
}

fn sharing_name(sharing: Sharing) -> &'static str {
    match sharing {
        Sharing::Shared => "shared",
        Sharing::PerPattern => "own per pattern",
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
    println!("  remove <track>       remove a track from this pattern");
    println!("  sound <track> <s>    change a track's built-in sound");
    println!("  load <track> <file>  play a WAV/FLAC/MP3/OGG file on a track");
    println!("  unload <track>       back to the built-in sound");
    println!("  pat                  list patterns");
    println!("  pat <n> | new | del <n>  select, add or delete a pattern");
    println!("  share [shared|own]   show or switch: tracks in all patterns, or per pattern");
    println!("  save | open <file>   save or open a .refraktal project");
    println!("  export <file.wav>    render the current pattern to WAV");
    println!("  show                 print the pattern");
    println!("  q / quit             exit");
}
