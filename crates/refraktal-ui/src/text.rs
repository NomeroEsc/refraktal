// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal GPU text: glyphs are rasterized on demand into an atlas and drawn
//! as instanced quads on top of the frame.

use std::collections::HashMap;

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use bytemuck::{Pod, Zeroable};

const ATLAS_SIZE: u32 = 1024;
const TEXT_WGSL: &str = include_str!("shaders/text.wgsl");
// Chakra Petch, SIL Open Font License 1.1 (see assets/fonts/OFL.txt).
const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-Medium.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/ChakraPetch-SemiBold.ttf");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlyphInstance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    /// Offset of the bitmap's top-left corner from the pen position on the baseline.
    offset: [f32; 2],
    size: [f32; 2],
    uv: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    weight: Weight,
    glyph: u16,
    size: u16,
}

pub(crate) struct TextRenderer {
    regular: FontArc,
    bold: FontArc,
    atlas: wgpu::Texture,
    cache: HashMap<GlyphKey, Option<CachedGlyph>>,
    // Shelf packer state.
    cursor: (u32, u32),
    row_height: u32,
    uploads: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    instances: Vec<GlyphInstance>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    screen: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl TextRenderer {
    pub(crate) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let regular = FontArc::try_from_slice(FONT_REGULAR).expect("bundled font is valid");
        let bold = FontArc::try_from_slice(FONT_BOLD).expect("bundled font is valid");

        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let screen = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("text bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_text"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_text"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity = 512;
        let instance_buffer = create_instance_buffer(device, instance_capacity);

        Self {
            regular,
            bold,
            atlas,
            cache: HashMap::new(),
            cursor: (1, 1),
            row_height: 0,
            uploads: Vec::new(),
            instances: Vec::new(),
            instance_buffer,
            instance_capacity,
            screen,
            bind_group,
            pipeline,
        }
    }

    fn font(&self, weight: Weight) -> &FontArc {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    /// Width of `text` in pixels.
    pub(crate) fn measure(&self, text: &str, size: f32, weight: Weight) -> f32 {
        let font = self.font(weight).as_scaled(PxScale::from(size));
        let mut width = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                width += font.kern(p, id);
            }
            width += font.h_advance(id);
            prev = Some(id);
        }
        width
    }

    /// Queue a line of text with its baseline at `y`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        weight: Weight,
        color: [f32; 4],
        align: Align,
    ) {
        let size = size.round().max(4.0);
        let width = self.measure(text, size, weight);
        let mut pen_x = match align {
            Align::Left => x,
            Align::Center => x - width * 0.5,
            Align::Right => x - width,
        }
        .round();
        let baseline = y.round();

        let font = self.font(weight).clone();
        let scaled = font.as_scaled(PxScale::from(size));
        let mut prev: Option<GlyphId> = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                pen_x += scaled.kern(p, id);
            }
            if let Some(glyph) = self.glyph(&font, weight, id, size) {
                self.instances.push(GlyphInstance {
                    rect: [pen_x + glyph.offset[0], baseline + glyph.offset[1], glyph.size[0], glyph.size[1]],
                    uv: glyph.uv,
                    color,
                });
            }
            pen_x += scaled.h_advance(id);
            prev = Some(id);
        }
    }

    fn glyph(&mut self, font: &FontArc, weight: Weight, id: GlyphId, size: f32) -> Option<CachedGlyph> {
        let key = GlyphKey { weight, glyph: id.0, size: size as u16 };
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), point(0.0, 0.0));
        let entry = font.outline_glyph(glyph).and_then(|outlined| {
            let bounds = outlined.px_bounds();
            let w = bounds.width().ceil() as u32;
            let h = bounds.height().ceil() as u32;
            if w == 0 || h == 0 {
                return None;
            }
            let (ax, ay) = self.allocate(w, h)?;
            let mut pixels = vec![0_u8; (w * h) as usize];
            outlined.draw(|gx, gy, coverage| {
                if gx < w && gy < h {
                    pixels[(gy * w + gx) as usize] = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
                }
            });
            self.uploads.push((ax, ay, w, h, pixels));
            let atlas = ATLAS_SIZE as f32;
            Some(CachedGlyph {
                offset: [bounds.min.x, bounds.min.y],
                size: [w as f32, h as f32],
                uv: [ax as f32 / atlas, ay as f32 / atlas, (ax + w) as f32 / atlas, (ay + h) as f32 / atlas],
            })
        });
        self.cache.insert(key, entry);
        entry
    }

    /// Reserve space in the atlas with a one-pixel gutter.
    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.cursor.0 + w + 1 > ATLAS_SIZE {
            self.cursor = (1, self.cursor.1 + self.row_height + 1);
            self.row_height = 0;
        }
        if self.cursor.1 + h + 1 > ATLAS_SIZE {
            return None; // atlas full; more glyph sizes than any screen needs
        }
        let pos = self.cursor;
        self.cursor.0 += w + 1;
        self.row_height = self.row_height.max(h);
        Some(pos)
    }

    /// Upload new glyphs and queued instances. Call once per frame.
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        for (x, y, w, h, pixels) in self.uploads.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(device, self.instance_capacity);
        }
        if !self.instances.is_empty() {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances));
        }
        let screen = [width as f32, height as f32, 0.0, 0.0];
        queue.write_buffer(&self.screen, 0, bytemuck::cast_slice(&screen));
    }

    /// Draw everything queued since the last call, on top of `target`.
    pub(crate) fn render(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let count = self.instances.len() as u32;
        self.instances.clear();
        if count == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..4, 0..count);
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("glyph instances"),
        size: (capacity * std::mem::size_of::<GlyphInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
