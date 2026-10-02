// SPDX-License-Identifier: GPL-3.0-or-later
//! Project files (`.refraktal`): a small, versioned JSON document.
//!
//! Sample paths are stored relative to the project file whenever possible,
//! so a project folder can be moved or shared without exposing the
//! author's directory layout.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// File extension for projects, without the dot.
pub const PROJECT_EXTENSION: &str = "refraktal";
const FORMAT: &str = "refraktal-project";
const VERSION: u32 = 1;
const STEPS: usize = 16;
const MAX_TRACKS: usize = 8;

/// A saved beat.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub bpm: f32,
    pub tracks: Vec<TrackData>,
}

/// One track of a saved beat.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackData {
    /// Name of the built-in sound, e.g. `"kick"`.
    pub sound: String,
    /// Palette index of the track color.
    pub color: u8,
    pub steps: [bool; STEPS],
    /// Absolute path of a loaded sample, if any.
    pub sample: Option<PathBuf>,
}

/// On-disk representation. Steps are written as `"x...x..."` so the file
/// stays readable and diffs well in Git.
#[derive(Serialize, Deserialize)]
struct FileV1 {
    format: String,
    version: u32,
    bpm: f32,
    tracks: Vec<TrackV1>,
}

#[derive(Serialize, Deserialize)]
struct TrackV1 {
    sound: String,
    color: u8,
    steps: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample: Option<String>,
}

impl Project {
    /// Write the project to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let base = path.parent().unwrap_or(Path::new(""));
        let file = FileV1 {
            format: FORMAT.to_owned(),
            version: VERSION,
            bpm: self.bpm,
            tracks: self
                .tracks
                .iter()
                .map(|t| TrackV1 {
                    sound: t.sound.clone(),
                    color: t.color,
                    steps: t.steps.iter().map(|&on| if on { 'x' } else { '.' }).collect(),
                    sample: t.sample.as_deref().map(|s| path_for_file(s, base)),
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

    /// Read a project from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("could not open {}", path.display()))?;
        let file: FileV1 = serde_json::from_str(&text).context("this is not a valid Refraktal project")?;
        if file.format != FORMAT {
            bail!("this is not a Refraktal project");
        }
        if file.version > VERSION {
            bail!("this project was saved by a newer version of Refraktal");
        }
        if file.tracks.is_empty() || file.tracks.len() > MAX_TRACKS {
            bail!("a project must have between 1 and {MAX_TRACKS} tracks");
        }
        if !file.bpm.is_finite() {
            bail!("the tempo in this project is invalid");
        }

        let base = path.parent().unwrap_or(Path::new(""));
        let tracks = file
            .tracks
            .into_iter()
            .map(|t| {
                let mut steps = [false; STEPS];
                let chars: Vec<char> = t.steps.chars().collect();
                if chars.len() != STEPS {
                    bail!("each track must have {STEPS} steps");
                }
                for (cell, c) in steps.iter_mut().zip(chars) {
                    *cell = match c {
                        'x' | 'X' => true,
                        '.' | '-' => false,
                        other => bail!("unexpected character '{other}' in steps"),
                    };
                }
                Ok(TrackData {
                    sound: t.sound,
                    color: t.color % MAX_TRACKS as u8,
                    steps,
                    sample: t.sample.map(|s| base.join(s)),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { bpm: file.bpm, tracks })
    }
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

    fn example(dir: &Path) -> Project {
        let mut steps = [false; STEPS];
        steps[0] = true;
        steps[9] = true;
        Project {
            bpm: 128.0,
            tracks: vec![
                TrackData { sound: "kick".into(), color: 0, steps, sample: None },
                TrackData {
                    sound: "clap".into(),
                    color: 4,
                    steps: [false; STEPS],
                    sample: Some(dir.join("samples").join("clap.wav")),
                },
            ],
        }
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
    fn rejects_broken_files() {
        let dir = temp_dir("broken");
        let path = dir.join("bad.refraktal");
        for text in [
            "not json",
            r#"{"format":"something-else","version":1,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":99,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":1,"bpm":120,"tracks":[]}"#,
            r#"{"format":"refraktal-project","version":1,"bpm":120,"tracks":[{"sound":"kick","color":0,"steps":"x..."}]}"#,
        ] {
            fs::write(&path, text).unwrap();
            assert!(Project::load(&path).is_err(), "accepted: {text}");
        }
        fs::remove_dir_all(&dir).ok();
    }
}
