// SPDX-License-Identifier: GPL-3.0-or-later
//! Render one frame without a window and save it as PNG.
//! Handy for devlogs and for checking the look on machines without a display.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow};
use refraktal_engine::{MAX_TRACKS, default_pattern};
use refraktal_ui::{FrameState, Hit, Layout, Renderer};

pub fn run(path: &Path, width: u32, height: u32, scale: f32) -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .context("no graphics adapter found")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .context("could not open the graphics device")?;

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let renderer = Renderer::new(&device, format, width, height);
    let track_count = 4;
    let layout = Layout::compute(width as f32, height as f32, scale, track_count);
    let mut pattern = default_pattern();
    for step in [3, 7, 11, 14] {
        pattern[3][step] = true;
    }
    let frame = FrameState {
        time: 8.0,
        playing: true,
        current_step: Some(4),
        pattern,
        track_count,
        colors: [0, 1, 2, 4, 0, 0, 0, 0],
        hover: Hit::None,
        selected_track: 3,
        sample_loaded: std::array::from_fn(|i| i == 0 && i < MAX_TRACKS),
        file_hover: false,
    };

    // Rows in a texture-to-buffer copy must be aligned to 256 bytes.
    let row_bytes = width * 4;
    let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("screenshot") });
    renderer.render(&queue, &mut encoder, &view, &layout, &frame);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);

    let (tx, rx) = mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).context("GPU did not finish")?;
    rx.recv()
        .map_err(|_| anyhow!("readback was cancelled"))?
        .context("could not read the frame back")?;

    let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
    {
        let data = buffer.get_mapped_range(..).context("could not map the readback buffer")?;
        for row in data.chunks(padded_row as usize) {
            pixels.extend_from_slice(&row[..row_bytes as usize]);
        }
    }
    buffer.unmap();

    let file = File::create(path).with_context(|| format!("could not create {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    println!("Saved {}", path.display());
    Ok(())
}
