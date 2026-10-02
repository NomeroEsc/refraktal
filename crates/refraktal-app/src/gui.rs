// SPDX-License-Identifier: GPL-3.0-or-later
//! Window, input handling and the frame loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use refraktal_dsp::Sample;
use refraktal_engine::{Command, DEFAULT_TRACKS, DrumKind, Event, MAX_TRACKS, Pattern, STEPS, default_pattern};
use refraktal_ui::{FrameState, Hit, Layout, Renderer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::audio::Audio;

const DEFAULT_BPM: f32 = 120.0;
const MIN_BPM: f32 = 40.0;
const MAX_BPM: f32 = 300.0;
const BPM_STEP: f32 = 5.0;

/// What the UI knows about one track.
#[derive(Clone, Copy)]
struct TrackState {
    kind: DrumKind,
    sample_loaded: bool,
    /// Palette index; stays with the track when others are added or removed.
    color: u8,
}

/// Result of decoding a dropped file on a loader thread.
struct LoadResult {
    track: usize,
    path: PathBuf,
    sample: anyhow::Result<Sample>,
}

/// Open the main window and run until it is closed.
pub fn run(audio: Audio) -> Result<()> {
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    let mut app = App::new(audio);
    event_loop.run_app(&mut app)?;
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
                .map(|(i, &kind)| TrackState { kind, sample_loaded: false, color: i as u8 })
                .collect(),
            file_hover: false,
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
        format!("Refraktal – {:.0} BPM", self.bpm)
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
        if let Some(gpu) = &self.gpu {
            gpu.window.set_title(&self.title());
        }
    }

    fn hover(&self) -> Hit {
        match (&self.gpu, self.cursor) {
            (Some(gpu), Some((x, y))) => gpu.layout.hit_test(x, y),
            _ => Hit::None,
        }
    }

    fn click(&mut self) {
        match self.hover() {
            Hit::Play => self.toggle_play(),
            Hit::AddTrack => self.add_track(),
            Hit::Track(track) => {
                // A second click on the selected track cycles its built-in sound.
                if track == self.selected_track && !self.tracks[track].sample_loaded {
                    let kind = self.tracks[track].kind.next();
                    self.tracks[track].kind = kind;
                    self.send(Command::SetDrum { track, kind });
                }
                self.selected_track = track;
                self.send(Command::Trigger(track));
            }
            Hit::Step { track, step } => {
                self.pattern[track][step] = !self.pattern[track][step];
                self.send(Command::ToggleStep { track, step });
            }
            Hit::None => {}
        }
    }

    /// Right click on a track marker switches the track back to its synth.
    fn right_click(&mut self) {
        if let Hit::Track(track) = self.hover() {
            if self.tracks[track].sample_loaded {
                self.tracks[track].sample_loaded = false;
                self.send(Command::SetSample { track, sample: None });
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
        self.tracks.push(TrackState { kind, sample_loaded: false, color });
        self.pattern[index] = [false; STEPS];
        self.send(Command::AddTrack(kind));
        self.send(Command::Trigger(index));
        self.selected_track = index;
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
        self.relayout();
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
        let track = self.selected_track;
        let tx = self.loads_tx.clone();
        std::thread::spawn(move || {
            let sample = refraktal_io::load_sample(&path);
            let _ = tx.send(LoadResult { track, path, sample });
        });
    }

    fn finish_loads(&mut self) {
        while let Ok(result) = self.loads_rx.try_recv() {
            let name = result.path.file_name().map_or_else(
                || result.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            if result.track >= self.tracks.len() {
                continue; // the track was removed while the file was loading
            }
            match result.sample {
                Ok(sample) => {
                    self.tracks[result.track].sample_loaded = true;
                    self.send(Command::SetSample {
                        track: result.track,
                        sample: Some(Arc::new(sample)),
                    });
                    println!("Loaded {name} on track {}", result.track + 1);
                }
                Err(err) => eprintln!("Could not load {name}: {err:#}"),
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
            hover: self.hover(),
            track_count: self.tracks.len(),
            colors: std::array::from_fn(|i| self.tracks.get(i).map_or(0, |t| t.color)),
            selected_track: self.selected_track,
            sample_loaded: std::array::from_fn(|i| self.tracks.get(i).is_some_and(|t| t.sample_loaded)),
            file_hover: self.file_hover,
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                self.click();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                self.right_click();
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
                match event.logical_key {
                    Key::Named(NamedKey::Space) => self.toggle_play(),
                    Key::Named(NamedKey::ArrowUp) => self.change_bpm(BPM_STEP),
                    Key::Named(NamedKey::ArrowDown) => self.change_bpm(-BPM_STEP),
                    Key::Named(NamedKey::Delete | NamedKey::Backspace) => self.remove_selected_track(),
                    Key::Character(ref c) => {
                        let count = self.tracks.len();
                        if let Some(track) = c.parse::<usize>().ok().filter(|n| (1..=count).contains(n)) {
                            self.selected_track = track - 1;
                            self.send(Command::Trigger(track - 1));
                        } else if c.as_str() == "+" || c.as_str() == "=" {
                            self.add_track();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
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
        self.renderer.render(&self.queue, &mut encoder, &view, &self.layout, frame_state);
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);

        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
    }
}
