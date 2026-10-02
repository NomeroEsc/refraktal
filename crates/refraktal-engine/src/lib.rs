// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-time audio engine for Refraktal.
//!
//! The engine is split into two halves:
//!
//! * [`Engine`] lives on the audio thread. [`Engine::process`] never
//!   allocates, locks or blocks.
//! * [`EngineHandle`] lives on the UI/control thread and talks to the engine
//!   through lock-free ring buffers.

use refraktal_dsp::{Hat, Kick, Snare, Voice};
use rtrb::{Consumer, Producer, RingBuffer};

/// Number of steps in a pattern (one bar of 16th notes).
pub const STEPS: usize = 16;
/// Number of drum tracks.
pub const TRACKS: usize = 3;
/// Human-readable track names, indexed like the pattern rows.
pub const TRACK_NAMES: [&str; TRACKS] = ["kick", "snare", "hat"];

const QUEUE_CAPACITY: usize = 256;
const MIN_BPM: f32 = 20.0;
const MAX_BPM: f32 = 999.0;

/// A pattern: one row of steps per track.
pub type Pattern = [[bool; STEPS]; TRACKS];

/// Messages from the control thread to the audio thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Play,
    Stop,
    SetBpm(f32),
    ToggleStep { track: usize, step: usize },
}

/// Messages from the audio thread back to the control thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The sequencer just played this step.
    Step(usize),
}

/// A simple four-on-the-floor starting beat.
#[must_use]
pub fn default_pattern() -> Pattern {
    let mut p = [[false; STEPS]; TRACKS];
    for step in (0..STEPS).step_by(4) {
        p[0][step] = true; // kick on every beat
    }
    p[1][4] = true; // snare on 2 and 4
    p[1][12] = true;
    for step in (0..STEPS).step_by(2) {
        p[2][step] = true; // 8th-note hats
    }
    p
}

/// Audio-thread half of the engine.
pub struct Engine {
    sample_rate: f32,
    bpm: f32,
    playing: bool,
    pattern: Pattern,
    step: usize,
    samples_until_step: f64,
    kick: Kick,
    snare: Snare,
    hat: Hat,
    gain: f32,
    commands: Consumer<Command>,
    events: Producer<Event>,
}

/// Control-thread half of the engine.
pub struct EngineHandle {
    commands: Producer<Command>,
    events: Consumer<Event>,
}

impl Engine {
    /// Create an engine and its control handle.
    #[must_use]
    pub fn new(sample_rate: f32) -> (Self, EngineHandle) {
        let (cmd_tx, cmd_rx) = RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx) = RingBuffer::new(QUEUE_CAPACITY);
        let engine = Self {
            sample_rate,
            bpm: 120.0,
            playing: false,
            pattern: default_pattern(),
            step: 0,
            samples_until_step: 0.0,
            kick: Kick::new(sample_rate),
            snare: Snare::new(sample_rate),
            hat: Hat::new(sample_rate),
            gain: 0.8,
            commands: cmd_rx,
            events: evt_tx,
        };
        let handle = EngineHandle { commands: cmd_tx, events: evt_rx };
        (engine, handle)
    }

    /// Render interleaved audio into `out`. Real-time safe.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }

        let channels = channels.max(1);
        for frame in out.chunks_exact_mut(channels) {
            if self.playing {
                if self.samples_until_step <= 0.0 {
                    self.fire_step();
                    self.samples_until_step += self.samples_per_step();
                }
                self.samples_until_step -= 1.0;
            }

            let mix = self.kick.next_sample() * 0.9
                + self.snare.next_sample() * 0.6
                + self.hat.next_sample() * 0.35;
            frame.fill((mix * self.gain).clamp(-1.0, 1.0));
        }
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play => {
                if !self.playing {
                    self.playing = true;
                    self.step = 0;
                    self.samples_until_step = 0.0;
                }
            }
            Command::Stop => self.playing = false,
            Command::SetBpm(bpm) => {
                if bpm.is_finite() {
                    self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
                }
            }
            Command::ToggleStep { track, step } => {
                if let Some(cell) = self.pattern.get_mut(track).and_then(|row| row.get_mut(step)) {
                    *cell = !*cell;
                }
            }
        }
    }

    fn fire_step(&mut self) {
        let step = self.step;
        if self.pattern[0][step] {
            self.kick.trigger(1.0);
        }
        if self.pattern[1][step] {
            self.snare.trigger(1.0);
        }
        if self.pattern[2][step] {
            // Accent hats on the beat.
            self.hat.trigger(if step % 4 == 0 { 1.0 } else { 0.6 });
        }
        // If the control thread is not reading, dropping events is fine.
        let _ = self.events.push(Event::Step(step));
        self.step = (step + 1) % STEPS;
    }

    fn samples_per_step(&self) -> f64 {
        // 4 steps per beat.
        f64::from(self.sample_rate) * 60.0 / f64::from(self.bpm) / 4.0
    }
}

impl EngineHandle {
    /// Send a command to the audio thread. Returns it back if the queue is full.
    pub fn send(&mut self, cmd: Command) -> Result<(), Command> {
        self.commands.push(cmd).map_err(|e| match e {
            rtrb::PushError::Full(cmd) => cmd,
        })
    }

    /// Take the next event from the audio thread, if any.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn silent_until_play() {
        let (mut engine, _handle) = Engine::new(SR);
        let mut buf = vec![0.0; 4096];
        engine.process(&mut buf, 2);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn play_advances_steps_and_makes_sound() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::Play).unwrap();

        // At 120 BPM a step is 6000 samples; render a bit more than two steps.
        let mut buf = vec![0.0; 13_000];
        engine.process(&mut buf, 1);

        assert!(buf.iter().any(|&s| s.abs() > 0.01), "no audio while playing");
        assert_eq!(handle.poll_event(), Some(Event::Step(0)));
        assert_eq!(handle.poll_event(), Some(Event::Step(1)));
        assert_eq!(handle.poll_event(), Some(Event::Step(2)));
        assert_eq!(handle.poll_event(), None);
    }

    #[test]
    fn invalid_commands_are_ignored() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::ToggleStep { track: 99, step: 99 }).unwrap();
        handle.send(Command::SetBpm(f32::NAN)).unwrap();
        let mut buf = vec![0.0; 64];
        engine.process(&mut buf, 2);
        assert_eq!(engine.pattern, default_pattern());
        assert!((engine.bpm - 120.0).abs() < f32::EPSILON);
    }
}
