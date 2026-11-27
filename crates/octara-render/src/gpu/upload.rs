//! Frame-local CPU uploads into reusable GPU storage buffers.

use super::atlas::{RecordError, SceneTextures};
use super::glyph_pipeline::GlyphInst;
use super::instances::{DrawInst, GlyphClip, GlyphDraw, GradientInst, ImageInst, Instances, MaterialInst, QuadInst, ShadowInst};
use super::pipeline::{Globals, PipelineKind};
use super::plan::PassBounds;
use crate::shape::ClipNode;

const STORAGE: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE.union(wgpu::BufferUsages::COPY_DST);

fn buffer(device: &wgpu::Device, label: &'static str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
}

struct Storage(wgpu::Buffer, &'static str);

impl Storage {
    fn new<T>(device: &wgpu::Device, label: &'static str) -> Self {
        Self(buffer(device, label, size_of::<T>() as u64, STORAGE), label)
    }

    fn prepare<T>(&mut self, device: &wgpu::Device, data: &[T], limit: u64) -> Result<(), RecordError> {
        let bytes = size_of_val(data) as u64;
        if bytes > limit {
            return Err(RecordError::InstanceBufferSize { bytes, limit });
        }
        if bytes > self.0.size() {
            self.0 = buffer(device, self.1, bytes.next_power_of_two().min(limit), STORAGE);
        }
        Ok(())
    }

    fn write<T: bytemuck::Pod>(&self, queue: &wgpu::Queue, data: &[T]) {
        if !data.is_empty() {
            queue.write_buffer(&self.0, 0, bytemuck::cast_slice(data));
        }
    }
}

pub(super) struct FrameBuffers {
    globals: wgpu::Buffer,
    globals_stride: usize,
    clips: Storage,
    draws: Storage,
    quads: Storage,
    shadows: Storage,
    images: Storage,
    glyphs: Storage,
    glyph_draws: Storage,
    glyph_clips: Storage,
    materials: Storage,
    gradients: Storage,
    sampler: wgpu::Sampler,
}

impl FrameBuffers {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let globals_stride = size_of::<Globals>().next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as usize);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("octara image sampler"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            globals: buffer(
                device,
                "octara globals",
                size_of::<Globals>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            globals_stride,
            clips: Storage::new::<ClipNode>(device, "octara clip nodes"),
            draws: Storage::new::<DrawInst>(device, "octara draw geometry"),
            quads: Storage::new::<QuadInst>(device, "octara quad instances"),
            shadows: Storage::new::<ShadowInst>(device, "octara shadow instances"),
            images: Storage::new::<ImageInst>(device, "octara image instances"),
            glyphs: Storage::new::<GlyphInst>(device, "octara glyph instances"),
            glyph_draws: Storage::new::<GlyphDraw>(device, "octara glyph depth and clip indices"),
            glyph_clips: Storage::new::<GlyphClip>(device, "octara glyph clips"),
            materials: Storage::new::<MaterialInst>(device, "octara material instances"),
            gradients: Storage::new::<GradientInst>(device, "octara gradient buffer"),
            sampler,
        }
    }

    pub(super) fn prepare(&mut self, device: &wgpu::Device, data: &FrameData) -> Result<(), RecordError> {
        let limits = device.limits();
        let limit = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        self.clips.prepare(device, &data.clips, limit)?;
        self.draws.prepare(device, &data.instances.draws, limit)?;
        self.quads.prepare(device, &data.instances.quads, limit)?;
        self.shadows.prepare(device, &data.instances.shadows, limit)?;
        self.images.prepare(device, &data.instances.images, limit)?;
        self.glyphs.prepare(device, &data.instances.glyphs, limit)?;
        self.glyph_draws.prepare(device, &data.instances.glyph_draws, limit)?;
        self.glyph_clips.prepare(device, &data.instances.glyph_clips, limit)?;
        self.materials.prepare(device, &data.instances.materials, limit)?;
        self.gradients.prepare(device, &data.instances.gradients, limit)?;
        Ok(())
    }

    pub(super) fn prepare_projections(
        &mut self,
        device: &wgpu::Device,
        bounds: &[PassBounds],
        window: [u32; 2],
        layer_views: &[wgpu::TextureView],
    ) -> Vec<u8> {
        // One aligned, distinct projection for each target in the submission.
        let uniform_size = size_of::<Globals>();
        let bytes_len = layer_views.len() * self.globals_stride + uniform_size;
        if self.globals.size() < bytes_len as u64 {
            self.globals = buffer(
                device,
                "octara globals",
                bytes_len.next_power_of_two() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            );
        }
        let mut uniforms = vec![0; bytes_len];
        let sizes = std::iter::once(window).chain(layer_views.iter().map(|view| [view.texture().width(), view.texture().height()]));
        for (index, (bounds, extent)) in bounds.iter().zip(sizes).enumerate() {
            let globals = Globals { size: extent.map(|n| n as f32), origin: bounds.origin.map(|n| n as f32) };
            let at = index * self.globals_stride;
            uniforms[at..at + uniform_size].copy_from_slice(bytemuck::bytes_of(&globals));
        }
        uniforms
    }

    pub(super) fn projection_offset(&self, pass: usize) -> u32 {
        u32::try_from(pass * self.globals_stride).expect("pass uniform offset")
    }

    pub(super) fn bind(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        kind: PipelineKind,
        atlas: Option<&wgpu::TextureView>,
        layer: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let payload = match kind {
            PipelineKind::Quad => &self.quads,
            PipelineKind::Shadow => &self.shadows,
            PipelineKind::Image => &self.images,
            PipelineKind::Glyph => &self.glyphs,
            PipelineKind::Material => &self.materials,
        };
        let draws = if kind == PipelineKind::Glyph { &self.glyph_draws } else { &self.draws };
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.globals,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
                }),
            },
            wgpu::BindGroupEntry { binding: 2, resource: payload.0.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.clips.0.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: draws.0.as_entire_binding() },
        ];
        if matches!(kind, PipelineKind::Quad | PipelineKind::Image) {
            entries.push(wgpu::BindGroupEntry { binding: 7, resource: self.gradients.0.as_entire_binding() });
        }
        if kind == PipelineKind::Glyph {
            entries.push(wgpu::BindGroupEntry { binding: 7, resource: self.glyph_clips.0.as_entire_binding() });
        }
        if let Some(atlas) = atlas {
            entries.push(wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(atlas) });
        }
        if let Some(layer) = layer {
            entries.extend([
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(layer) },
            ]);
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("octara"), layout, entries: &entries })
    }

    pub(super) fn stage<'a>(&'a self, textures: &'a mut SceneTextures, data: FrameData) -> FrameUploads<'a> {
        FrameUploads { textures, buffers: self, data }
    }
}

pub(super) struct FrameData {
    pub(super) instances: Instances,
    pub(super) uniforms: Vec<u8>,
    pub(super) clips: Vec<ClipNode>,
}

pub(super) struct FrameUploads<'renderer> {
    textures: &'renderer mut SceneTextures,
    buffers: &'renderer FrameBuffers,
    data: FrameData,
}

impl FrameUploads<'_> {
    pub(super) fn bytes(&self) -> u64 {
        let i = &self.data.instances;
        self.textures.atlases.upload_bytes
            + (self.data.uniforms.len()
                + size_of_val(i.draws.as_slice())
                + size_of_val(i.quads.as_slice())
                + size_of_val(i.shadows.as_slice())
                + size_of_val(i.images.as_slice())
                + size_of_val(i.glyphs.as_slice())
                + size_of_val(i.glyph_draws.as_slice())
                + size_of_val(i.glyph_clips.as_slice())
                + size_of_val(i.materials.as_slice())
                + size_of_val(i.gradients.as_slice())
                + size_of_val(self.data.clips.as_slice())) as u64
    }

    pub(super) fn queue(&self, queue: &wgpu::Queue) {
        self.textures.atlases.queue_uploads(queue);
        queue.write_buffer(&self.buffers.globals, 0, &self.data.uniforms);
        let i = &self.data.instances;
        self.buffers.clips.write(queue, &self.data.clips);
        self.buffers.draws.write(queue, &i.draws);
        self.buffers.quads.write(queue, &i.quads);
        self.buffers.shadows.write(queue, &i.shadows);
        self.buffers.images.write(queue, &i.images);
        self.buffers.glyphs.write(queue, &i.glyphs);
        self.buffers.glyph_draws.write(queue, &i.glyph_draws);
        self.buffers.glyph_clips.write(queue, &i.glyph_clips);
        self.buffers.materials.write(queue, &i.materials);
        self.buffers.gradients.write(queue, &i.gradients);
    }

    pub(super) fn commit(&mut self) {
        self.textures.atlases.commit_uploads();
    }
}

impl Drop for FrameUploads<'_> {
    fn drop(&mut self) {
        self.textures.finish_frame();
    }
}
