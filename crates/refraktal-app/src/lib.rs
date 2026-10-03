// SPDX-License-Identifier: GPL-3.0-or-later
//! The Refraktal application: audio, window and input. The desktop binary
//! (`src/main.rs`) and the Android library (`crates/refraktal-android`)
//! are thin wrappers around this crate.

mod audio;
#[cfg(any(target_os = "android", test))]
mod android_files;
mod dialogs;
mod export;
mod gui;

#[cfg(not(target_os = "android"))]
mod cli;
#[cfg(not(target_os = "android"))]
mod screenshot;

#[cfg(not(target_os = "android"))]
use std::path::PathBuf;

#[cfg(not(target_os = "android"))]
use anyhow::{Context, bail};
use anyhow::Result;

// The project model, the engine and the screen must agree on sizes. The
// screen shows one pattern, so its row count is the per-pattern limit.
const _: () = assert!(
    refraktal_ui::STEPS == refraktal_engine::STEPS
        && refraktal_io::STEPS == refraktal_engine::STEPS
        && refraktal_io::MAX_TRACKS == refraktal_engine::MAX_TRACKS
        && refraktal_io::MAX_PATTERNS == refraktal_engine::MAX_PATTERNS
        && refraktal_ui::MAX_TRACKS == refraktal_io::MAX_PATTERN_TRACKS
);

#[cfg(not(target_os = "android"))]
const USAGE: &str = "\
Usage: refraktal [options]

Options:
  --cli                     text interface instead of the window
  --export <project> <wav>  render a project to a WAV file and exit
  --screenshot <file.png>   render one frame to a PNG and exit
  --size <W>x<H>            screenshot size in pixels (default 1600x1000)
  --scale <factor>          screenshot UI scale (default 1)
  --show-help               include the help overlay in the screenshot
  --show-notice             include the Android note in the screenshot
  --touch                   screenshot the touch (phone) version of the help
  -v, --verbose             print audio device details
  -h, --help                show this help";

#[cfg(not(target_os = "android"))]
enum Mode {
    Gui,
    Cli,
    Export { project: PathBuf, wav: PathBuf },
    Screenshot { path: PathBuf, width: u32, height: u32, scale: f32, overlay: screenshot::Overlay, touch: bool },
}

/// Desktop entry point: parse the command line and run.
#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> Result<()> {
    let mut mode = Mode::Gui;
    let mut verbose = false;
    let mut size = (1600_u32, 1000_u32);
    let mut scale = 1.0_f32;
    let mut screenshot_path = None;
    let mut overlay = screenshot::Overlay::None;
    let mut touch = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cli" => mode = Mode::Cli,
            "--export" => {
                let project = args.next().context("--export needs a project file")?;
                let wav = args.next().context("--export needs an output file, e.g. beat.wav")?;
                mode = Mode::Export { project: PathBuf::from(project), wav: PathBuf::from(wav) };
            }
            "--show-help" => overlay = screenshot::Overlay::Help,
            "--show-notice" => overlay = screenshot::Overlay::Notice,
            "--touch" => touch = true,
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
        mode = Mode::Screenshot { path, width: size.0, height: size.1, scale, overlay, touch };
    }

    match mode {
        Mode::Screenshot { path, width, height, scale, overlay, touch } => {
            screenshot::run(&path, width, height, scale, overlay, touch)
        }
        Mode::Export { project, wav } => {
            let rendered = export::export_file(&project, &wav)?;
            println!(
                "Exported {}: {:.1} s ({} loops, then a {:.1} s tail)",
                wav.display(),
                rendered.seconds(),
                export::EXPORT_LOOPS,
                rendered.tail_seconds(),
            );
            Ok(())
        }
        Mode::Cli => {
            let mut audio = audio::start(verbose)?;
            println!("Refraktal prototype");
            cli::run(&mut audio.handle)
        }
        Mode::Gui => {
            let audio = audio::start(verbose)?;
            gui::run_desktop(audio)
        }
    }
}

#[cfg(not(target_os = "android"))]
fn parse_size(value: &str) -> Result<(u32, u32)> {
    let (w, h) = value.split_once('x').context("size must look like 1600x1000")?;
    let w: u32 = w.parse().context("bad width")?;
    let h: u32 = h.parse().context("bad height")?;
    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
        bail!("size must be between 64 and 8192 pixels per side");
    }
    Ok((w, h))
}

/// Android entry point, called from `crates/refraktal-android`.
#[cfg(target_os = "android")]
pub fn run_android(app: winit::platform::android::activity::AndroidApp) -> Result<()> {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    // The beat lives in the app's private storage and is saved automatically.
    let app_dir = app.internal_data_path();
    let autosave = app_dir.as_ref().map(|dir| dir.join("current.refraktal"));
    let audio = audio::start(false)?;
    let event_loop = winit::event_loop::EventLoop::builder().with_android_app(app).build()?;
    let notice = app_dir.map(|dir| dir.join("notice-shown"));
    gui::run(audio, event_loop, gui::Options { autosave, touch: true, notice })
}
