// SPDX-License-Identifier: GPL-3.0-or-later
//! Refraktal: an open-source, pattern-based music studio.

// Release builds on Windows open no console window. Use a debug build
// (`cargo run -- --cli`) for the text interface on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// In debug builds, abort if anything allocates inside the audio callback.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() -> anyhow::Result<()> {
    refraktal::run_desktop()
}
