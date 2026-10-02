// SPDX-License-Identifier: GPL-3.0-or-later
//! Window, input handling and the frame loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use refraktal_dsp::Sample;
use refraktal_engine::{Command, DEFAULT_TRACKS, DrumKind, Event, MAX_TRACKS, Pattern, STEPS, default_pattern};
use refraktal_io::{PROJECT_EXTENSION, Project, TrackData};
use refraktal_ui::{FrameState, Hit, Layout, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::audio::Audio;
use crate::dialogs::{self, Answer};

const DEFAULT_BPM: f32 = 120.0;
const MIN_BPM: f32 = 40.0;
const MAX_BPM: f32 = 300.0;
const BPM_STEP: f32 = 5.0;
/// How long a status message stays, including its fade-out.
const STATUS_TIME: Duration = Duration::from_millis(3000);
const STATUS_FADE: Duration = Duration::from_millis(500);

/// What the UI knows about one track.
#[derive(Clone)]
struct TrackState {
    kind: DrumKind,
    sample_loaded: bool,
    /// File the loaded sample came from, saved with the project.
    sample_path: Option<PathBuf>,
    /// Palette index; stays with the track when others are added or removed.
    color: u8,
}

/// Result of decoding a dropped file on a loader thread.
struct LoadResult {
    /// Project generation the load was started for; stale results are ignored.
    generation: u64,
    track: usize,
    path: PathBuf,
    sample: anyhow::Result<Sample>,
}

/// Platform differences the window needs to know about.
pub struct Options {
    /// Keep the project in this file, saving it automatically (Android).
    pub autosave: Option<PathBuf>,
    /// Start with touch instructions instead of keyboard shortcuts.
    pub touch: bool,
}

/// Desktop: open the main window and run until it is closed.
pub fn run_desktop(audio: Audio) -> Result<()> {
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    run(audio, event_loop, Options { autosave: None, touch: false })
}

/// Run the interface on an existing event loop.
pub fn run(audio: Audio, event_loop: EventLoop<()>, options: Options) -> Result<()> {
    let mut app = App::new(audio);
    app.touch = options.touch;
    if let Some(path) = options.autosave {
        if path.exists() {
            match Project::load(&path) {
                Ok(project) => app.apply_project(&project),
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
    track_count: usize,
}

struct App {
    /// Keeps the audio stream alive for as long as the window is open.
    audio: Audio,
    gpu: Option<Gpu>,
    pattern: Pattern,
    playing: bool,
    current_step: Option<usize>,
    bpm: f32,
    cursor: Option<(f32, f32)>,
    pointer_cursor: bool,
    selected_track: usize,
    tracks: Vec<TrackState>,
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
        Self {
            audio,
            gpu: None,
            pattern: default_pattern(),
            playing: false,
            current_step: None,
            bpm: DEFAULT_BPM,
            cursor: None,
            pointer_cursor: false,
            selected_track: 0,
            tracks: DEFAULT_TRACKS
                .iter()
                .enumerate()
                .map(|(i, &kind)| TrackState { kind, sample_loaded: false, sample_path: None, color: i as u8 })
                .collect(),
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
        }
    }

    fn send(&mut self, cmd: Command) {
        if self.audio.handle.send(cmd).is_err() {
            eprintln!("engine queue is full, command dropped");
        }
    }

    fn title(&self) -> String {
        let name = self
            .project_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".to_owned(), |n| n.to_string_lossy().into_owned());
        let mark = if self.dirty { " *" } else { "" };
        format!("Refraktal – {name}{mark} – {:.0} BPM", self.bpm)
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

    fn track_labels(&self) -> Vec<String> {
        self.tracks
            .iter()
            .map(|t| match &t.sample_path {
                Some(path) => path
                    .file_stem()
                    .map_or_else(|| t.kind.name().to_owned(), |n| n.to_string_lossy().into_owned()),
                None => t.kind.name().to_owned(),
            })
            .collect()
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
        self.bpm = (self.bpm + delta).clamp(MIN_BPM, MAX_BPM);
        self.send(Command::SetBpm(self.bpm));
        self.dirty = true;
        self.update_title();
    }

    fn hover(&self) -> Hit {
        match (&self.gpu, self.cursor) {
            (Some(gpu), Some((x, y))) => gpu.layout.hit_test(x, y),
            _ => Hit::None,
        }
    }

    fn click(&mut self) {
        if self.help_visible {
            self.help_visible = false;
            return;
        }
        match self.hover() {
            Hit::Play => self.toggle_play(),
            Hit::AddTrack => self.add_track(),
            Hit::TempoDown => self.change_bpm(-BPM_STEP),
            Hit::TempoUp => self.change_bpm(BPM_STEP),
            Hit::Help => self.help_visible = true,
            Hit::Track(track) => {
                // A second click on the selected track cycles its built-in sound.
                if track == self.selected_track && !self.tracks[track].sample_loaded {
                    let kind = self.tracks[track].kind.next();
                    self.tracks[track].kind = kind;
                    self.send(Command::SetDrum { track, kind });
                    self.mark_dirty();
                }
                self.selected_track = track;
                self.send(Command::Trigger(track));
            }
            Hit::Step { track, step } => {
                self.pattern[track][step] = !self.pattern[track][step];
                self.send(Command::ToggleStep { track, step });
                self.mark_dirty();
            }
            Hit::None => {}
        }
    }

    /// Right click (or a long press) on a track marker removes its sample,
    /// or the track itself when it plays a built-in sound.
    fn right_click(&mut self) {
        if self.help_visible {
            return;
        }
        if let Hit::Track(track) = self.hover() {
            if self.tracks[track].sample_loaded {
                self.tracks[track].sample_loaded = false;
                self.tracks[track].sample_path = None;
                self.send(Command::SetSample { track, sample: None });
                self.mark_dirty();
                self.notify("Back to the built-in sound");
            } else if self.tracks.len() > 1 {
                let name = self.tracks[track].kind.name();
                self.selected_track = track;
                self.remove_selected_track();
                self.notify(format!("Removed {name}"));
            }
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
            match self.to_project().save(path) {
                Ok(()) => self.dirty = false,
                Err(err) => eprintln!("Could not save the beat: {err:#}"),
            }
        }
    }

    fn add_track(&mut self) {
        if self.tracks.len() >= MAX_TRACKS {
            return;
        }
        // Prefer a sound and a color that no track uses yet.
        let kind = DrumKind::ALL
            .into_iter()
            .find(|k| self.tracks.iter().all(|t| t.kind != *k))
            .unwrap_or(DrumKind::Kick);
        let color = (0..MAX_TRACKS as u8)
            .find(|c| self.tracks.iter().all(|t| t.color != *c))
            .unwrap_or(0);
        let index = self.tracks.len();
        self.tracks.push(TrackState { kind, sample_loaded: false, sample_path: None, color });
        self.pattern[index] = [false; STEPS];
        self.send(Command::AddTrack(kind));
        self.send(Command::Trigger(index));
        self.selected_track = index;
        self.mark_dirty();
        self.relayout();
    }

    fn remove_selected_track(&mut self) {
        let index = self.selected_track;
        if self.tracks.len() <= 1 || index >= self.tracks.len() {
            return;
        }
        let count = self.tracks.len();
        self.tracks.remove(index);
        self.pattern[index..count].rotate_left(1);
        self.pattern[count - 1] = [false; STEPS];
        self.send(Command::RemoveTrack(index));
        self.selected_track = index.min(self.tracks.len() - 1);
        self.mark_dirty();
        self.relayout();
    }

    fn to_project(&self) -> Project {
        Project {
            bpm: self.bpm,
            tracks: self
                .tracks
                .iter()
                .zip(self.pattern.iter())
                .map(|(t, steps)| TrackData {
                    sound: t.kind.name().to_owned(),
                    color: t.color,
                    steps: *steps,
                    sample: t.sample_path.clone(),
                })
                .collect(),
        }
    }

    /// Replace everything with `project`. Samples load in the background.
    fn apply_project(&mut self, project: &Project) {
        self.generation += 1;
        self.send(Command::Stop);
        self.playing = false;
        self.current_step = None;

        let kind_of = |t: &TrackData| DrumKind::from_name(&t.sound).unwrap_or(DrumKind::Kick);
        self.pattern = [[false; STEPS]; MAX_TRACKS];
        self.tracks.clear();
        for (index, data) in project.tracks.iter().take(MAX_TRACKS).enumerate() {
            let kind = kind_of(data);
            self.send(if index == 0 { Command::Reset(kind) } else { Command::AddTrack(kind) });
            self.tracks.push(TrackState { kind, sample_loaded: false, sample_path: None, color: data.color });
            for (step, &on) in data.steps.iter().enumerate() {
                if on {
                    self.pattern[index][step] = true;
                    self.send(Command::SetStep { track: index, step, on });
                }
            }
        }
        self.bpm = project.bpm.clamp(MIN_BPM, MAX_BPM);
        self.send(Command::SetBpm(self.bpm));

        for (index, data) in project.tracks.iter().enumerate() {
            if let Some(path) = &data.sample {
                self.load_sample_into(index, path.clone());
            }
        }
        self.selected_track = 0;
        self.relayout();
    }

    fn new_project(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let pattern = default_pattern();
        let project = Project {
            bpm: DEFAULT_BPM,
            tracks: DEFAULT_TRACKS
                .iter()
                .enumerate()
                .map(|(i, kind)| TrackData {
                    sound: kind.name().to_owned(),
                    color: i as u8,
                    steps: pattern[i],
                    sample: None,
                })
                .collect(),
        };
        self.apply_project(&project);
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
                self.apply_project(&project);
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
            _ => {
                match dialogs::pick_project_to_save() {
                    Some(path) => path.with_extension(PROJECT_EXTENSION),
                    None => return false,
                }
            }
        };
        match self.to_project().save(&path) {
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
        let count = self.tracks.len();
        if let Some(gpu) = &mut self.gpu {
            gpu.track_count = count;
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
        self.load_sample_into(self.selected_track, path);
    }

    fn load_sample_into(&mut self, track: usize, path: PathBuf) {
        let generation = self.generation;
        let tx = self.loads_tx.clone();
        std::thread::spawn(move || {
            let sample = refraktal_io::load_sample(&path);
            let _ = tx.send(LoadResult { generation, track, path, sample });
        });
    }

    fn finish_loads(&mut self) {
        while let Ok(result) = self.loads_rx.try_recv() {
            let name = result.path.file_name().map_or_else(
                || result.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            if result.generation != self.generation || result.track >= self.tracks.len() {
                continue; // another project was opened, or the track was removed
            }
            match result.sample {
                Ok(sample) => {
                    let track = &mut self.tracks[result.track];
                    let changed = track.sample_path.as_deref() != Some(result.path.as_path());
                    track.sample_loaded = true;
                    track.sample_path = Some(result.path.clone());
                    self.send(Command::SetSample {
                        track: result.track,
                        sample: Some(Arc::new(sample)),
                    });
                    if changed {
                        self.mark_dirty();
                    }
                    self.notify(format!("Loaded {name} on track {}", result.track + 1));
                }
                Err(err) => {
                    eprintln!("Could not load {name}: {err:#}");
                    self.notify(format!("Could not load {name}: {err}"));
                }
            }
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
        // Free samples the engine has replaced; never done on the audio thread.
        self.audio.handle.collect_garbage();
        while let Some(Event::Step(step)) = self.audio.handle.poll_event() {
            if self.playing {
                self.current_step = Some(step);
            }
        }

        let frame_state = FrameState {
            // Wrap time so the shaders keep float precision in long sessions.
            time: self.start.elapsed().as_secs_f32() % 3600.0,
            playing: self.playing,
            current_step: self.current_step,
            pattern: self.pattern,
            hover: if self.help_visible { Hit::None } else { self.hover() },
            bpm: self.bpm,
            track_labels: self.track_labels(),
            help_visible: self.help_visible,
            status: self.status_for_frame(),
            track_count: self.tracks.len(),
            colors: std::array::from_fn(|i| self.tracks.get(i).map_or(0, |t| t.color)),
            selected_track: self.selected_track,
            sample_loaded: std::array::from_fn(|i| self.tracks.get(i).is_some_and(|t| t.sample_loaded)),
            file_hover: self.file_hover,
            touch: self.touch,
            autosave: self.autosave.is_some(),
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
        match Gpu::new(event_loop, &self.title(), self.tracks.len()) {
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
                        _ => {}
                    }
                    return;
                }
                match code {
                    KeyCode::F1 => self.help_visible = !self.help_visible,
                    KeyCode::Escape => self.help_visible = false,
                    KeyCode::Space => self.toggle_play(),
                    KeyCode::ArrowUp => self.change_bpm(BPM_STEP),
                    KeyCode::ArrowDown => self.change_bpm(-BPM_STEP),
                    KeyCode::Delete | KeyCode::Backspace => self.remove_selected_track(),
                    KeyCode::Equal | KeyCode::NumpadAdd => self.add_track(),
                    _ => {
                        if let Some(track) = digit(code).filter(|&n| (1..=self.tracks.len()).contains(&n)) {
                            self.selected_track = track - 1;
                            self.send(Command::Trigger(track - 1));
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
    fn new(event_loop: &ActiveEventLoop, title: &str, track_count: usize) -> Result<Self> {
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
        let layout = Layout::compute(
            config.width as f32,
            config.height as f32,
            window.scale_factor() as f32,
            track_count,
        );

        Ok(Self { window, surface, device, queue, config, renderer, layout, track_count })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.renderer.resize(&self.device, size.width, size.height);
        self.layout = Layout::compute(
            size.width as f32,
            size.height as f32,
            self.window.scale_factor() as f32,
            self.track_count,
        );
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
