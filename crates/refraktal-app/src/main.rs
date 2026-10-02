// SPDX-License-Identifier: GPL-3.0-or-later
//! Refraktal: an open-source, pattern-based music studio.

mod audio;
mod cli;
mod gui;
mod screenshot;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

// In debug builds, abort if anything allocates inside the audio callback.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

// The UI and the engine must agree on the grid size.
const _: () = assert!(
    refraktal_ui::STEPS == refraktal_engine::STEPS
        && refraktal_ui::MAX_TRACKS == refraktal_engine::MAX_TRACKS
);

const USAGE: &str = "\
Usage: refraktal [options]

Options:
  --cli                     text interface instead of the window
  --screenshot <file.png>   render one frame to a PNG and exit
  --size <W>x<H>            screenshot size in pixels (default 1600x1000)
  --scale <factor>          screenshot UI scale (default 1)
  -v, --verbose             print audio device details
  -h, --help                show this help";

enum Mode {
    Gui,
    Cli,
    Screenshot { path: PathBuf, width: u32, height: u32, scale: f32 },
}

fn main() -> Result<()> {
    let mut mode = Mode::Gui;
    let mut verbose = false;
    let mut size = (1600_u32, 1000_u32);
    let mut scale = 1.0_f32;
    let mut screenshot_path = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cli" => mode = Mode::Cli,
            "-v" | "--verbose" => verbose = true,
            "--screenshot" => screenshot_path = Some(PathBuf::from(args.next().context("--screenshot needs a file name")?)),
            "--size" => size = parse_size(&args.next().context("--size needs a value like 1600x1000")?)?,
            "--scale" => {
                scale = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .filter(|s: &f32| *s > 0.0)
                    .context("--scale needs a positive number")?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option: {other}\n\n{USAGE}"),
        }
    }
    if let Some(path) = screenshot_path {
        mode = Mode::Screenshot { path, width: size.0, height: size.1, scale };
    }

    match mode {
        Mode::Screenshot { path, width, height, scale } => screenshot::run(&path, width, height, scale),
        Mode::Cli => {
            let mut audio = audio::start(verbose)?;
            println!("Refraktal prototype");
            cli::run(&mut audio.handle)
        }
        Mode::Gui => {
            let audio = audio::start(verbose)?;
            gui::run(audio)
        }
    }
}

fn parse_size(value: &str) -> Result<(u32, u32)> {
    let (w, h) = value.split_once('x').context("size must look like 1600x1000")?;
    let w: u32 = w.parse().context("bad width")?;
    let h: u32 = h.parse().context("bad height")?;
    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
        bail!("size must be between 64 and 8192 pixels per side");
    }
    Ok((w, h))
}
