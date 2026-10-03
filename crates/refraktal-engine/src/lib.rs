// SPDX-License-Identifier: GPL-3.0-or-later
//! Real-time audio engine for Refraktal.
//!
//! The engine is split into two halves:
//!
//! * [`Engine`] lives on the audio thread. [`Engine::process`] never
//!   allocates, frees, locks or blocks.
//! * [`EngineHandle`] lives on the UI/control thread and talks to the engine
//!   through lock-free ring buffers.
//!
//! The engine knows nothing about files or the editing model: it holds
//! [`MAX_TRACKS`] preallocated track slots and [`MAX_PATTERNS`] step grids,
//! and plays the selected pattern in a loop.
//!
//! Samples are shared as `Arc<Sample>`. When the engine replaces a sample it
//! hands the old one back through a queue, so the memory is always freed on
//! the control thread (see [`EngineHandle::collect_garbage`]).

use std::sync::Arc;

use refraktal_dsp::{Clap, Hat, Kick, Sample, SampleVoice, Snare, Tom, Voice};
use rtrb::{Consumer, Producer, RingBuffer};

/// Number of steps in a pattern (one bar of 16th notes).
pub const STEPS: usize = 16;
/// Most tracks a project can have. Slots are preallocated so adding a
/// track never allocates on the audio thread.
pub const MAX_TRACKS: usize = 32;
/// Most patterns a project can have, also preallocated.
pub const MAX_PATTERNS: usize = 16;

// Large enough to load a whole project in one go: one `SetRow` per track
// and pattern (32 × 16) plus the tracks themselves.
const QUEUE_CAPACITY: usize = 2048;
const RETIRE_CAPACITY: usize = 64;
const MIN_BPM: f32 = 20.0;
const MAX_BPM: f32 = 999.0;
const SAMPLE_GAIN: f32 = 0.8;
/// Fade applied to a sample that is cut off by the next hit.
const RETRIGGER_FADE_SECONDS: f32 = 0.003;

/// Steps of one track in one pattern.
pub type Steps = [bool; STEPS];
/// One pattern: a row of steps per track slot. Rows past the track count
/// are always empty.
type Grid = [Steps; MAX_TRACKS];

/// Built-in drum sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumKind {
    Kick,
    Snare,
    Hat,
    Clap,
    Tom,
}

impl DrumKind {
    pub const ALL: [Self; 5] = [Self::Kick, Self::Snare, Self::Hat, Self::Clap, Self::Tom];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Kick => "kick",
            Self::Snare => "snare",
            Self::Hat => "hat",
            Self::Clap => "clap",
            Self::Tom => "tom",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The next sound in [`DrumKind::ALL`], wrapping around.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn gain(self) -> f32 {
        match self {
            Self::Kick => 0.9,
            Self::Snare => 0.6,
            Self::Hat => 0.35,
            Self::Clap => 0.55,
            Self::Tom => 0.7,
        }
    }
}

/// Messages from the control thread to the audio thread.
#[derive(Debug)]
pub enum Command {
    Play,
    Stop,
    SetBpm(f32),
    ToggleStep { pattern: usize, track: usize, step: usize },
    SetStep { pattern: usize, track: usize, step: usize, on: bool },
    /// Replace all steps of one track in one pattern (used to load projects).
    SetRow { pattern: usize, track: usize, steps: Steps },
    /// Start over with no tracks and empty patterns, e.g. before loading a project.
    Reset,
    /// Add a track at the end, if there is room. It starts with no steps.
    AddTrack(DrumKind),
    /// Remove a track from every pattern; the ones below move up.
    RemoveTrack(usize),
    /// Play this pattern. While playing, the switch waits for the end of
    /// the current bar so the beat never stumbles.
    SelectPattern(usize),
    /// Delete a pattern's steps; later patterns move up by one.
    RemovePattern(usize),
    /// Change a track's built-in sound.
    SetDrum { track: usize, kind: DrumKind },
    /// Play a track once right now (to audition a sound).
    Trigger(usize),
    /// Play a sample on this track instead of its built-in synth.
    /// `None` switches back to the synth.
    SetSample { track: usize, sample: Option<Arc<Sample>> },
}

/// Messages from the audio thread back to the control thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The sequencer just played this step.
    Step(usize),
    /// This pattern is now playing (after `SelectPattern` took effect).
    Pattern(usize),
}

/// Built-in synth voice of a track.
enum Drum {
    Kick(Kick),
    Snare(Snare),
    Hat(Hat),
    Clap(Clap),
    Tom(Tom),
}

impl Drum {
    fn new(kind: DrumKind, sample_rate: f32) -> Self {
        match kind {
            DrumKind::Kick => Self::Kick(Kick::new(sample_rate)),
            DrumKind::Snare => Self::Snare(Snare::new(sample_rate)),
            DrumKind::Hat => Self::Hat(Hat::new(sample_rate)),
            DrumKind::Clap => Self::Clap(Clap::new(sample_rate)),
            DrumKind::Tom => Self::Tom(Tom::new(sample_rate)),
        }
    }

    fn voice(&mut self) -> &mut dyn Voice {
        match self {
            Self::Kick(v) => v,
            Self::Snare(v) => v,
            Self::Hat(v) => v,
            Self::Clap(v) => v,
            Self::Tom(v) => v,
        }
    }
}

struct Track {
    kind: DrumKind,
    drum: Drum,
    sample: Option<Arc<Sample>>,
    /// Two voices so a retriggered sample can fade out while the new hit starts.
    voices: [SampleVoice; 2],
    next_voice: usize,
}

impl Track {
    fn new(kind: DrumKind, sample_rate: f32) -> Self {
        Self {
            kind,
            drum: Drum::new(kind, sample_rate),
            sample: None,
            voices: [SampleVoice::default(), SampleVoice::default()],
            next_voice: 0,
        }
    }

    fn trigger(&mut self, velocity: f32, sample_rate: f32) {
        match &self.sample {
            Some(sample) => {
                let previous = 1 - self.next_voice;
                self.voices[previous].release(RETRIGGER_FADE_SECONDS, sample_rate);
                self.voices[self.next_voice].trigger(sample, sample_rate, velocity);
                self.next_voice = previous;
            }
            None => self.drum.voice().trigger(velocity),
        }
    }

    #[inline]
    fn render(&mut self) -> [f32; 2] {
        // The synth keeps rendering so a tail can ring out after switching.
        let s = self.drum.voice().next_sample() * self.kind.gain();
        let mut out = [s, s];
        if let Some(sample) = &self.sample {
            for voice in &mut self.voices {
                let [l, r] = voice.render(sample);
                out[0] += l * SAMPLE_GAIN;
                out[1] += r * SAMPLE_GAIN;
            }
        }
        out
    }
}

/// Audio-thread half of the engine.
pub struct Engine {
    sample_rate: f32,
    bpm: f32,
    playing: bool,
    patterns: Box<[Grid; MAX_PATTERNS]>,
    pattern: usize,
    /// Pattern to switch to at the start of the next bar.
    next_pattern: Option<usize>,
    step: usize,
    samples_until_step: f64,
    tracks: [Track; MAX_TRACKS],
    track_count: usize,
    gain: f32,
    commands: Consumer<Command>,
    events: Producer<Event>,
    retired: Producer<Arc<Sample>>,
}

/// Control-thread half of the engine.
pub struct EngineHandle {
    commands: Producer<Command>,
    events: Consumer<Event>,
    retired: Consumer<Arc<Sample>>,
}

impl Engine {
    /// Create an engine and its control handle.
    #[must_use]
    pub fn new(sample_rate: f32) -> (Self, EngineHandle) {
        let (cmd_tx, cmd_rx) = RingBuffer::new(QUEUE_CAPACITY);
        let (evt_tx, evt_rx) = RingBuffer::new(QUEUE_CAPACITY);
        let (ret_tx, ret_rx) = RingBuffer::new(RETIRE_CAPACITY);
        let engine = Self {
            sample_rate,
            bpm: 120.0,
            playing: false,
            patterns: Box::new([[[false; STEPS]; MAX_TRACKS]; MAX_PATTERNS]),
            pattern: 0,
            next_pattern: None,
            step: 0,
            samples_until_step: 0.0,
            tracks: std::array::from_fn(|_| Track::new(DrumKind::Kick, sample_rate)),
            track_count: 0,
            gain: 0.8,
            commands: cmd_rx,
            events: evt_tx,
            retired: ret_tx,
        };
        let handle = EngineHandle { commands: cmd_tx, events: evt_rx, retired: ret_rx };
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

            let mut mix = [0.0_f32; 2];
            for track in &mut self.tracks[..self.track_count] {
                let [l, r] = track.render();
                mix[0] += l;
                mix[1] += r;
            }
            let l = (mix[0] * self.gain).clamp(-1.0, 1.0);
            let r = (mix[1] * self.gain).clamp(-1.0, 1.0);

            if channels == 1 {
                frame[0] = 0.5 * (l + r);
            } else {
                frame[0] = l;
                frame[1] = r;
                frame[2..].fill(0.0);
            }
        }
    }

    /// Frames that [`process`](Self::process) will render before the next
    /// step fires, or `None` when stopped. An offline renderer uses this to
    /// stop exactly at a pattern boundary. Commands still waiting in the
    /// queue are not taken into account.
    #[must_use]
    pub fn frames_until_step(&self) -> Option<usize> {
        // `process` fires on the first frame whose counter is <= 0 and then
        // counts down by one per frame, so the answer is ceil(counter).
        self.playing.then(|| {
            if self.samples_until_step <= 0.0 { 0 } else { self.samples_until_step.ceil() as usize }
        })
    }

    /// Index of the step that fires next.
    #[must_use]
    pub fn next_step(&self) -> usize {
        self.step
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play => {
                if !self.playing {
                    self.playing = true;
                    self.step = 0;
                    self.samples_until_step = 0.0;
                    self.switch_pattern();
                }
            }
            Command::Stop => self.playing = false,
            Command::SetBpm(bpm) => {
                if bpm.is_finite() {
                    self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
                }
            }
            Command::ToggleStep { pattern, track, step } => {
                if let Some(cell) = self.cell(pattern, track, step) {
                    *cell = !*cell;
                }
            }
            Command::SetStep { pattern, track, step, on } => {
                if let Some(cell) = self.cell(pattern, track, step) {
                    *cell = on;
                }
            }
            Command::SetRow { pattern, track, steps } => {
                if track < self.track_count {
                    if let Some(grid) = self.patterns.get_mut(pattern) {
                        grid[track] = steps;
                    }
                }
            }
            Command::Reset => {
                for track in &mut self.tracks {
                    let old = track.sample.take();
                    retire(&mut self.retired, old);
                }
                self.track_count = 0;
                for grid in self.patterns.iter_mut() {
                    *grid = [[false; STEPS]; MAX_TRACKS];
                }
                self.pattern = 0;
                self.next_pattern = None;
            }
            Command::SetSample { track, sample } => {
                if track < self.track_count {
                    let t = &mut self.tracks[track];
                    for voice in &mut t.voices {
                        voice.stop();
                    }
                    let old = std::mem::replace(&mut t.sample, sample);
                    retire(&mut self.retired, old);
                } else {
                    retire(&mut self.retired, sample);
                }
            }
            Command::AddTrack(kind) => {
                if self.track_count < MAX_TRACKS {
                    let index = self.track_count;
                    let old = self.tracks[index].sample.take();
                    retire(&mut self.retired, old);
                    self.tracks[index] = Track::new(kind, self.sample_rate);
                    for grid in self.patterns.iter_mut() {
                        grid[index] = [false; STEPS];
                    }
                    self.track_count += 1;
                }
            }
            Command::RemoveTrack(track) => {
                if track < self.track_count {
                    let old = self.tracks[track].sample.take();
                    retire(&mut self.retired, old);
                    // Move the removed slot to the end; nothing is allocated or freed.
                    let count = self.track_count;
                    self.tracks[track..count].rotate_left(1);
                    for grid in self.patterns.iter_mut() {
                        grid[track..count].rotate_left(1);
                        grid[count - 1] = [false; STEPS];
                    }
                    self.track_count -= 1;
                }
            }
            Command::SelectPattern(pattern) => {
                if pattern < MAX_PATTERNS {
                    self.next_pattern = Some(pattern);
                    if !self.playing {
                        self.switch_pattern();
                    }
                }
            }
            Command::RemovePattern(pattern) => {
                if pattern < MAX_PATTERNS {
                    self.patterns[pattern..].rotate_left(1);
                    self.patterns[MAX_PATTERNS - 1] = [[false; STEPS]; MAX_TRACKS];
                    self.pattern = after_removal(self.pattern, pattern);
                    self.next_pattern = self.next_pattern.map(|p| after_removal(p, pattern));
                }
            }
            Command::SetDrum { track, kind } => {
                if track < self.track_count {
                    let t = &mut self.tracks[track];
                    t.kind = kind;
                    t.drum = Drum::new(kind, self.sample_rate);
                }
            }
            Command::Trigger(track) => {
                if track < self.track_count {
                    self.tracks[track].trigger(1.0, self.sample_rate);
                }
            }
        }
    }

    fn cell(&mut self, pattern: usize, track: usize, step: usize) -> Option<&mut bool> {
        if track >= self.track_count {
            return None;
        }
        self.patterns.get_mut(pattern)?[track].get_mut(step)
    }

    /// Apply a pending `SelectPattern`.
    fn switch_pattern(&mut self) {
        if let Some(next) = self.next_pattern.take() {
            if next != self.pattern {
                self.pattern = next;
                let _ = self.events.push(Event::Pattern(next));
            }
        }
    }

    fn fire_step(&mut self) {
        let step = self.step;
        if step == 0 {
            self.switch_pattern();
        }
        let grid = &self.patterns[self.pattern];
        for (index, track) in self.tracks[..self.track_count].iter_mut().enumerate() {
            if grid[index][step] {
                // Accent hats on the beat; everything else at full velocity.
                let velocity = if track.kind == DrumKind::Hat && step % 4 != 0 { 0.6 } else { 1.0 };
                track.trigger(velocity, self.sample_rate);
            }
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

/// Index of pattern `p` after pattern `removed` was deleted.
fn after_removal(p: usize, removed: usize) -> usize {
    if p > removed { p - 1 } else { p }
}

/// Send a sample back to the control thread to be freed there.
fn retire(queue: &mut Producer<Arc<Sample>>, sample: Option<Arc<Sample>>) {
    if let Some(sample) = sample {
        if let Err(rtrb::PushError::Full(sample)) = queue.push(sample) {
            // The control thread is not collecting. Leaking is bounded and
            // real-time safe; freeing here would not be.
            std::mem::forget(sample);
        }
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

    /// Free samples the engine no longer uses. Call this regularly, e.g.
    /// once per UI frame. Returns how many were freed.
    pub fn collect_garbage(&mut self) -> usize {
        let mut freed = 0;
        while self.retired.pop().is_ok() {
            freed += 1;
        }
        freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    /// Frames per step at 120 BPM and 48 kHz.
    const STEP: usize = 6000;

    fn row(on: &[usize]) -> Steps {
        let mut steps = [false; STEPS];
        for &i in on {
            steps[i] = true;
        }
        steps
    }

    /// Kick, snare and hat with a four-on-the-floor beat in pattern 0.
    fn demo(handle: &mut EngineHandle) {
        for kind in [DrumKind::Kick, DrumKind::Snare, DrumKind::Hat] {
            handle.send(Command::AddTrack(kind)).unwrap();
        }
        for (track, steps) in [row(&[0, 4, 8, 12]), row(&[4, 12]), row(&[0, 2, 4, 6, 8, 10, 12, 14])]
            .into_iter()
            .enumerate()
        {
            handle.send(Command::SetRow { pattern: 0, track, steps }).unwrap();
        }
    }

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0; frames];
        engine.process(&mut buf, 1);
        buf
    }

    fn events(handle: &mut EngineHandle) -> Vec<Event> {
        std::iter::from_fn(|| handle.poll_event()).collect()
    }

    #[test]
    fn a_new_engine_is_empty_and_silent() {
        let (mut engine, mut handle) = Engine::new(SR);
        assert_eq!(engine.track_count, 0);
        handle.send(Command::Play).unwrap();
        assert!(render(&mut engine, 4096).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn silent_until_play() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        let mut buf = vec![0.0; 4096];
        engine.process(&mut buf, 2);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn play_advances_steps_and_makes_sound() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        handle.send(Command::Play).unwrap();
        // A bit more than two steps.
        let buf = render(&mut engine, 2 * STEP + 1000);
        assert!(buf.iter().any(|&s| s.abs() > 0.01), "no audio while playing");
        assert_eq!(events(&mut handle), [Event::Step(0), Event::Step(1), Event::Step(2)]);
    }

    #[test]
    fn invalid_commands_are_ignored() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        engine.process(&mut [], 1);
        let before = engine.patterns.clone();
        for cmd in [
            Command::ToggleStep { pattern: 0, track: 99, step: 0 },
            Command::ToggleStep { pattern: 0, track: 0, step: 99 },
            Command::ToggleStep { pattern: 99, track: 0, step: 0 },
            Command::ToggleStep { pattern: 0, track: 5, step: 0 }, // slot exists, track does not
            Command::SetRow { pattern: 0, track: 5, steps: [true; STEPS] },
            Command::SetBpm(f32::NAN),
            Command::SelectPattern(99),
            Command::RemovePattern(99),
            Command::RemoveTrack(99),
        ] {
            handle.send(cmd).unwrap();
        }
        engine.process(&mut [], 1);
        assert_eq!(engine.patterns, before);
        assert_eq!(engine.track_count, 3);
        assert_eq!(engine.pattern, 0);
        assert!((engine.bpm - 120.0).abs() < f32::EPSILON);
    }

    fn constant_sample(value: f32, frames: usize) -> Arc<Sample> {
        Arc::new(Sample::from_interleaved(&vec![value; frames], 1, SR))
    }

    #[test]
    fn loaded_sample_replaces_the_synth() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::AddTrack(DrumKind::Kick)).unwrap();
        handle.send(Command::SetStep { pattern: 0, track: 0, step: 0, on: true }).unwrap();
        handle.send(Command::SetSample { track: 0, sample: Some(constant_sample(0.5, 1000)) }).unwrap();
        handle.send(Command::Play).unwrap();

        let mut buf = vec![0.0; 2 * 100];
        engine.process(&mut buf, 2);
        let expected = 0.5 * SAMPLE_GAIN * 0.8;
        assert!((buf[0] - expected).abs() < 1e-4, "left {}", buf[0]);
        assert!((buf[1] - expected).abs() < 1e-4, "right {}", buf[1]);
    }

    #[test]
    fn replaced_samples_come_back_to_be_freed() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        handle.send(Command::SetSample { track: 1, sample: Some(constant_sample(0.1, 10)) }).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(handle.collect_garbage(), 0);

        handle.send(Command::SetSample { track: 1, sample: Some(constant_sample(0.2, 10)) }).unwrap();
        handle.send(Command::SetSample { track: 1, sample: None }).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(handle.collect_garbage(), 2);
    }

    #[test]
    fn add_and_remove_tracks() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        handle.send(Command::AddTrack(DrumKind::Clap)).unwrap();
        handle.send(Command::ToggleStep { pattern: 0, track: 3, step: 5 }).unwrap();
        handle.send(Command::ToggleStep { pattern: 2, track: 3, step: 6 }).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(engine.track_count, 4);
        assert!(engine.patterns[0][3][5]);
        assert_eq!(engine.tracks[3].kind, DrumKind::Clap);

        // Removing the snare moves hat and clap up one row in every pattern.
        handle.send(Command::RemoveTrack(1)).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(engine.track_count, 3);
        assert_eq!(engine.tracks[1].kind, DrumKind::Hat);
        assert_eq!(engine.tracks[2].kind, DrumKind::Clap);
        assert!(engine.patterns[0][2][5]);
        assert!(engine.patterns[2][2][6]);
        assert_eq!(engine.patterns[0][3], [false; STEPS]);
        assert_eq!(engine.patterns[2][3], [false; STEPS]);
    }

    #[test]
    fn a_reused_slot_starts_empty() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::AddTrack(DrumKind::Kick)).unwrap();
        handle.send(Command::SetRow { pattern: 4, track: 0, steps: [true; STEPS] }).unwrap();
        handle.send(Command::RemoveTrack(0)).unwrap();
        handle.send(Command::AddTrack(DrumKind::Tom)).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(engine.patterns[4][0], [false; STEPS]);
    }

    #[test]
    fn track_limits_are_respected() {
        let (mut engine, mut handle) = Engine::new(SR);
        for _ in 0..MAX_TRACKS + 5 {
            handle.send(Command::AddTrack(DrumKind::Tom)).unwrap();
        }
        engine.process(&mut [], 2);
        assert_eq!(engine.track_count, MAX_TRACKS);
        for _ in 0..MAX_TRACKS + 5 {
            handle.send(Command::RemoveTrack(0)).unwrap();
        }
        engine.process(&mut [], 2);
        assert_eq!(engine.track_count, 0);
    }

    #[test]
    fn reset_then_rebuild() {
        let (mut engine, mut handle) = Engine::new(SR);
        demo(&mut handle);
        handle.send(Command::SetSample { track: 0, sample: Some(constant_sample(0.1, 10)) }).unwrap();
        handle.send(Command::SetRow { pattern: 3, track: 2, steps: [true; STEPS] }).unwrap();
        handle.send(Command::SelectPattern(3)).unwrap();
        handle.send(Command::Reset).unwrap();
        handle.send(Command::AddTrack(DrumKind::Tom)).unwrap();
        handle.send(Command::SetStep { pattern: 0, track: 0, step: 3, on: true }).unwrap();
        engine.process(&mut [], 2);
        assert_eq!(engine.track_count, 1);
        assert_eq!(engine.tracks[0].kind, DrumKind::Tom);
        assert_eq!(engine.pattern, 0);
        assert_eq!(engine.patterns[0][0], row(&[3]));
        assert_eq!(engine.patterns[3][2], [false; STEPS]);
        assert_eq!(handle.collect_garbage(), 1);
    }

    #[test]
    fn selecting_a_pattern_while_stopped_is_immediate() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::SelectPattern(2)).unwrap();
        engine.process(&mut [], 1);
        assert_eq!(engine.pattern, 2);
        assert_eq!(events(&mut handle), [Event::Pattern(2)]);
    }

    #[test]
    fn selecting_a_pattern_while_playing_waits_for_the_bar() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::AddTrack(DrumKind::Kick)).unwrap();
        // Pattern 0: kick on step 0. Pattern 1: kick on step 8 only.
        handle.send(Command::SetRow { pattern: 0, track: 0, steps: row(&[0]) }).unwrap();
        handle.send(Command::SetRow { pattern: 1, track: 0, steps: row(&[8]) }).unwrap();
        handle.send(Command::Play).unwrap();
        render(&mut engine, 5 * STEP + 10);
        events(&mut handle);

        // Halfway through the bar: still pattern 0 until step 0 comes round.
        handle.send(Command::SelectPattern(1)).unwrap();
        render(&mut engine, 1);
        assert_eq!(engine.pattern, 0, "switched in the middle of a bar");
        // Up to the last frame before the next bar: steps 6 to 15 play.
        let rest_of_bar = engine.frames_until_step().unwrap() + 10 * STEP;
        render(&mut engine, rest_of_bar);
        assert_eq!(engine.pattern, 0, "switched before the bar ended");
        assert_eq!(engine.next_step(), 0);
        events(&mut handle);

        // The frame that starts the next bar switches, then plays step 0.
        render(&mut engine, 1);
        assert_eq!(events(&mut handle), [Event::Pattern(1), Event::Step(0)]);
        assert_eq!(engine.pattern, 1);

        // The new pattern really plays: nothing until its hit on step 8.
        let bar = render(&mut engine, STEPS * STEP);
        let hit = 8 * STEP - 1;
        assert!(bar[..hit].iter().all(|&s| s == 0.0), "pattern 1 has nothing before step 8");
        assert!(bar[hit..hit + 2000].iter().any(|s| s.abs() > 0.1), "pattern 1 step 8 did not play");
    }

    #[test]
    fn removing_a_pattern_moves_the_later_ones_up() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::AddTrack(DrumKind::Kick)).unwrap();
        for pattern in 0..4 {
            handle.send(Command::SetRow { pattern, track: 0, steps: row(&[pattern]) }).unwrap();
        }
        handle.send(Command::SelectPattern(3)).unwrap();
        handle.send(Command::RemovePattern(1)).unwrap();
        engine.process(&mut [], 1);
        assert_eq!(engine.patterns[0][0], row(&[0]));
        assert_eq!(engine.patterns[1][0], row(&[2]));
        assert_eq!(engine.patterns[2][0], row(&[3]));
        assert_eq!(engine.patterns[3][0], [false; STEPS]);
        assert_eq!(engine.pattern, 2, "the playing pattern keeps playing");
    }

    #[test]
    fn a_whole_project_fits_in_the_queue() {
        let (mut engine, mut handle) = Engine::new(SR);
        handle.send(Command::Reset).unwrap();
        for _ in 0..MAX_TRACKS {
            handle.send(Command::AddTrack(DrumKind::Hat)).unwrap();
        }
        for pattern in 0..MAX_PATTERNS {
            for track in 0..MAX_TRACKS {
                handle.send(Command::SetRow { pattern, track, steps: [true; STEPS] }).unwrap();
            }
        }
        handle.send(Command::SetBpm(130.0)).unwrap();
        handle.send(Command::SelectPattern(0)).unwrap();
        engine.process(&mut [], 1);
        assert!(engine.patterns.iter().all(|g| g.iter().all(|r| *r == [true; STEPS])));
    }

    #[test]
    fn frames_until_step_predicts_the_next_step() {
        // Includes tempos where a step is not a whole number of frames.
        for bpm in [120.0, 133.0, 97.3, 300.0] {
            let (mut engine, mut handle) = Engine::new(44_100.0);
            assert_eq!(engine.frames_until_step(), None);
            handle.send(Command::SetBpm(bpm)).unwrap();
            handle.send(Command::Play).unwrap();
            engine.process(&mut [], 1);
            for expected_step in 0..40 {
                let n = engine.frames_until_step().unwrap();
                let mut buf = vec![0.0; n];
                engine.process(&mut buf, 1);
                assert_eq!(handle.poll_event(), None, "a step fired early at {bpm} BPM");
                assert_eq!(engine.next_step(), expected_step % STEPS);
                engine.process(&mut [0.0], 1);
                assert_eq!(handle.poll_event(), Some(Event::Step(expected_step % STEPS)), "at {bpm} BPM");
            }
        }
    }
}
