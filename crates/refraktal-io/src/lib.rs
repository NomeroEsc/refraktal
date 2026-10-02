// SPDX-License-Identifier: GPL-3.0-or-later
//! File input and output for Refraktal. Nothing here is real-time safe;
//! call it from the UI or a loader thread, never from the audio thread.

mod project;
mod sample;

pub use project::{PROJECT_EXTENSION, Project, TrackData};
pub use sample::{MAX_SAMPLE_SECONDS, load_sample};
