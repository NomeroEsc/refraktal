// SPDX-License-Identifier: GPL-3.0-or-later
//! wgpu renderer: backdrop → dual Kawase blur → glass composite.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::layout::{Chip, Hit, Layout, MAX_CHIPS, MAX_TRACKS, STEPS};
use crate::text::TextRenderer;

const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Number of downsample levels in the blur chain (each halves the size).
const BLUR_LEVELS: usize = 4;
const BLUR_OFFSET: f32 = 1.2;

const BACKGROUND_WGSL: &str =
    concat!(include_str!("shaders/common.wgsl"), include_str!("shaders/background.wgsl"));
const BLUR_WGSL: &str =
    concat!(include_str!("shaders/common.wgsl"), include_str!("shaders/blur.wgsl"));
const COMPOSITE_WGSL: &str =
    concat!(include_str!("shaders/common.wgsl"), include_str!("shaders/composite.wgsl"));

/// Everything that changes from frame to frame.
#[derive(Clone, Debug)]
pub struct FrameState {
    /// Seconds since start; drives the backdrop animation.
    pub time: f32,
    pub playing: bool,
    pub current_step: Option<usize>,
    /// One row of steps per track slot.
    pub pattern: [[bool; STEPS]; MAX_TRACKS],
    pub track_count: usize,
    /// Palette index (0–7) of each track's color.
    pub colors: [u8; MAX_TRACKS],
    pub hover: Hit,
    /// Track that receives dropped sample files.
    pub selected_track: usize,
    /// Which tracks play a loaded sample instead of the built-in synth.
    pub sample_loaded: [bool; MAX_TRACKS],
    /// A file is being dragged over the window.
    pub file_hover: bool,
    pub bpm: f32,
    /// Name shown next to each track: its sound or sample file.
    pub track_labels: Vec<String>,
    pub help_visible: bool,
    /// Show touch instructions instead of keyboard shortcuts.
    pub touch: bool,
    /// The project is saved automatically (no save shortcuts).
    pub autosave: bool,
    /// A short message under the sequencer and its opacity (0–1).
    pub status: Option<(String, f32)>,
    /// Patterns in the project.
    pub pattern_count: usize,
    /// The pattern being edited.
    pub selected_pattern: usize,
    /// The pattern the engine plays; differs from the selected one until
    /// the current bar ends.
    pub playing_pattern: usize,
    /// Every track is in every pattern (otherwise each pattern has its own).
    pub shared_tracks: bool,
    /// Labels of the sound chips, and the palette color of each (`None`
    /// for chips that are not a sound, like "Sample…").
    pub sounds: Vec<(String, Option<u8>)>,
}

/// Must match `struct Globals` in `shaders/common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 4],
    sequencer: [f32; 4],
    transport: [f32; 4],
    radii: [f32; 4],
    play: [f32; 4],
    grid: [f32; 4],
    grid2: [f32; 4],
    dots: [f32; 4],
    hover: [f32; 4],
    pattern: [[u32; 4]; 2],
    tracks: [u32; 4],
    colors: [u32; 4],
    overlay: [f32; 4],
    overlay_info: [f32; 4],
    tempo_buttons: [f32; 4],
    help_button: [f32; 4],
    export_button: [f32; 4],
    browser: [f32; 4],
    browser_info: [f32; 4],
    chips: [[f32; 4]; MAX_CHIPS],
    chip_flags: [[u32; 4]; MAX_CHIPS / 4],
}

// `common.wgsl` sizes the chip arrays with literals.
const _: () = assert!(MAX_CHIPS == 24);

// Chip flags; keep in sync with `composite.wgsl`.
const CHIP_SELECTED: u32 = 1;
const CHIP_PLAYING: u32 = 2;
const CHIP_HOVERED: u32 = 4;
const CHIP_COLORED: u32 = 8;
/// Palette index of a colored chip, in bits 8 to 11.
const CHIP_COLOR_SHIFT: u32 = 8;

/// Must match `struct BlurParams` in `shaders/blur.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    texel: [f32; 2],
    offset: f32,
    _pad: f32,
}

struct BlurPass {
    bind_group: wgpu::BindGroup,
    target: usize,
    upsample: bool,
}

/// Size-dependent GPU resources, rebuilt on resize.
struct Targets {
    scene_view: wgpu::TextureView,
    level_views: Vec<wgpu::TextureView>,
    blur_passes: Vec<BlurPass>,
    composite_bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    width: u32,
    height: u32,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
    blur_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    background_pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    up_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    background_bind_group: wgpu::BindGroup,
    targets: Targets,
    text: TextRenderer,
}

impl Renderer {
    /// Create a renderer that draws into textures of `target_format`.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let (width, height) = (width.max(1), height.max(1));

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[uniform_entry(0)],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur layout"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite layout"),
            entries: &[uniform_entry(0), texture_entry(1), texture_entry(2), sampler_entry(3)],
        });

        let background_module = shader(device, "background", BACKGROUND_WGSL);
        let blur_module = shader(device, "blur", BLUR_WGSL);
        let composite_module = shader(device, "composite", COMPOSITE_WGSL);

        let background_pipeline = fullscreen_pipeline(
            device,
            "background",
            &globals_layout,
            &background_module,
            "fs_main",
            SCENE_FORMAT,
        );
        let down_pipeline =
            fullscreen_pipeline(device, "blur down", &blur_layout, &blur_module, "fs_down", SCENE_FORMAT);
        let up_pipeline =
            fullscreen_pipeline(device, "blur up", &blur_layout, &blur_module, "fs_up", SCENE_FORMAT);
        let composite_pipeline = fullscreen_pipeline(
            device,
            "composite",
            &composite_layout,
            &composite_module,
            "fs_main",
            target_format,
        );

        let background_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("background bind group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });

        let targets = Targets::new(
            device,
            &blur_layout,
            &composite_layout,
            &globals,
            &sampler,
            width,
            height,
        );

        let text = TextRenderer::new(device, target_format);

        Self {
            text,
            width,
            height,
            globals,
            sampler,
            blur_layout,
            composite_layout,
            background_pipeline,
            down_pipeline,
            up_pipeline,
            composite_pipeline,
            background_bind_group,
            targets,
        }
    }

    /// Rebuild size-dependent resources. Cheap to call with an unchanged size.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        self.targets = Targets::new(
            device,
            &self.blur_layout,
            &self.composite_layout,
            &self.globals,
            &self.sampler,
            width,
            height,
        );
    }

    /// Record one frame into `encoder`, drawing into `target`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        layout: &Layout,
        frame: &FrameState,
    ) {
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&self.globals(layout, frame)));

        // 1. Backdrop.
        draw(encoder, "background", &self.targets.scene_view, &self.background_pipeline, &self.background_bind_group);

        // 2. Blur chain.
        for pass in &self.targets.blur_passes {
            let pipeline = if pass.upsample { &self.up_pipeline } else { &self.down_pipeline };
            draw(encoder, "blur", &self.targets.level_views[pass.target], pipeline, &pass.bind_group);
        }

        // 3. Glass and controls.
        draw(encoder, "composite", target, &self.composite_pipeline, &self.targets.composite_bind_group);

        // 4. Text on top.
        crate::hud::queue(&mut self.text, layout, frame);
        self.text.prepare(device, queue, self.width, self.height);
        self.text.render(encoder, target);
    }

    fn globals(&self, layout: &Layout, frame: &FrameState) -> Globals {
        // hover.z identifies a transport button: 1 play, 2 tempo down, 3 tempo up, 4 help, 5 export.
        let (hover_track, hover_step, hover_button) = match frame.hover {
            Hit::Step { track, step } => (track as f32, step as f32, 0.0),
            Hit::Play => (-1.0, -1.0, 1.0),
            Hit::TempoDown => (-1.0, -1.0, 2.0),
            Hit::TempoUp => (-1.0, -1.0, 3.0),
            Hit::Help => (-1.0, -1.0, 4.0),
            Hit::Export => (-1.0, -1.0, 5.0),
            Hit::Track(track) => (track as f32, -1.0, 0.0),
            Hit::Chip(_) | Hit::None => (-1.0, -1.0, 0.0),
        };
        let current = match (frame.playing, frame.current_step) {
            (true, Some(step)) => step as f32,
            _ => -1.0,
        };
        let mut pattern = [[0_u32; 4]; 2];
        for (track, row) in frame.pattern.iter().enumerate() {
            for (step, &on) in row.iter().enumerate() {
                if on {
                    pattern[track / 4][track % 4] |= 1 << step;
                }
            }
        }
        let packed_colors = frame
            .colors
            .iter()
            .enumerate()
            .fold(0_u32, |packed, (i, &c)| packed | (u32::from(c) & 0xF) << (i * 4));
        let mut chips = [[0.0_f32; 4]; MAX_CHIPS];
        let mut chip_flags = [[0_u32; 4]; MAX_CHIPS / 4];
        for (i, slot) in layout.chips().iter().enumerate() {
            chips[i] = slot.rect.to_array();
            let mut flags = if frame.hover == Hit::Chip(slot.chip) { CHIP_HOVERED } else { 0 };
            match slot.chip {
                Chip::Pattern(p) => {
                    if p == frame.selected_pattern {
                        flags |= CHIP_SELECTED;
                    }
                    if p == frame.playing_pattern && frame.playing {
                        flags |= CHIP_PLAYING;
                    }
                }
                Chip::Sound(n) => {
                    if let Some((_, Some(color))) = frame.sounds.get(n) {
                        flags |= CHIP_COLORED | (u32::from(*color) & 0xF) << CHIP_COLOR_SHIFT;
                    }
                }
                Chip::AddPattern | Chip::Sharing => {}
            }
            chip_flags[i / 4][i % 4] = flags;
        }

        let sample_mask = frame
            .sample_loaded
            .iter()
            .enumerate()
            .fold(0_u32, |mask, (i, &loaded)| if loaded { mask | 1 << i } else { mask });

        Globals {
            screen: [self.width as f32, self.height as f32, frame.time, layout.scale],
            sequencer: layout.sequencer.to_array(),
            transport: layout.transport.to_array(),
            radii: [layout.panel_radius, layout.transport.h * 0.5, 0.0, 0.0],
            play: [
                layout.play_center.0,
                layout.play_center.1,
                layout.play_radius,
                if frame.playing { 1.0 } else { 0.0 },
            ],
            grid: [layout.grid_origin.0, layout.grid_origin.1, layout.cell, layout.gap],
            grid2: [layout.beat_gap, layout.row_gap, layout.label_x, current],
            dots: layout.dots,
            hover: [hover_track, hover_step, hover_button, 0.0],
            pattern,
            tracks: [
                frame.selected_track as u32,
                sample_mask,
                u32::from(frame.file_hover),
                layout.track_count as u32,
            ],
            colors: [packed_colors, 0, 0, 0],
            overlay: layout.help.to_array(),
            overlay_info: [if frame.help_visible { 1.0 } else { 0.0 }, 28.0 * layout.scale, 0.0, 0.0],
            tempo_buttons: layout.tempo_buttons,
            help_button: [layout.help_button.0, layout.help_button.1, layout.help_button.2, 0.0],
            export_button: [layout.export_button.0, layout.export_button.1, layout.export_button.2, 0.0],
            browser: layout.browser.to_array(),
            browser_info: [
                layout.panel_radius * 0.8,
                layout.chips().first().map_or(0.0, |c| c.rect.h * 0.5),
                layout.chip_count as f32,
                0.0,
            ],
            chips,
            chip_flags,
        }
    }
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        blur_layout: &wgpu::BindGroupLayout,
        composite_layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        width: u32,
        height: u32,
    ) -> Self {
        let scene_view = render_texture(device, "scene", width, height);
        let level_sizes: Vec<(u32, u32)> =
            (1..=BLUR_LEVELS).map(|i| ((width >> i).max(1), (height >> i).max(1))).collect();
        let level_views: Vec<wgpu::TextureView> = level_sizes
            .iter()
            .map(|&(w, h)| render_texture(device, "blur level", w, h))
            .collect();

        let mut blur_passes = Vec::with_capacity(BLUR_LEVELS * 2 - 1);
        let mut add_pass = |source: &wgpu::TextureView, target: usize, upsample: bool| {
            let (w, h) = level_sizes[target];
            let params = BlurParams {
                texel: [1.0 / w as f32, 1.0 / h as f32],
                offset: BLUR_OFFSET,
                _pad: 0.0,
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("blur params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blur bind group"),
                layout: blur_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
                ],
            });
            blur_passes.push(BlurPass { bind_group, target, upsample });
        };

        // Down: scene → 1/2 → 1/4 → 1/8 → 1/16. Up: back to 1/2.
        add_pass(&scene_view, 0, false);
        for i in 1..BLUR_LEVELS {
            add_pass(&level_views[i - 1], i, false);
        }
        for i in (0..BLUR_LEVELS - 1).rev() {
            add_pass(&level_views[i + 1], i, true);
        }

        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bind group"),
            layout: composite_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene_view) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&level_views[0]),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });

        Self { scene_view, level_views, blur_passes, composite_bind_group }
    }
}

fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn render_texture(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    bind_group_layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn draw(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}
