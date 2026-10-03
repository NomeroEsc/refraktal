// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline rendering: a project in, finished audio out.
//!
//! The renderer runs its own [`Engine`], separate from the one playing
//! live, and feeds it the same commands the window sends when it opens a
//! project. An export therefore sounds exactly like playback.

#[cfg(not(target_os = "android"))]
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use refraktal_dsp::Sample;
use refraktal_engine::{Command, DrumKind, Engine, STEPS};
use refraktal_io::{Instrument, Project};

use crate::gui::{MAX_BPM, MIN_BPM};

/// Sample rate of exported audio.
pub const EXPORT_SAMPLE_RATE: u32 = 48_000;
/// How many times the selected pattern plays in an export.
pub const EXPORT_LOOPS: usize = 4;
/// Sounds still ringing after the last loop get at most this long.
const MAX_TAIL_SECONDS: f32 = 5.0;
/// The tail ends once the output has stayed below this level (−80 dBFS)...
const SILENCE: f32 = 1.0e-4;
/// ...for this long.
const SILENT_SECONDS: f32 = 0.1;
/// A tail cut off at the limit fades out over this time instead of clicking.
const FADE_SECONDS: f32 = 0.05;
const BLOCK_FRAMES: usize = 1024;

/// Finished audio of an export.
pub struct Rendered {
    /// Interleaved stereo at [`EXPORT_SAMPLE_RATE`].
    pub audio: Vec<f32>,
    /// Frames up to the end of the last loop; the tail follows.
    pub loop_frames: usize,
}

impl Rendered {
    #[must_use]
    pub fn frames(&self) -> usize {
        self.audio.len() / 2
    }

    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / EXPORT_SAMPLE_RATE as f32
    }

    /// Length of the ring-out after the last loop.
    #[cfg(not(target_os = "android"))]
    #[must_use]
    pub fn tail_seconds(&self) -> f32 {
        (self.frames() - self.loop_frames) as f32 / EXPORT_SAMPLE_RATE as f32
    }
}

/// The engine voice for an instrument. Unknown drum names play a kick.
pub fn drum_kind(instrument: &Instrument) -> DrumKind {
    match instrument {
        Instrument::Drum { sound } => DrumKind::from_name(sound).unwrap_or(DrumKind::Kick),
    }
}

/// Tempo of a saved project, limited to what the interface allows.
pub fn project_bpm(project: &Project) -> f32 {
    project.bpm.clamp(MIN_BPM, MAX_BPM)
}

/// Commands that rebuild `project` in an engine: tracks, every pattern's
/// steps, tempo and the selected pattern. Samples are not included; they
/// are decoded separately. The engine's track and pattern indices then
/// match the project's.
pub fn project_commands(project: &Project) -> Vec<Command> {
    let mut commands = vec![Command::Reset];
    commands.extend(project.tracks.iter().map(|t| Command::AddTrack(drum_kind(&t.instrument))));
    for (pattern, data) in project.patterns.iter().enumerate() {
        for (track, steps) in data.steps.iter().enumerate() {
            if let Some(steps) = steps.filter(|s| s.contains(&true)) {
                commands.push(Command::SetRow { pattern, track, steps });
            }
        }
    }
    commands.push(Command::SetBpm(project_bpm(project)));
    commands.push(Command::SelectPattern(project.selected_pattern));
    commands
}

/// Decode every sample a project uses. Fails if any of them cannot be read,
/// so an export never silently swaps a sample for a built-in sound.
pub fn load_samples(project: &Project) -> Result<Vec<Option<Arc<Sample>>>> {
    project
        .tracks
        .iter()
        .map(|track| {
            track
                .sample
                .as_deref()
                .map(|path| refraktal_io::load_sample(path).map(Arc::new))
                .transpose()
        })
        .collect()
}

/// Render `project`: its selected pattern [`EXPORT_LOOPS`] times, then
/// whatever is still ringing. `samples[i]` replaces the sound of track `i`.
#[must_use]
pub fn render(project: &Project, samples: &[Option<Arc<Sample>>]) -> Rendered {
    let sample_rate = EXPORT_SAMPLE_RATE as f32;
    let (mut engine, mut handle) = Engine::new(sample_rate);

    let mut commands = project_commands(project);
    for (track, sample) in samples.iter().enumerate().take(project.tracks.len()) {
        if let Some(sample) = sample {
            commands.push(Command::SetSample { track, sample: Some(Arc::clone(sample)) });
        }
    }
    commands.push(Command::Play);
    for cmd in commands {
        if let Err(cmd) = handle.send(cmd) {
            // Queue full: let the engine take what is there, then retry.
            engine.process(&mut [], 2);
            assert!(handle.send(cmd).is_ok(), "the engine queue is empty after processing");
        }
    }
    engine.process(&mut [], 2);

    let mut audio = Vec::new();
    for _ in 0..EXPORT_LOOPS * STEPS {
        // Everything up to and including the frame that plays the next step.
        let frames = engine.frames_until_step().expect("the engine is playing") + 1;
        render_frames(&mut engine, &mut audio, frames);
    }
    // Up to where the next loop would begin, then stop before it does.
    let frames = engine.frames_until_step().expect("the engine is playing");
    render_frames(&mut engine, &mut audio, frames);
    let loop_frames = audio.len() / 2;
    assert!(handle.send(Command::Stop).is_ok(), "the engine queue is empty");

    // Let the last hits ring out.
    let max_tail = (MAX_TAIL_SECONDS * sample_rate) as usize;
    let silent_needed = (SILENT_SECONDS * sample_rate) as usize;
    let mut tail = 0;
    let mut silent = 0;
    while tail < max_tail && silent < silent_needed {
        let start = audio.len();
        render_frames(&mut engine, &mut audio, BLOCK_FRAMES);
        tail += BLOCK_FRAMES;
        if audio[start..].iter().all(|s| s.abs() < SILENCE) {
            silent += BLOCK_FRAMES;
        } else {
            silent = 0;
        }
    }
    if silent < silent_needed {
        // Cut off at the limit: fade out the end of the tail.
        let fade = ((FADE_SECONDS * sample_rate) as usize).min(tail);
        let first = audio.len() / 2 - fade;
        for (i, frame) in audio[first * 2..].chunks_exact_mut(2).enumerate() {
            let gain = 1.0 - (i + 1) as f32 / fade as f32;
            frame[0] *= gain;
            frame[1] *= gain;
        }
    }

    drop(engine);
    handle.collect_garbage();
    Rendered { audio, loop_frames }
}

fn render_frames(engine: &mut Engine, audio: &mut Vec<f32>, frames: usize) {
    let start = audio.len();
    audio.resize(start + frames * 2, 0.0);
    for block in audio[start..].chunks_mut(BLOCK_FRAMES * 2) {
        engine.process(block, 2);
    }
}

/// Render a saved project to a WAV file. Used by `--export`.
#[cfg(not(target_os = "android"))]
pub fn export_file(project_path: &Path, wav_path: &Path) -> Result<Rendered> {
    let project = Project::load(project_path)?;
    let samples = load_samples(&project)?;
    let rendered = render(&project, &samples);
    refraktal_io::save_wav(wav_path, &rendered.audio, EXPORT_SAMPLE_RATE)
        .with_context(|| format!("could not export to {}", wav_path.display()))?;
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use refraktal_io::{Steps, TrackData};

    use super::*;

    /// One pattern with these tracks and steps.
    fn project(bpm: f32, tracks: &[(&str, Steps)]) -> Project {
        let mut p = Project::new();
        p.bpm = bpm;
        for (i, (sound, steps)) in tracks.iter().enumerate() {
            let track = p.add_track(0, TrackData::new(Instrument::drum(sound), i as u8)).unwrap();
            for (step, &on) in steps.iter().enumerate() {
                p.set_step(0, track, step, on);
            }
        }
        p
    }

    fn default_project() -> Project {
        Project::demo()
    }

    fn kick_on_beats() -> Steps {
        std::array::from_fn(|i| i % 4 == 0)
    }

    fn only_first_step() -> [bool; STEPS] {
        let mut steps = [false; STEPS];
        steps[0] = true;
        steps
    }

    /// A sample that holds `value` for `frames` frames at the export rate.
    fn constant(value: f32, frames: usize) -> Option<Arc<Sample>> {
        Some(Arc::new(Sample::from_interleaved(&vec![value; frames], 1, EXPORT_SAMPLE_RATE as f32)))
    }

    fn frame(r: &Rendered, i: usize) -> [f32; 2] {
        [r.audio[2 * i], r.audio[2 * i + 1]]
    }

    #[test]
    fn loops_last_exactly_four_bars() {
        // At 120 BPM and 48 kHz a step is exactly 6000 frames.
        let r = render(&default_project(), &[]);
        assert_eq!(r.loop_frames, EXPORT_LOOPS * STEPS * 6000);
        assert!(r.frames() > r.loop_frames, "no tail");
        assert!(r.frames() <= r.loop_frames + (MAX_TAIL_SECONDS * 48_000.0) as usize + BLOCK_FRAMES);
        assert!(r.audio.iter().any(|s| s.abs() > 0.1), "export is silent");
    }

    #[test]
    fn odd_tempos_keep_the_bar_length() {
        let r = render(&project(97.3, &[("kick", kick_on_beats())]), &[]);
        let expected = EXPORT_LOOPS as f64 * STEPS as f64 * 48_000.0 * 60.0 / 97.3 / 4.0;
        assert!((r.loop_frames as f64 - expected).abs() <= 1.0, "{} vs {expected}", r.loop_frames);
    }

    #[test]
    fn exports_are_deterministic() {
        let a = render(&default_project(), &[]);
        let b = render(&default_project(), &[]);
        assert_eq!(a.audio, b.audio);
    }

    #[test]
    fn the_next_loop_never_starts() {
        // A 100-frame sample on step 0: it must play at the start of every
        // loop, and not again where a fifth loop would begin.
        let r = render(&project(120.0, &[("kick", only_first_step())]), &[constant(0.5, 100)]);
        let bar = STEPS * 6000;
        for k in 0..EXPORT_LOOPS {
            assert!(frame(&r, k * bar)[0] > 0.1, "loop {k} did not play");
        }
        assert_eq!(r.loop_frames, EXPORT_LOOPS * bar);
        assert!(r.audio[r.loop_frames * 2..].iter().all(|&s| s == 0.0), "something played after the last loop");
        // Nothing rings, so the tail is just the silence check.
        assert!(r.frames() - r.loop_frames <= (SILENT_SECONDS * 48_000.0) as usize + BLOCK_FRAMES);
    }

    #[test]
    fn an_empty_pattern_is_silent() {
        let r = render(&project(120.0, &[("kick", [false; STEPS])]), &[]);
        assert!(r.audio.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn long_tails_are_cut_and_faded() {
        // A 20-second sample would ring far past the tail limit.
        let r = render(&project(120.0, &[("kick", only_first_step())]), &[constant(0.5, 20 * 48_000)]);
        let tail = r.frames() - r.loop_frames;
        assert!(tail >= (MAX_TAIL_SECONDS * 48_000.0) as usize);
        assert!(tail < (MAX_TAIL_SECONDS * 48_000.0) as usize + BLOCK_FRAMES);
        let [l, r_] = frame(&r, r.frames() - 1);
        assert!(l.abs() < 1e-3 && r_.abs() < 1e-3, "no fade-out: {l} {r_}");
        assert!(frame(&r, r.frames() - 48_000)[0] > 0.1, "faded too early");
    }

    #[test]
    fn writes_a_wav_file() {
        let dir = std::env::temp_dir().join(format!("refraktal-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project_path = dir.join("beat.refraktal");
        let wav_path = dir.join("beat.wav");
        default_project().save(&project_path).unwrap();
        let r = export_file(&project_path, &wav_path).unwrap();
        let bytes = std::fs::read(&wav_path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(bytes.len(), 44 + r.audio.len() * 2);
    }

    #[test]
    fn the_selected_pattern_is_exported() {
        let mut p = project(120.0, &[("kick", only_first_step())]);
        let second = p.add_pattern().unwrap();
        p.set_step(second, 0, 8, true);
        let samples = [constant(0.5, 100)];

        let first = render(&p, &samples);
        assert!(frame(&first, 0)[0] > 0.1 && frame(&first, 8 * 6000)[0] == 0.0);
        p.selected_pattern = second;
        let other = render(&p, &samples);
        assert!(frame(&other, 0)[0] == 0.0 && frame(&other, 8 * 6000)[0] > 0.1);
    }

    #[test]
    fn an_empty_project_exports_silence() {
        let r = render(&Project::new(), &[]);
        assert_eq!(r.loop_frames, EXPORT_LOOPS * STEPS * 6000);
        assert!(r.audio.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn a_missing_sample_is_an_error() {
        let mut p = default_project();
        p.tracks[0].sample = Some(std::env::temp_dir().join("refraktal-no-such-sample.wav"));
        assert!(load_samples(&p).is_err());
    }
}
