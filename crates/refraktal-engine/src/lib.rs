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
pub const MAX_TRACKS: usize = 8;
/// Tracks in a new project.
pub const DEFAULT_TRACKS: [DrumKind; 3] = [DrumKind::Kick, DrumKind::Snare, DrumKind::Hat];

// Large enough to load a whole project (8 tracks × 16 steps) in one go.
const QUEUE_CAPACITY: usize = 1024;
const RETIRE_CAPACITY: usize = 64;
const MIN_BPM: f32 = 20.0;
const MAX_BPM: f32 = 999.0;
const SAMPLE_GAIN: f32 = 0.8;
/// Fade applied to a sample that is cut off by the next hit.
const RETRIGGER_FADE_SECONDS: f32 = 0.003;

/// A pattern: one row of steps per track slot. Rows past the track count
/// are always empty.
pub type Pattern = [[bool; STEPS]; MAX_TRACKS];

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
    ToggleStep { track: usize, step: usize },
    SetStep { track: usize, step: usize, on: bool },
    /// Start over with a single empty track, e.g. before loading a project.
    Reset(DrumKind),
    /// Add a track at the end, if there is room.
    AddTrack(DrumKind),
    /// Remove a track; the ones below move up. The last track cannot be removed.
    RemoveTrack(usize),
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
}

/// A simple four-on-the-floor starting beat.
#[must_use]
pub fn default_pattern() -> Pattern {
    let mut p = [[false; STEPS]; MAX_TRACKS];
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
    pattern: Pattern,
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
            pattern: default_pattern(),
            step: 0,
            samples_until_step: 0.0,
            tracks: std::array::from_fn(|i| {
                Track::new(DEFAULT_TRACKS.get(i).copied().unwrap_or(DrumKind::Kick), sample_rate)
            }),
            track_count: DEFAULT_TRACKS.len(),
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
                if track < self.track_count {
                    if let Some(cell) = self.pattern[track].get_mut(step) {
                        *cell = !*cell;
                    }
                }
            }
            Command::SetStep { track, step, on } => {
                if track < self.track_count {
                    if let Some(cell) = self.pattern[track].get_mut(step) {
                        *cell = on;
                    }
                }
            }
            Command::Reset(kind) => {
                for track in &mut self.tracks {
                    let old = track.sample.take();
                    retire(&mut self.retired, old);
                }
                self.tracks[0] = Track::new(kind, self.sample_rate);
                self.track_count = 1;
                self.pattern = [[false; STEPS]; MAX_TRACKS];
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
                    self.pattern[index] = [false; STEPS];
                    self.track_count += 1;
                }
            }
            Command::RemoveTrack(track) => {
                if track < self.track_count && self.track_count > 1 {
                    let old = self.tracks[track].sample.take();
                    retire(&mut self.retired, old);
                    // Move the removed slot to the end; nothing is allocated or freed.
                    self.tracks[track..self.track_count].rotate_left(1);
                    self.pattern[track..self.track_count].rotate_left(1);
                    self.track_count -= 1;
                    self.pattern[self.track_count] = [false; STEPS];
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

    fn fire_step(&mut self) {
        let step = self.step;
        for (index, track) in self.tracks[..self.track_count].iter_mut().enumerate() {
            if self.pattern[index][step] {
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

    fn constant_sample(value: f32, frames: usize) -> Arc<Sample> {
        Arc::new(Sample::from_interleaved(&vec![value; frames], 1, SR))
    }

    #[test]
    fn loaded_sample_replaces_the_synth() {
        let (mut engine, mut handle) = Engine::new(SR);
        // Only the kick track plays, on step 0.
        for step in 0..STEPS {
            for track in 0..MAX_TRACKS {
                if engine.pattern[track][step] && !(track == 0 && step == 0) {
                    handle.send(Command::ToggleStep { track, step }).unwrap();
                }
            }
        }
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
        let mut buf = vec![0.0; 64];
        handle.send(Command::SetSample { track: 1, sample: Some(constant_sample(0.1, 10)) }).unwrap();
        engine.process(&mut buf, 2);
        assert_eq!(handle.collect_garbage(), 0);

        handle.send(Command::SetSample { track: 1, sample: Some(constant_sample(0.2, 10)) }).unwrap();
        handle.send(Command::SetSample { track: 1, sample: None }).unwrap();
        engine.process(&mut buf, 2);
        assert_eq!(handle.collect_garbage(), 2);
    }

    #[test]
    fn add_and_remove_tracks() {
        let (mut engine, mut handle) = Engine::new(SR);
        let mut buf = vec![0.0; 64];
        handle.send(Command::AddTrack(DrumKind::Clap)).unwrap();
        handle.send(Command::ToggleStep { track: 3, step: 5 }).unwrap();
        engine.process(&mut buf, 2);
        assert_eq!(engine.track_count, 4);
        assert!(engine.pattern[3][5]);
        assert_eq!(engine.tracks[3].kind, DrumKind::Clap);

        // Removing the snare moves hat and clap up one row.
        handle.send(Command::RemoveTrack(1)).unwrap();
        engine.process(&mut buf, 2);
        assert_eq!(engine.track_count, 3);
        assert_eq!(engine.tracks[1].kind, DrumKind::Hat);
        assert_eq!(engine.tracks[2].kind, DrumKind::Clap);
        assert!(engine.pattern[2][5]);
        assert_eq!(engine.pattern[3], [false; STEPS]);
    }

    #[test]
    fn track_limits_are_respected() {
        let (mut engine, mut handle) = Engine::new(SR);
        let mut buf = vec![0.0; 64];
        for _ in 0..20 {
            handle.send(Command::AddTrack(DrumKind::Tom)).unwrap();
        }
        engine.process(&mut buf, 2);
        assert_eq!(engine.track_count, MAX_TRACKS);
        for _ in 0..20 {
            handle.send(Command::RemoveTrack(0)).unwrap();
        }
        engine.process(&mut buf, 2);
        assert_eq!(engine.track_count, 1);
    }

    #[test]
    fn reset_then_rebuild() {
        let (mut engine, mut handle) = Engine::new(SR);
        let mut buf = vec![0.0; 64];
        handle.send(Command::SetSample { track: 0, sample: Some(constant_sample(0.1, 10)) }).unwrap();
        handle.send(Command::Reset(DrumKind::Tom)).unwrap();
        handle.send(Command::AddTrack(DrumKind::Clap)).unwrap();
        handle.send(Command::SetStep { track: 1, step: 3, on: true }).unwrap();
        engine.process(&mut buf, 2);
        assert_eq!(engine.track_count, 2);
        assert_eq!(engine.tracks[0].kind, DrumKind::Tom);
        assert!(engine.pattern[1][3]);
        assert!(!engine.pattern[0][0]);
        assert_eq!(handle.collect_garbage(), 1);
    }
}
