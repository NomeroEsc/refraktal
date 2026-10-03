// SPDX-License-Identifier: GPL-3.0-or-later
//! Window, input handling and the frame loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use refraktal_dsp::Sample;
use refraktal_engine::{Command, DrumKind, Event, STEPS};
use refraktal_io::{Instrument, PROJECT_EXTENSION, Project, Sharing, TrackData};
use refraktal_ui::{Chip, Content, FrameState, Hit, Layout, MAX_TRACKS as MAX_ROWS, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::audio::Audio;
use crate::dialogs::{self, Answer};
use crate::export;

pub(crate) const MIN_BPM: f32 = 40.0;
pub(crate) const MAX_BPM: f32 = 300.0;
const BPM_STEP: f32 = 5.0;
/// How long a status message stays, including its fade-out.
const STATUS_TIME: Duration = Duration::from_millis(3000);
const STATUS_FADE: Duration = Duration::from_millis(500);

/// Where a decoded sample goes.
#[derive(Clone, Copy)]
enum LoadTarget {
    /// Replace the sound of this project track.
    Track(usize),
    /// Add a new track to this pattern once the file has decoded, so a
    /// file that fails to load leaves nothing behind.
    NewTrack { pattern: usize },
}

/// Result of decoding a file on a loader thread.
struct LoadResult {
    /// Project generation the load was started for; stale results are ignored.
    generation: u64,
    target: LoadTarget,
    path: PathBuf,
    sample: anyhow::Result<Sample>,
}

/// Result of an export on a background thread.
struct ExportResult {
    /// What to tell the user, or what went wrong.
    result: anyhow::Result<String>,
}

/// Save an export on the phone: Downloads/Refraktal, then the share sheet.
#[cfg(target_os = "android")]
fn save_on_phone(name: &str, audio: &[f32]) -> anyhow::Result<String> {
    use crate::android_files::{Outcome, save_to_downloads};
    let bytes = refraktal_io::encode_wav(audio, export::EXPORT_SAMPLE_RATE)?;
    Ok(match save_to_downloads(name, "audio/wav", &bytes)? {
        Outcome::Saved(where_to) => where_to,
        Outcome::NeedsPermission => "Allow storage access, then tap export again".to_owned(),
    })
}

#[cfg(not(target_os = "android"))]
fn save_on_phone(_name: &str, _audio: &[f32]) -> anyhow::Result<String> {
    anyhow::bail!("there is no Downloads folder to save into on this platform")
}

/// What a chip in the sound row adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SoundChip {
    Drum(DrumKind),
    /// Pick an audio file and add a track that plays it (desktop only).
    SampleFile,
}

fn sound_chips() -> Vec<SoundChip> {
    let mut chips: Vec<SoundChip> = DrumKind::ALL.iter().map(|&k| SoundChip::Drum(k)).collect();
    if dialogs::CAN_PICK_FILES {
        chips.push(SoundChip::SampleFile);
    }
    chips
}

/// Each built-in sound has its own color, so a chip and the track it adds match.
fn sound_color(kind: DrumKind) -> u8 {
    DrumKind::ALL.iter().position(|&k| k == kind).unwrap_or(0) as u8
}

/// Platform differences the window needs to know about.
pub struct Options {
    /// Keep the project in this file, saving it automatically (Android).
    pub autosave: Option<PathBuf>,
    /// Start with touch instructions instead of keyboard shortcuts.
    pub touch: bool,
    /// Show a note about Android developer verification once per version,
    /// remembering in this file which version it was last shown for.
    pub notice: Option<PathBuf>,
}

/// Desktop: open the main window and run until it is closed.
#[cfg(not(target_os = "android"))]
pub fn run_desktop(audio: Audio) -> Result<()> {
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    run(audio, event_loop, Options { autosave: None, touch: false, notice: None })
}

/// Run the interface on an existing event loop.
pub fn run(audio: Audio, event_loop: EventLoop<()>, options: Options) -> Result<()> {
    let mut app = App::new(audio);
    app.touch = options.touch;
    if let Some(path) = options.notice {
        let seen = std::fs::read_to_string(&path).unwrap_or_default();
        app.notice_visible = seen.trim() != env!("CARGO_PKG_VERSION");
        app.notice_file = Some(path);
    }
    if let Some(path) = options.autosave {
        if path.exists() {
            match Project::load(&path) {
                Ok(project) => app.apply_project(project),
                Err(err) => eprintln!("Could not restore the last beat: {err:#}"),
            }
        }
        app.autosave = Some(path);
    }
    event_loop.run_app(&mut app)?;
    app.save_autosave();
    match app.error.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    layout: Layout,
    content: Content,
}

struct App {
    /// Keeps the audio stream alive for as long as the window is open.
    audio: Audio,
    gpu: Option<Gpu>,
    /// The document. The engine mirrors it: same track and pattern indices.
    project: Project,
    /// Per project track: its sample is decoded and playing.
    loaded: Vec<bool>,
    playing: bool,
    current_step: Option<usize>,
    cursor: Option<(f32, f32)>,
    pointer_cursor: bool,
    /// Row of the selected track in the current pattern.
    selected_row: usize,
    /// Pattern the engine is playing (it waits for the bar to end before switching).
    playing_pattern: usize,
    exports_tx: Sender<ExportResult>,
    exports_rx: Receiver<ExportResult>,
    exporting: bool,
    /// The note about Android developer verification is on screen.
    notice_visible: bool,
    notice_file: Option<PathBuf>,
    file_hover: bool,
    project_path: Option<PathBuf>,
    dirty: bool,
    generation: u64,
    modifiers: ModifiersState,
    help_visible: bool,
    status: Option<(String, Instant)>,
    autosave: Option<PathBuf>,
    touch: bool,
    /// Ongoing touch: finger id, when it started, where it is now, how far it moved.
    press: Option<(u64, Instant, (f32, f32), f32)>,
    /// Some systems also send mouse events for a tap; ignore those.
    last_touch: Option<Instant>,
    loads_tx: Sender<LoadResult>,
    loads_rx: Receiver<LoadResult>,
    start: Instant,
    error: Option<anyhow::Error>,
}

impl App {
    fn new(audio: Audio) -> Self {
        let (loads_tx, loads_rx) = channel();
        let (exports_tx, exports_rx) = channel();
        let mut app = Self {
            audio,
            gpu: None,
            project: Project::new(),
            loaded: Vec::new(),
            playing: false,
            current_step: None,
            cursor: None,
            pointer_cursor: false,
            selected_row: 0,
            playing_pattern: 0,
            exports_tx,
            exports_rx,
            exporting: false,
            notice_visible: false,
            notice_file: None,
            file_hover: false,
            project_path: None,
            dirty: false,
            generation: 0,
            modifiers: ModifiersState::default(),
            help_visible: false,
            status: None,
            autosave: None,
            touch: false,
            press: None,
            last_touch: None,
            loads_tx,
            loads_rx,
            start: Instant::now(),
            error: None,
        };
        app.apply_project(Project::new());
        app
    }

    fn send(&mut self, cmd: Command) {
        if self.audio.handle.send(cmd).is_err() {
            eprintln!("engine queue is full, command dropped");
        }
    }

    fn pattern(&self) -> usize {
        self.project.selected_pattern
    }

    /// Project tracks shown in the current pattern, top to bottom.
    fn rows(&self) -> Vec<usize> {
        self.project.rows(self.pattern())
    }

    /// Project track shown in `row`, if there is one.
    fn track_at(&self, row: usize) -> Option<usize> {
        self.rows().get(row).copied()
    }

    /// What the screen has to fit.
    fn content(&self) -> Content {
        Content {
            tracks: self.rows().len(),
            patterns: self.project.patterns.len(),
            sounds: sound_chips().len(),
        }
    }

    fn kind(&self, track: usize) -> DrumKind {
        export::drum_kind(&self.project.tracks[track].instrument)
    }

    fn title(&self) -> String {
        let name = self
            .project_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        let mark = if self.dirty { " *" } else { "" };
        format!("Refraktal – {name}{mark} – {:.0} BPM", self.project.bpm)
    }

    fn update_title(&self) {
        if let Some(gpu) = &self.gpu {
            gpu.window.set_title(&self.title());
        }
    }

    /// Show a short message under the sequencer.
    fn notify(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    fn status_for_frame(&mut self) -> Option<(String, f32)> {
        let (message, since) = self.status.as_ref()?;
        let age = since.elapsed();
        if age >= STATUS_TIME {
            self.status = None;
            return None;
        }
        let remaining = (STATUS_TIME - age).as_secs_f32();
        let alpha = (remaining / STATUS_FADE.as_secs_f32()).min(1.0);
        Some((message.clone(), alpha))
    }

    fn track_label(&self, track: usize) -> String {
        let data = &self.project.tracks[track];
        match (&data.sample, self.loaded[track]) {
            (Some(path), true) => path
                .file_stem()
                .map_or_else(|| data.instrument.name().to_owned(), |n| n.to_string_lossy().into_owned()),
            _ => data.instrument.name().to_owned(),
        }
    }

    fn mark_dirty(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.update_title();
        }
    }

    fn toggle_play(&mut self) {
        if self.playing {
            self.send(Command::Stop);
            self.playing = false;
            self.current_step = None;
        } else {
            self.send(Command::Play);
            self.playing = true;
        }
    }

    fn change_bpm(&mut self, delta: f32) {
        self.project.bpm = (self.project.bpm + delta).clamp(MIN_BPM, MAX_BPM);
        self.send(Command::SetBpm(self.project.bpm));
        self.dirty = true;
        self.update_title();
    }

    fn hover(&self) -> Hit {
        match (&self.gpu, self.cursor) {
            (Some(gpu), Some((x, y))) => gpu.layout.hit_test(x, y),
            _ => Hit::None,
        }
    }

    /// Close the Android note and remember that this version showed it.
    fn close_notice(&mut self) {
        self.notice_visible = false;
        if let Some(path) = &self.notice_file {
            if let Err(err) = std::fs::write(path, env!("CARGO_PKG_VERSION")) {
                eprintln!("Could not remember the notice: {err}");
            }
        }
    }

    fn click(&mut self) {
        if self.notice_visible {
            self.close_notice();
            return;
        }
        if self.help_visible {
            self.help_visible = false;
            return;
        }
        match self.hover() {
            Hit::Play => self.toggle_play(),
            Hit::TempoDown => self.change_bpm(-BPM_STEP),
            Hit::TempoUp => self.change_bpm(BPM_STEP),
            Hit::Help => self.help_visible = true,
            Hit::Export => self.export(),
            Hit::Chip(Chip::Pattern(pattern)) => self.select_pattern(pattern),
            Hit::Chip(Chip::AddPattern) => match self.project.add_pattern() {
                Some(pattern) => {
                    self.select_pattern(pattern);
                    self.mark_dirty();
                }
                None => self.notify("A project has at most 16 patterns"),
            },
            Hit::Chip(Chip::Sharing) => self.toggle_sharing(),
            Hit::Chip(Chip::Sound(n)) => match sound_chips().get(n) {
                Some(SoundChip::Drum(kind)) => self.add_track(*kind),
                Some(SoundChip::SampleFile) => {
                    if !self.project.can_add_track(self.pattern()) {
                        self.notify(format!("A pattern holds at most {MAX_ROWS} tracks"));
                    } else if let Some(path) = dialogs::pick_sample_to_open() {
                        let pattern = self.pattern();
                        self.load_sample(LoadTarget::NewTrack { pattern }, path);
                    }
                }
                None => {}
            },
            Hit::Track(row) => {
                let Some(track) = self.track_at(row) else { return };
                // A second click on the selected track cycles its built-in sound.
                if row == self.selected_row && !self.loaded[track] {
                    let kind = self.kind(track).next();
                    self.project.tracks[track].instrument = Instrument::drum(kind.name());
                    self.send(Command::SetDrum { track, kind });
                    self.mark_dirty();
                }
                self.selected_row = row;
                self.send(Command::Trigger(track));
            }
            Hit::Step { track: row, step } => {
                let Some(track) = self.track_at(row) else { return };
                let pattern = self.pattern();
                if self.project.toggle_step(pattern, track, step).is_some() {
                    self.send(Command::ToggleStep { pattern, track, step });
                    self.mark_dirty();
                }
            }
            Hit::None => {}
        }
    }

    /// Right click (or a long press) on a track marker removes its sample,
    /// or the track itself when it plays a built-in sound.
    fn right_click(&mut self) {
        if self.notice_visible {
            self.close_notice();
            return;
        }
        if self.help_visible {
            return;
        }
        let row = match self.hover() {
            Hit::Track(row) => row,
            Hit::Chip(Chip::Pattern(pattern)) => return self.delete_pattern(pattern),
            _ => return,
        };
        let Some(track) = self.track_at(row) else { return };
        if self.loaded[track] {
            self.loaded[track] = false;
            self.project.tracks[track].sample = None;
            self.send(Command::SetSample { track, sample: None });
            self.mark_dirty();
            self.notify("Back to the built-in sound");
        } else {
            let name = self.project.tracks[track].instrument.name().to_owned();
            self.selected_row = row;
            self.remove_selected_track();
            self.notify(format!("Removed {name}"));
        }
    }

    fn touch(&mut self, touch: winit::event::Touch) {
        use winit::event::TouchPhase;
        const LONG_PRESS: Duration = Duration::from_millis(450);
        let at = (touch.location.x as f32, touch.location.y as f32);
        self.touch = true;
        self.last_touch = Some(Instant::now());
        match touch.phase {
            TouchPhase::Started => {
                self.press = Some((touch.id, Instant::now(), at, 0.0));
                self.cursor = Some(at);
            }
            TouchPhase::Moved => {
                if let Some((id, start, last, moved)) = self.press {
                    if id == touch.id {
                        let step = (at.0 - last.0).hypot(at.1 - last.1);
                        self.press = Some((id, start, at, moved + step));
                    }
                }
            }
            TouchPhase::Ended => {
                if let Some((id, start, _, moved)) = self.press.take() {
                    if id == touch.id && moved < 24.0 {
                        self.cursor = Some(at);
                        if start.elapsed() >= LONG_PRESS {
                            self.right_click();
                        } else {
                            self.click();
                        }
                    }
                }
                // No hover highlight after a tap.
                self.cursor = None;
            }
            TouchPhase::Cancelled => {
                self.press = None;
                self.cursor = None;
            }
        }
    }

    fn recently_touched(&self) -> bool {
        self.last_touch.is_some_and(|t| t.elapsed() < Duration::from_millis(600))
    }

    /// Write the project to the autosave file, if this platform uses one.
    fn save_autosave(&mut self) {
        if let Some(path) = &self.autosave {
            match self.project.save(path) {
                Ok(()) => self.dirty = false,
                Err(err) => eprintln!("Could not save the beat: {err:#}"),
            }
        }
    }

    /// Add a track with a built-in sound to the current pattern.
    fn add_track(&mut self, kind: DrumKind) {
        let data = TrackData::new(Instrument::drum(kind.name()), sound_color(kind));
        if let Some(track) = self.push_track(data) {
            self.send(Command::Trigger(track));
        }
    }

    /// Add a track to the current pattern and the engine; select it.
    fn push_track(&mut self, data: TrackData) -> Option<usize> {
        let pattern = self.pattern();
        let kind = export::drum_kind(&data.instrument);
        let Some(track) = self.project.add_track(pattern, data) else {
            self.notify(format!("A pattern holds at most {MAX_ROWS} tracks"));
            return None;
        };
        self.loaded.push(false);
        self.send(Command::AddTrack(kind));
        self.selected_row = self.rows().iter().position(|&t| t == track).unwrap_or(0);
        self.mark_dirty();
        self.relayout();
        Some(track)
    }

    fn select_pattern(&mut self, pattern: usize) {
        if pattern >= self.project.patterns.len() {
            return;
        }
        self.project.selected_pattern = pattern;
        self.send(Command::SelectPattern(pattern));
        self.selected_row = 0;
        self.relayout();
    }

    fn delete_pattern(&mut self, pattern: usize) {
        let name = self.project.patterns.get(pattern).map(|p| p.name.clone()).unwrap_or_default();
        let Some(removed) = self.project.remove_pattern(pattern) else {
            self.notify("The last pattern cannot be deleted");
            return;
        };
        self.send(Command::RemovePattern(pattern));
        for &track in &removed {
            self.loaded.remove(track);
            self.send(Command::RemoveTrack(track));
        }
        if !removed.is_empty() {
            // Indices moved; results of samples still loading would land on the wrong track.
            self.generation += 1;
        }
        let selected = self.pattern();
        self.select_pattern(selected);
        self.mark_dirty();
        self.notify(format!("Deleted {name}"));
    }

    fn toggle_sharing(&mut self) {
        let next = match self.project.sharing {
            Sharing::Shared => Sharing::PerPattern,
            Sharing::PerPattern => Sharing::Shared,
        };
        // Switching never changes what plays, so the engine needs no update.
        match self.project.set_sharing(next) {
            Ok(()) => {
                self.selected_row = 0;
                self.mark_dirty();
                self.relayout();
                self.notify(match next {
                    Sharing::Shared => "Tracks are shared by every pattern",
                    Sharing::PerPattern => "New tracks join only their own pattern",
                });
            }
            Err(err) => self.notify(format!("Cannot share tracks: {err}")),
        }
    }

    /// Render the selected pattern to a WAV file on a background thread.
    /// Desktop asks where to save it; Android saves into Downloads.
    fn export(&mut self) {
        if self.exporting {
            return;
        }
        let target = if dialogs::CAN_PICK_FILES {
            match dialogs::pick_wav_to_save() {
                Some(path) => Some(path.with_extension("wav")),
                None => return,
            }
        } else {
            None
        };
        let project = self.project.clone();
        let name = format!("{}.wav", self.project_name());
        let tx = self.exports_tx.clone();
        self.exporting = true;
        self.notify("Exporting…");
        std::thread::spawn(move || {
            let result = export::load_samples(&project).and_then(|samples| {
                let rendered = export::render(&project, &samples);
                let seconds = rendered.seconds();
                match target {
                    Some(path) => {
                        refraktal_io::save_wav(&path, &rendered.audio, export::EXPORT_SAMPLE_RATE)?;
                        Ok(format!("Exported {} ({seconds:.1} s)", file_name(&path)))
                    }
                    None => save_on_phone(&name, &rendered.audio),
                }
            });
            let _ = tx.send(ExportResult { result });
        });
    }

    fn finish_exports(&mut self) {
        while let Ok(done) = self.exports_rx.try_recv() {
            self.exporting = false;
            match done.result {
                Ok(message) => self.notify(message),
                Err(err) => dialogs::show_error("Could not export", &err),
            }
        }
    }

    /// File name for exports: the project's name, or "beat".
    fn project_name(&self) -> String {
        self.project_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "beat".to_owned(), |n| n.to_string_lossy().into_owned())
    }

    fn remove_selected_track(&mut self) {
        let pattern = self.pattern();
        let Some(track) = self.track_at(self.selected_row) else { return };
        if self.project.remove_row(pattern, track) {
            self.loaded.remove(track);
            self.send(Command::RemoveTrack(track));
            // Indices moved; results of samples still loading would land on the wrong track.
            self.generation += 1;
        } else {
            // Still used by another pattern: only clear it here.
            self.send(Command::SetRow { pattern, track, steps: [false; STEPS] });
        }
        self.selected_row = self.selected_row.min(self.rows().len().saturating_sub(1));
        self.mark_dirty();
        self.relayout();
    }

    /// Replace everything with `project`. Samples load in the background.
    fn apply_project(&mut self, mut project: Project) {
        self.generation += 1;
        self.send(Command::Stop);
        self.playing = false;
        self.current_step = None;

        project.bpm = export::project_bpm(&project);
        // The same commands the exporter uses, so export matches playback.
        for cmd in export::project_commands(&project) {
            self.send(cmd);
        }
        self.loaded = vec![false; project.tracks.len()];
        let samples: Vec<(usize, PathBuf)> = project
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.sample.clone().map(|path| (i, path)))
            .collect();
        self.project = project;
        for (track, path) in samples {
            self.load_sample(LoadTarget::Track(track), path);
        }
        self.selected_row = 0;
        self.playing_pattern = self.project.selected_pattern;
        self.relayout();
    }

    fn new_project(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        self.apply_project(Project::new());
        self.project_path = None;
        self.dirty = false;
        self.update_title();
        self.notify("New project");
    }

    fn open_project(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(path) = dialogs::pick_project_to_open() {
            self.open_path(&path);
        }
    }

    fn open_path(&mut self, path: &std::path::Path) {
        match Project::load(path) {
            Ok(project) => {
                self.apply_project(project);
                self.project_path = Some(path.to_path_buf());
                self.dirty = false;
                self.update_title();
                self.notify(format!("Opened {}", file_name(path)));
            }
            Err(err) => dialogs::show_error("Could not open the project", &err),
        }
    }

    /// Save to the current file, or ask for one. Returns `true` on success.
    fn save_project(&mut self, save_as: bool) -> bool {
        let path = match (&self.project_path, save_as) {
            (Some(path), false) => path.clone(),
            _ => match dialogs::pick_project_to_save() {
                Some(path) => path.with_extension(PROJECT_EXTENSION),
                None => return false,
            },
        };
        match self.project.save(&path) {
            Ok(()) => {
                self.notify(format!("Saved {}", file_name(&path)));
                self.project_path = Some(path);
                self.dirty = false;
                self.update_title();
                true
            }
            Err(err) => {
                dialogs::show_error("Could not save the project", &err);
                false
            }
        }
    }

    /// Ask what to do with unsaved changes. Returns `true` if it is fine to
    /// throw the current project away.
    fn confirm_discard(&mut self) -> bool {
        if !self.dirty || self.autosave.is_some() {
            return true;
        }
        match dialogs::ask_about_unsaved_changes() {
            Answer::Save => self.save_project(false),
            Answer::Discard => true,
            Answer::Cancel => false,
        }
    }

    fn relayout(&mut self) {
        let content = self.content();
        if let Some(gpu) = &mut self.gpu {
            gpu.content = content;
            let size = gpu.window.inner_size();
            gpu.resize(size);
        }
        self.update_cursor_icon();
    }

    /// Decode a dropped file on a background thread so the window stays smooth.
    fn load_file(&mut self, path: PathBuf) {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION)) {
            // A project dropped on the window opens it.
            if self.confirm_discard() {
                self.open_path(&path);
            }
            return;
        }
        let target = match self.track_at(self.selected_row) {
            Some(track) => LoadTarget::Track(track),
            None => LoadTarget::NewTrack { pattern: self.pattern() },
        };
        self.load_sample(target, path);
    }

    fn load_sample(&mut self, target: LoadTarget, path: PathBuf) {
        let generation = self.generation;
        let tx = self.loads_tx.clone();
        std::thread::spawn(move || {
            let sample = refraktal_io::load_sample(&path);
            let _ = tx.send(LoadResult { generation, target, path, sample });
        });
    }

    fn finish_loads(&mut self) {
        while let Ok(result) = self.loads_rx.try_recv() {
            let name = file_name(&result.path);
            if result.generation != self.generation {
                continue; // another project was opened, or tracks were removed
            }
            let sample = match result.sample {
                Ok(sample) => sample,
                Err(err) => {
                    eprintln!("Could not load {name}: {err:#}");
                    self.notify(format!("Could not load {name}: {err}"));
                    continue;
                }
            };
            let track = match result.target {
                LoadTarget::Track(track) if track < self.project.tracks.len() => track,
                LoadTarget::Track(_) => continue,
                LoadTarget::NewTrack { pattern } => {
                    if pattern != self.pattern() {
                        continue; // the user moved on to another pattern
                    }
                    // A sample track gets a color no built-in sound uses.
                    let used: Vec<u8> = self.rows().iter().map(|&t| self.project.tracks[t].color).collect();
                    let color = (DrumKind::ALL.len() as u8..MAX_ROWS as u8).find(|c| !used.contains(c)).unwrap_or(7);
                    match self.push_track(TrackData::new(Instrument::drum(DrumKind::Kick.name()), color)) {
                        Some(track) => track,
                        None => continue,
                    }
                }
            };
            let data = &mut self.project.tracks[track];
            let changed = data.sample.as_deref() != Some(result.path.as_path());
            data.sample = Some(result.path.clone());
            self.loaded[track] = true;
            self.send(Command::SetSample { track, sample: Some(Arc::new(sample)) });
            self.send(Command::Trigger(track));
            if changed {
                self.mark_dirty();
            }
            let row = self.rows().iter().position(|&t| t == track);
            self.notify(match row {
                Some(row) => format!("Loaded {name} on track {}", row + 1),
                None => format!("Loaded {name}"),
            });
        }
    }

    fn update_cursor_icon(&mut self) {
        let wants_pointer = self.hover() != Hit::None;
        if wants_pointer != self.pointer_cursor {
            self.pointer_cursor = wants_pointer;
            if let Some(gpu) = &self.gpu {
                let icon = if wants_pointer { CursorIcon::Pointer } else { CursorIcon::Default };
                gpu.window.set_cursor(icon);
            }
        }
    }

    fn redraw(&mut self) {
        self.finish_loads();
        self.finish_exports();
        // Free samples the engine has replaced; never done on the audio thread.
        self.audio.handle.collect_garbage();
        while let Some(event) = self.audio.handle.poll_event() {
            match event {
                Event::Step(step) if self.playing => self.current_step = Some(step),
                Event::Pattern(pattern) => self.playing_pattern = pattern,
                Event::Step(_) => {}
            }
        }

        let rows = self.rows();
        let pattern = self.pattern();
        let mut grid = [[false; STEPS]; MAX_ROWS];
        for (row, &track) in rows.iter().enumerate().take(MAX_ROWS) {
            grid[row] = self.project.steps(pattern, track).copied().unwrap_or_default();
        }
        let frame_state = FrameState {
            // Wrap time so the shaders keep float precision in long sessions.
            time: self.start.elapsed().as_secs_f32() % 3600.0,
            playing: self.playing,
            current_step: self.current_step,
            pattern: grid,
            hover: if self.help_visible { Hit::None } else { self.hover() },
            bpm: self.project.bpm,
            track_labels: rows.iter().map(|&t| self.track_label(t)).collect(),
            help_visible: self.help_visible,
            notice_visible: self.notice_visible,
            status: self.status_for_frame(),
            track_count: rows.len(),
            colors: std::array::from_fn(|i| rows.get(i).map_or(0, |&t| self.project.tracks[t].color)),
            selected_track: self.selected_row,
            sample_loaded: std::array::from_fn(|i| rows.get(i).is_some_and(|&t| self.loaded[t])),
            file_hover: self.file_hover,
            touch: self.touch,
            autosave: self.autosave.is_some(),
            pattern_count: self.project.patterns.len(),
            selected_pattern: pattern,
            playing_pattern: self.playing_pattern,
            shared_tracks: self.project.sharing == Sharing::Shared,
            sounds: sound_chips()
                .into_iter()
                .map(|chip| match chip {
                    SoundChip::Drum(kind) => (kind.name().to_owned(), Some(sound_color(kind))),
                    SoundChip::SampleFile => ("Sample…".to_owned(), None),
                })
                .collect(),
        };
        if let Some(gpu) = &mut self.gpu {
            gpu.render(&frame_state);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, &self.title(), self.content()) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(err) => {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if self.autosave.is_some() {
                    self.save_autosave();
                    event_loop.exit();
                } else if self.confirm_discard() {
                    event_loop.exit();
                }
            }
            WindowEvent::Touch(touch) => self.touch(touch),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(gpu) = &mut self.gpu {
                    let size = gpu.window.inner_size();
                    gpu.resize(size);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x as f32, position.y as f32));
                self.update_cursor_icon();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.update_cursor_icon();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } if !self.recently_touched() => {
                self.touch = false;
                match button {
                    MouseButton::Left => self.click(),
                    MouseButton::Right => self.right_click(),
                    _ => {}
                }
            }
            WindowEvent::HoveredFile(_) => self.file_hover = true,
            WindowEvent::HoveredFileCancelled => self.file_hover = false,
            WindowEvent::DroppedFile(path) => {
                self.file_hover = false;
                self.load_file(path);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                // Physical keys, so shortcuts work with any keyboard layout.
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                if self.modifiers.control_key() || self.modifiers.super_key() {
                    match code {
                        KeyCode::KeyS => {
                            self.save_project(self.modifiers.shift_key());
                        }
                        KeyCode::KeyO => self.open_project(),
                        KeyCode::KeyN => self.new_project(),
                        KeyCode::KeyE => self.export(),
                        _ => {}
                    }
                    return;
                }
                match code {
                    KeyCode::F1 => self.help_visible = !self.help_visible,
                    KeyCode::Escape => {
                        self.help_visible = false;
                        if self.notice_visible {
                            self.close_notice();
                        }
                    }
                    KeyCode::Space => self.toggle_play(),
                    KeyCode::ArrowUp => self.change_bpm(BPM_STEP),
                    KeyCode::ArrowDown => self.change_bpm(-BPM_STEP),
                    KeyCode::Delete | KeyCode::Backspace => self.remove_selected_track(),
                    _ => {
                        if let Some(row) = digit(code).map(|n| n - 1) {
                            if let Some(track) = self.track_at(row) {
                                self.selected_row = row;
                                self.send(Command::Trigger(track));
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    /// Android destroys the window surface when the app goes to the background.
    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // On a phone, leaving the app should not leave the beat playing.
        if self.autosave.is_some() && self.playing {
            self.toggle_play();
        }
        self.save_autosave();
        self.gpu = None;
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop, title: &str, content: Content) -> Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(960.0, 600.0));
        let window = Arc::new(event_loop.create_window(attributes).context("could not open a window")?);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .context("could not create a drawing surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no compatible graphics adapter found")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .context("could not open the graphics device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow!("the surface supports no texture formats"))?;
        let alpha_mode = caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("the surface is not supported by the graphics adapter"))?;
        config.format = format;
        config.alpha_mode = alpha_mode;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        config.view_formats = vec![];
        surface.configure(&device, &config);

        let renderer = Renderer::new(&device, format, config.width, config.height);
        let layout = Layout::compute(config.width as f32, config.height as f32, window.scale_factor() as f32, content);

        Ok(Self { window, surface, device, queue, config, renderer, layout, content })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.renderer.resize(&self.device, size.width, size.height);
        self.layout =
            Layout::compute(size.width as f32, size.height as f32, self.window.scale_factor() as f32, self.content);
    }

    fn render(&mut self, frame_state: &FrameState) {
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            _ => {
                // Outdated or lost: reconfigure and try again next frame.
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        self.renderer.render(&self.device, &self.queue, &mut encoder, &view, &self.layout, frame_state);
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);

        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
    }
}

fn digit(code: KeyCode) -> Option<usize> {
    Some(match code {
        KeyCode::Digit1 | KeyCode::Numpad1 => 1,
        KeyCode::Digit2 | KeyCode::Numpad2 => 2,
        KeyCode::Digit3 | KeyCode::Numpad3 => 3,
        KeyCode::Digit4 | KeyCode::Numpad4 => 4,
        KeyCode::Digit5 | KeyCode::Numpad5 => 5,
        KeyCode::Digit6 | KeyCode::Numpad6 => 6,
        KeyCode::Digit7 | KeyCode::Numpad7 => 7,
        KeyCode::Digit8 | KeyCode::Numpad8 => 8,
        _ => return None,
    })
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}
