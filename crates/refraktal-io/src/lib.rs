// SPDX-License-Identifier: GPL-3.0-or-later
//! File input and output for Refraktal. Nothing here is real-time safe;
//! call it from a loader thread, never from the audio thread.

mod sample;

pub use sample::{MAX_SAMPLE_SECONDS, load_sample};
