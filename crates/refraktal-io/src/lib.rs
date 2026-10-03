// SPDX-License-Identifier: GPL-3.0-or-later
//! Files and the project model for Refraktal. Nothing here is real-time
//! safe; call it from the UI or a loader thread, never from the audio thread.

mod project;
mod sample;
mod wav;

pub use project::{
    Instrument, MAX_PATTERN_TRACKS, MAX_PATTERNS, MAX_TRACKS, PROJECT_EXTENSION, PatternData, Project, STEPS,
    Sharing, Steps, TrackData,
};
pub use sample::{MAX_SAMPLE_SECONDS, load_sample};
pub use wav::{encode_wav, save_wav};
