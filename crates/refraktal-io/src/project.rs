// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects: the document being edited, and its file format (`.refraktal`).
//!
//! A project has one list of tracks (sounds) and a list of patterns. Each
//! pattern stores steps for the tracks that take part in it. With
//! [`Sharing::Shared`] every track is in every pattern, as in a classic
//! channel rack; with [`Sharing::PerPattern`] a new track joins only the
//! pattern it was added to. The data is the same either way, so switching
//! loses nothing.
//!
//! Files are small, versioned JSON. Sample paths are stored relative to the
//! project file whenever possible, so a project folder can be moved or
//! shared without exposing the author's directory layout. Version 1 files
//! (one pattern, before instruments) are read and upgraded transparently.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// File extension for projects, without the dot.
pub const PROJECT_EXTENSION: &str = "refraktal";
/// Steps in a pattern (one bar of 16th notes).
pub const STEPS: usize = 16;
/// Most tracks in a project.
pub const MAX_TRACKS: usize = 32;
/// Most patterns in a project.
pub const MAX_PATTERNS: usize = 16;
/// Most tracks one pattern can show.
pub const MAX_PATTERN_TRACKS: usize = 8;

const FORMAT: &str = "refraktal-project";
const VERSION: u32 = 2;
const DEFAULT_BPM: f32 = 120.0;
/// Size of the track color palette.
const COLORS: u8 = 8;

/// Steps of one track in one pattern.
pub type Steps = [bool; STEPS];

/// How tracks relate to patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sharing {
    /// Every track is in every pattern.
    #[default]
    Shared,
    /// A track is only in the patterns it was added to.
    PerPattern,
}

/// What makes a track's sound. New kinds of instruments are added here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Instrument {
    /// A built-in drum synth, e.g. `"kick"`.
    Drum { sound: String },
}

impl Instrument {
    #[must_use]
    pub fn drum(sound: &str) -> Self {
        Self::Drum { sound: sound.to_owned() }
    }

    /// Short name shown on screen.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Drum { sound } => sound,
        }
    }
}

/// One track of a project.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackData {
    pub instrument: Instrument,
    /// Palette index of the track color.
    pub color: u8,
    /// Absolute path of a sample that replaces the instrument, if any.
    pub sample: Option<PathBuf>,
}

impl TrackData {
    #[must_use]
    pub fn new(instrument: Instrument, color: u8) -> Self {
        Self { instrument, color, sample: None }
    }
}

/// One pattern: steps for the tracks that take part in it.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternData {
    pub name: String,
    /// One entry per project track; `None` means the track is not in this pattern.
    pub steps: Vec<Option<Steps>>,
}

/// A beat: the document the interface edits and saves.
///
/// Invariants, kept by every method here and checked on load: there is at
/// least one pattern; every pattern has one entry per track; no pattern
/// shows more than [`MAX_PATTERN_TRACKS`] tracks; with [`Sharing::Shared`]
/// every track is in every pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub bpm: f32,
    pub sharing: Sharing,
    /// The pattern being edited and played.
    pub selected_pattern: usize,
    pub tracks: Vec<TrackData>,
    pub patterns: Vec<PatternData>,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    /// An empty project: no tracks and one empty pattern.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bpm: DEFAULT_BPM,
            sharing: Sharing::Shared,
            selected_pattern: 0,
            tracks: Vec::new(),
            patterns: vec![PatternData { name: pattern_name(1), steps: Vec::new() }],
        }
    }

    /// A simple four-on-the-floor beat with kick, snare and hats.
    #[must_use]
    pub fn demo() -> Self {
        let mut project = Self::new();
        for (sound, color) in [("kick", 0), ("snare", 1), ("hat", 2)] {
            project.add_track(0, TrackData::new(Instrument::drum(sound), color));
        }
        for step in 0..STEPS {
            project.set_step(0, 0, step, step % 4 == 0);
            project.set_step(0, 1, step, step == 4 || step == 12);
            project.set_step(0, 2, step, step % 2 == 0);
        }
        project
    }

    /// Tracks shown in `pattern`, in project order.
    #[must_use]
    pub fn rows(&self, pattern: usize) -> Vec<usize> {
        self.patterns
            .get(pattern)
            .map(|p| p.steps.iter().enumerate().filter(|(_, s)| s.is_some()).map(|(i, _)| i).collect())
            .unwrap_or_default()
    }

    /// Steps of `track` in `pattern`, if the track is in it.
    #[must_use]
    pub fn steps(&self, pattern: usize, track: usize) -> Option<&Steps> {
        self.patterns.get(pattern)?.steps.get(track)?.as_ref()
    }

    /// Whether [`add_track`](Self::add_track) would succeed on `pattern`.
    #[must_use]
    pub fn can_add_track(&self, pattern: usize) -> bool {
        if self.tracks.len() >= MAX_TRACKS || pattern >= self.patterns.len() {
            return false;
        }
        match self.sharing {
            Sharing::Shared => self.tracks.len() < MAX_PATTERN_TRACKS,
            Sharing::PerPattern => self.rows(pattern).len() < MAX_PATTERN_TRACKS,
        }
    }

    /// Add a track at the end of the list. It joins every pattern when
    /// tracks are shared, otherwise only `pattern`. Returns its index, or
    /// `None` if there is no room.
    pub fn add_track(&mut self, pattern: usize, track: TrackData) -> Option<usize> {
        if !self.can_add_track(pattern) {
            return None;
        }
        let shared = self.sharing == Sharing::Shared;
        for (i, p) in self.patterns.iter_mut().enumerate() {
            p.steps.push((shared || i == pattern).then_some([false; STEPS]));
        }
        self.tracks.push(track);
        Some(self.tracks.len() - 1)
    }

    /// Delete a track from the project and from every pattern. Later
    /// tracks move up by one, exactly like `Command::RemoveTrack` in the engine.
    pub fn remove_track(&mut self, track: usize) {
        if track < self.tracks.len() {
            self.tracks.remove(track);
            for p in &mut self.patterns {
                p.steps.remove(track);
            }
        }
    }

    /// Take a track out of `pattern`. With shared tracks, or when no other
    /// pattern uses it, the track is deleted from the project; returns
    /// `true` in that case.
    pub fn remove_row(&mut self, pattern: usize, track: usize) -> bool {
        if track >= self.tracks.len() || pattern >= self.patterns.len() {
            return false;
        }
        let used_elsewhere = self
            .patterns
            .iter()
            .enumerate()
            .any(|(i, p)| i != pattern && p.steps[track].is_some());
        if self.sharing == Sharing::Shared || !used_elsewhere {
            self.remove_track(track);
            true
        } else {
            self.patterns[pattern].steps[track] = None;
            false
        }
    }

    /// Flip one step. Returns the new value, or `None` if the track is not
    /// in that pattern.
    pub fn toggle_step(&mut self, pattern: usize, track: usize, step: usize) -> Option<bool> {
        let cell = self.patterns.get_mut(pattern)?.steps.get_mut(track)?.as_mut()?.get_mut(step)?;
        *cell = !*cell;
        Some(*cell)
    }

    /// Set one step. Does nothing if the track is not in that pattern.
    pub fn set_step(&mut self, pattern: usize, track: usize, step: usize, on: bool) {
        if let Some(cell) = self
            .patterns
            .get_mut(pattern)
            .and_then(|p| p.steps.get_mut(track))
            .and_then(Option::as_mut)
            .and_then(|s| s.get_mut(step))
        {
            *cell = on;
        }
    }

    /// Add an empty pattern at the end. With shared tracks it shows every
    /// track; otherwise it starts with none. Returns its index.
    pub fn add_pattern(&mut self) -> Option<usize> {
        if self.patterns.len() >= MAX_PATTERNS {
            return None;
        }
        let number = (1..).find(|n| self.patterns.iter().all(|p| p.name != pattern_name(*n))).unwrap_or(1);
        let fill = (self.sharing == Sharing::Shared).then_some([false; STEPS]);
        self.patterns.push(PatternData { name: pattern_name(number), steps: vec![fill; self.tracks.len()] });
        Some(self.patterns.len() - 1)
    }

    /// Delete a pattern; the last one cannot be deleted. Tracks that no
    /// pattern uses any more are deleted too. Returns the deleted tracks'
    /// indices in the order they were removed (each one valid at the time
    /// of its removal), so the engine can be told the same.
    pub fn remove_pattern(&mut self, pattern: usize) -> Option<Vec<usize>> {
        if self.patterns.len() <= 1 || pattern >= self.patterns.len() {
            return None;
        }
        self.patterns.remove(pattern);
        if self.selected_pattern > pattern || self.selected_pattern >= self.patterns.len() {
            self.selected_pattern = self.selected_pattern.saturating_sub(1);
        }
        // Highest index first, so the earlier indices stay valid.
        let orphans: Vec<usize> = (0..self.tracks.len())
            .rev()
            .filter(|&t| self.patterns.iter().all(|p| p.steps[t].is_none()))
            .collect();
        for &t in &orphans {
            self.remove_track(t);
        }
        Some(orphans)
    }

    /// Switch how tracks relate to patterns.
    ///
    /// To shared: every track joins every pattern with empty steps. Fails
    /// if the project has more tracks than a pattern can show.
    /// To per-pattern: each pattern keeps the tracks that have steps in it;
    /// a track with no steps anywhere stays in the selected pattern.
    pub fn set_sharing(&mut self, sharing: Sharing) -> Result<()> {
        if sharing == self.sharing {
            return Ok(());
        }
        match sharing {
            Sharing::Shared => {
                ensure!(
                    self.tracks.len() <= MAX_PATTERN_TRACKS,
                    "shared tracks allow at most {MAX_PATTERN_TRACKS}, this project has {}",
                    self.tracks.len()
                );
                for p in &mut self.patterns {
                    for s in &mut p.steps {
                        s.get_or_insert([false; STEPS]);
                    }
                }
            }
            Sharing::PerPattern => {
                let used: Vec<bool> = (0..self.tracks.len())
                    .map(|t| self.patterns.iter().any(|p| p.steps[t].is_some_and(|s| s.contains(&true))))
                    .collect();
                let selected = self.selected_pattern;
                for (i, p) in self.patterns.iter_mut().enumerate() {
                    for (t, s) in p.steps.iter_mut().enumerate() {
                        let keep = s.is_some_and(|s| s.contains(&true)) || (!used[t] && i == selected);
                        if !keep {
                            *s = None;
                        }
                    }
                }
            }
        }
        self.sharing = sharing;
        Ok(())
    }

    /// Write the project to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let base = path.parent().unwrap_or(Path::new(""));
        let file = FileV2 {
            format: FORMAT.to_owned(),
            version: VERSION,
            bpm: self.bpm,
            sharing: self.sharing,
            selected_pattern: self.selected_pattern,
            tracks: self
                .tracks
                .iter()
                .map(|t| TrackV2 {
                    instrument: t.instrument.clone(),
                    color: t.color,
                    sample: t.sample.as_deref().map(|s| path_for_file(s, base)),
                })
                .collect(),
            patterns: self
                .patterns
                .iter()
                .map(|p| PatternV2 {
                    name: p.name.clone(),
                    rows: p
                        .steps
                        .iter()
                        .enumerate()
                        .filter_map(|(track, s)| s.map(|s| RowV2 { track, steps: steps_to_text(&s) }))
                        .collect(),
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        // Write to a temporary file first so a crash never leaves a half-written project.
        let tmp = path.with_extension("refraktal.tmp");
        fs::write(&tmp, json).with_context(|| format!("could not write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("could not save {}", path.display()))?;
        Ok(())
    }

    /// Read a project from `path`. Version 1 files are upgraded.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("could not open {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(""));
        Self::from_json(&text, base)
    }

    fn from_json(text: &str, base: &Path) -> Result<Self> {
        let header: Header = serde_json::from_str(text).context("this is not a valid Refraktal project")?;
        if header.format != FORMAT {
            bail!("this is not a Refraktal project");
        }
        let project = match header.version {
            1 => {
                let file: FileV1 = serde_json::from_str(text).context("this is not a valid Refraktal project")?;
                from_v1(file, base)?
            }
            2 => {
                let file: FileV2 = serde_json::from_str(text).context("this project file is damaged")?;
                from_v2(file, base)?
            }
            v if v > VERSION => bail!("this project was saved by a newer version of Refraktal"),
            _ => bail!("unknown project version"),
        };
        project.check()?;
        Ok(project)
    }

    /// Verify the invariants listed on [`Project`].
    fn check(&self) -> Result<()> {
        ensure!(self.bpm.is_finite(), "the tempo in this project is invalid");
        ensure!(self.tracks.len() <= MAX_TRACKS, "a project can have at most {MAX_TRACKS} tracks");
        ensure!(
            (1..=MAX_PATTERNS).contains(&self.patterns.len()),
            "a project must have between 1 and {MAX_PATTERNS} patterns"
        );
        ensure!(self.selected_pattern < self.patterns.len(), "the selected pattern does not exist");
        for p in &self.patterns {
            ensure!(p.steps.len() == self.tracks.len(), "pattern '{}' does not match the track list", p.name);
            ensure!(
                p.steps.iter().flatten().count() <= MAX_PATTERN_TRACKS,
                "pattern '{}' has more than {MAX_PATTERN_TRACKS} tracks",
                p.name
            );
            if self.sharing == Sharing::Shared {
                ensure!(p.steps.iter().all(Option::is_some), "pattern '{}' is missing shared tracks", p.name);
            }
        }
        Ok(())
    }
}

fn pattern_name(number: usize) -> String {
    format!("Pattern {number}")
}

// ---- File format -------------------------------------------------------

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// Version 2: tracks with instruments, several patterns.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileV2 {
    format: String,
    version: u32,
    bpm: f32,
    #[serde(default)]
    sharing: Sharing,
    #[serde(default)]
    selected_pattern: usize,
    tracks: Vec<TrackV2>,
    patterns: Vec<PatternV2>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackV2 {
    instrument: Instrument,
    color: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternV2 {
    name: String,
    /// Tracks in this pattern. Steps are written as `"x...x..."` so the
    /// file stays readable and diffs well in Git.
    rows: Vec<RowV2>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowV2 {
    track: usize,
    steps: String,
}

/// Version 1: one pattern of up to 8 tracks.
#[derive(Deserialize)]
struct FileV1 {
    bpm: f32,
    tracks: Vec<TrackV1>,
}

#[derive(Deserialize)]
struct TrackV1 {
    sound: String,
    color: u8,
    steps: String,
    #[serde(default)]
    sample: Option<String>,
}

fn from_v1(file: FileV1, base: &Path) -> Result<Project> {
    ensure!(
        (1..=MAX_PATTERN_TRACKS).contains(&file.tracks.len()),
        "a project must have between 1 and {MAX_PATTERN_TRACKS} tracks"
    );
    let mut steps = Vec::new();
    let mut tracks = Vec::new();
    for t in file.tracks {
        steps.push(Some(steps_from_text(&t.steps)?));
        tracks.push(TrackData {
            instrument: Instrument::Drum { sound: t.sound },
            color: t.color % COLORS,
            sample: t.sample.map(|s| base.join(s)),
        });
    }
    Ok(Project {
        bpm: file.bpm,
        sharing: Sharing::Shared,
        selected_pattern: 0,
        tracks,
        patterns: vec![PatternData { name: pattern_name(1), steps }],
    })
}

fn from_v2(file: FileV2, base: &Path) -> Result<Project> {
    let track_count = file.tracks.len();
    ensure!(track_count <= MAX_TRACKS, "a project can have at most {MAX_TRACKS} tracks");
    let tracks = file
        .tracks
        .into_iter()
        .map(|t| TrackData {
            instrument: t.instrument,
            color: t.color % COLORS,
            sample: t.sample.map(|s| base.join(s)),
        })
        .collect();
    let patterns = file
        .patterns
        .into_iter()
        .map(|p| {
            let mut steps = vec![None; track_count];
            for row in p.rows {
                let slot = steps.get_mut(row.track).with_context(|| {
                    format!("pattern '{}' refers to track {}, which does not exist", p.name, row.track + 1)
                })?;
                ensure!(slot.is_none(), "pattern '{}' lists track {} twice", p.name, row.track + 1);
                *slot = Some(steps_from_text(&row.steps)?);
            }
            Ok(PatternData { name: p.name, steps })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Project { bpm: file.bpm, sharing: file.sharing, selected_pattern: file.selected_pattern, tracks, patterns })
}

fn steps_to_text(steps: &Steps) -> String {
    steps.iter().map(|&on| if on { 'x' } else { '.' }).collect()
}

fn steps_from_text(text: &str) -> Result<Steps> {
    let chars: Vec<char> = text.chars().collect();
    ensure!(chars.len() == STEPS, "each track must have {STEPS} steps");
    let mut steps = [false; STEPS];
    for (cell, c) in steps.iter_mut().zip(chars) {
        *cell = match c {
            'x' | 'X' => true,
            '.' | '-' => false,
            other => bail!("unexpected character '{other}' in steps"),
        };
    }
    Ok(steps)
}

/// Path to store for `sample`: relative to `base` when the sample lives in
/// the project folder or below it, absolute otherwise. Always uses `/`.
fn path_for_file(sample: &Path, base: &Path) -> String {
    let stored = match sample.strip_prefix(base) {
        Ok(rel) if !base.as_os_str().is_empty() => rel.to_path_buf(),
        _ => sample.to_path_buf(),
    };
    let parts: Vec<String> = stored
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_string_lossy().into_owned(),
            other => other.as_os_str().to_string_lossy().into_owned(),
        })
        .collect();
    parts.join("/").replace("//", "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("refraktal-{name}-{}", std::process::id()));
        fs::create_dir_all(dir.join("samples")).unwrap();
        dir
    }

    fn drum(sound: &str, color: u8) -> TrackData {
        TrackData::new(Instrument::drum(sound), color)
    }

    /// Two patterns; the clap has a sample next to the project.
    fn example(dir: &Path) -> Project {
        let mut p = Project::demo();
        p.bpm = 128.0;
        let clap = p.add_track(0, drum("clap", 4)).unwrap();
        p.tracks[clap].sample = Some(dir.join("samples").join("clap.wav"));
        let second = p.add_pattern().unwrap();
        p.set_step(second, clap, 9, true);
        p.selected_pattern = second;
        p
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("beat.refraktal");
        let project = example(&dir);
        project.save(&path).unwrap();
        let loaded = Project::load(&path).unwrap();
        fs::remove_dir_all(&dir).ok();
        assert_eq!(loaded, project);
    }

    #[test]
    fn per_pattern_round_trip() {
        let dir = temp_dir("roundtrip-own");
        let path = dir.join("beat.refraktal");
        let mut project = example(&dir);
        project.set_sharing(Sharing::PerPattern).unwrap();
        project.save(&path).unwrap();
        let loaded = Project::load(&path).unwrap();
        fs::remove_dir_all(&dir).ok();
        assert_eq!(loaded, project);
    }

    #[test]
    fn samples_inside_the_project_folder_are_stored_relative() {
        let dir = temp_dir("relative");
        let path = dir.join("beat.refraktal");
        example(&dir).save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_dir_all(&dir).ok();
        assert!(text.contains("\"sample\": \"samples/clap.wav\""), "{text}");
        assert!(!text.contains(&dir.to_string_lossy().into_owned()));
    }

    #[test]
    fn version_1_files_are_upgraded() {
        let v1 = r#"{"format":"refraktal-project","version":1,"bpm":128,"tracks":[
            {"sound":"kick","color":0,"steps":"x...x...x...x..."},
            {"sound":"clap","color":12,"steps":"....x.......x...","sample":"samples/clap.wav"}]}"#;
        let p = Project::from_json(v1, Path::new("/beats")).unwrap();
        assert_eq!(p.bpm, 128.0);
        assert_eq!(p.sharing, Sharing::Shared);
        assert_eq!(p.patterns.len(), 1);
        assert_eq!(p.rows(0), vec![0, 1]);
        assert_eq!(p.tracks[1].instrument, Instrument::drum("clap"));
        assert_eq!(p.tracks[1].color, 4);
        assert_eq!(p.tracks[1].sample, Some(Path::new("/beats").join("samples/clap.wav")));
        assert!(p.steps(0, 0).unwrap()[4]);
        assert!(!p.steps(0, 0).unwrap()[5]);
    }

    #[test]
    fn rejects_broken_files() {
        for text in [
            "not json",
            r#"{"format":"something-else","version":1,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":99,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":1,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":1,"bpm":120,"tracks":[{"sound":"kick","color":0,"steps":"x..."}]}"#,
            // Version 2: no patterns, a row for a missing track, a track listed
            // twice, a shared track missing from a pattern, an unknown instrument.
            r#"{"format":"refraktal-project","version":2,"bpm":120,"tracks":[],"patterns":[]}"#,
            r#"{"format":"refraktal-project","version":2,"bpm":120,"tracks":[],"patterns":[{"name":"P","rows":[{"track":0,"steps":"................"}]}]}"#,
            r#"{"format":"refraktal-project","version":2,"bpm":120,"tracks":[{"instrument":{"type":"drum","sound":"kick"},"color":0}],"patterns":[{"name":"P","rows":[{"track":0,"steps":"................"},{"track":0,"steps":"................"}]}]}"#,
            r#"{"format":"refraktal-project","version":2,"bpm":120,"tracks":[{"instrument":{"type":"drum","sound":"kick"},"color":0}],"patterns":[{"name":"P","rows":[]}]}"#,
            r#"{"format":"refraktal-project","version":2,"bpm":120,"tracks":[{"instrument":{"type":"theremin"},"color":0}],"patterns":[{"name":"P","rows":[{"track":0,"steps":"................"}]}]}"#,
        ] {
            assert!(Project::from_json(text, Path::new("")).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn a_new_project_is_empty() {
        let p = Project::new();
        assert!(p.tracks.is_empty());
        assert_eq!(p.patterns.len(), 1);
        assert!(p.rows(0).is_empty());
        p.check().unwrap();
    }

    #[test]
    fn shared_tracks_join_every_pattern() {
        let mut p = Project::demo();
        let second = p.add_pattern().unwrap();
        assert_eq!(p.rows(second), vec![0, 1, 2]);
        assert_eq!(p.steps(second, 0), Some(&[false; STEPS]), "a new pattern starts empty");
        let clap = p.add_track(second, drum("clap", 3)).unwrap();
        assert_eq!(p.rows(0), vec![0, 1, 2, clap]);
        // Removing a shared track removes it everywhere.
        assert!(p.remove_row(second, 1));
        assert_eq!(p.tracks.len(), 3);
        assert_eq!(p.rows(0), vec![0, 1, 2]);
        p.check().unwrap();
    }

    #[test]
    fn per_pattern_tracks_stay_in_their_pattern() {
        let mut p = Project::new();
        p.set_sharing(Sharing::PerPattern).unwrap();
        let kick = p.add_track(0, drum("kick", 0)).unwrap();
        let second = p.add_pattern().unwrap();
        assert!(p.rows(second).is_empty());
        let tom = p.add_track(second, drum("tom", 1)).unwrap();
        assert_eq!(p.rows(0), vec![kick]);
        assert_eq!(p.rows(second), vec![tom]);
        assert_eq!(p.toggle_step(0, tom, 3), None, "the tom is not in the first pattern");
        // The kick is only used here, so taking it out deletes it.
        assert!(p.remove_row(0, kick));
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.rows(1), vec![0]);
        p.check().unwrap();
    }

    #[test]
    fn a_track_used_elsewhere_survives_leaving_one_pattern() {
        let mut p = Project::demo();
        p.add_pattern().unwrap();
        p.set_step(1, 0, 0, true);
        p.set_sharing(Sharing::PerPattern).unwrap();
        assert!(!p.remove_row(0, 0), "the kick is still used by pattern 2");
        assert_eq!(p.rows(0), vec![1, 2]);
        assert_eq!(p.rows(1), vec![0]);
    }

    #[test]
    fn switching_sharing_keeps_the_beat() {
        let mut p = Project::demo();
        let clap = p.add_track(0, drum("clap", 3)).unwrap(); // no steps anywhere
        let second = p.add_pattern().unwrap();
        p.set_step(second, 1, 7, true);
        p.selected_pattern = second;

        p.set_sharing(Sharing::PerPattern).unwrap();
        assert_eq!(p.rows(0), vec![0, 1, 2], "pattern 1 keeps the tracks it plays");
        assert_eq!(p.rows(second), vec![1, clap], "the unused clap stays in the selected pattern");
        p.check().unwrap();

        let before = p.clone();
        p.set_sharing(Sharing::Shared).unwrap();
        assert_eq!(p.rows(0), vec![0, 1, 2, clap]);
        for pattern in 0..p.patterns.len() {
            for track in 0..p.tracks.len() {
                let old = before.steps(pattern, track).copied().unwrap_or([false; STEPS]);
                assert_eq!(p.steps(pattern, track), Some(&old), "pattern {pattern}, track {track}");
            }
        }
        p.check().unwrap();
    }

    #[test]
    fn sharing_refuses_more_tracks_than_a_pattern_shows() {
        let mut p = Project::new();
        p.set_sharing(Sharing::PerPattern).unwrap();
        p.add_pattern().unwrap();
        for i in 0..MAX_PATTERN_TRACKS {
            p.add_track(0, drum("kick", 0)).unwrap();
            p.add_track(1, drum("hat", 1)).unwrap();
            assert_eq!(p.rows(0).len(), i + 1);
        }
        assert!(p.add_track(0, drum("tom", 2)).is_none(), "pattern 1 is full");
        assert!(p.set_sharing(Sharing::Shared).is_err());
        assert_eq!(p.sharing, Sharing::PerPattern, "a refused switch changes nothing");
    }

    #[test]
    fn limits() {
        let mut p = Project::demo();
        while p.add_track(0, drum("tom", 0)).is_some() {}
        assert_eq!(p.tracks.len(), MAX_PATTERN_TRACKS);
        while p.add_pattern().is_some() {}
        assert_eq!(p.patterns.len(), MAX_PATTERNS);
        p.check().unwrap();
    }

    #[test]
    fn removing_a_pattern_removes_tracks_nobody_uses() {
        let mut p = Project::new();
        p.set_sharing(Sharing::PerPattern).unwrap();
        let kick = p.add_track(0, drum("kick", 0)).unwrap();
        let second = p.add_pattern().unwrap();
        let tom = p.add_track(second, drum("tom", 1)).unwrap();
        let hat = p.add_track(second, drum("hat", 2)).unwrap();
        p.selected_pattern = second;
        assert_eq!(p.remove_pattern(second), Some(vec![hat, tom]));
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tracks[kick].instrument, Instrument::drum("kick"));
        assert_eq!(p.selected_pattern, 0);
        assert_eq!(p.remove_pattern(0), None, "the last pattern stays");
        p.check().unwrap();
    }

    #[test]
    fn pattern_names_are_reused_after_deleting() {
        let mut p = Project::new();
        p.add_pattern().unwrap();
        p.add_pattern().unwrap();
        p.remove_pattern(1).unwrap();
        let again = p.add_pattern().unwrap();
        assert_eq!(p.patterns[again].name, "Pattern 2");
    }
}
