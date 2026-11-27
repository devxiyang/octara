//! Reusable scene buffers and uploads that commit with a recorded frame.

use super::RecordError;
use super::atlas::SceneTextures;
use super::batch::Inst;
use super::glyph_pipeline::GlyphInst;
use super::pipeline::Globals;
use super::plan::PassBounds;
use crate::shape::ClipNode;

pub(super) struct FrameBuffers {
    globals: wgpu::Buffer,
    globals_stride: usize,
    clip_nodes: wgpu::Buffer,
    instances: Option<wgpu::Buffer>,
    glyphs: Option<wgpu::Buffer>,
}

impl FrameBuffers {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_stride = size_of::<Globals>().next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as usize);
        let clip_nodes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("octara clips"),
            size: size_of::<ClipNode>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { globals, globals_stride, clip_nodes, instances: None, glyphs: None }
    }

    pub(super) fn prepare_instances(&mut self, device: &wgpu::Device, instances: &[Inst]) {
        let need = size_of_val(instances).max(size_of::<Inst>()) as u64;
        if self.instances.as_ref().is_none_or(|buffer| buffer.size() < need) {
            self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara instances"),
                size: need.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
    }

    pub(super) fn prepare_glyphs(&mut self, device: &wgpu::Device, glyphs: &[GlyphInst]) -> Result<(), RecordError> {
        let bytes = size_of_val(glyphs) as u64;
        let limits = device.limits();
        let limit = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        if bytes > limit {
            return Err(RecordError::GlyphBufferSize { bytes, limit });
        }
        if bytes > 0 && self.glyphs.as_ref().is_none_or(|buffer| buffer.size() < bytes) {
            self.glyphs = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara glyph instances"),
                size: bytes.next_power_of_two().min(limit),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
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
            self.globals = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara globals"),
                size: bytes_len.next_power_of_two() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
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

    pub(super) fn prepare_clips(&mut self, device: &wgpu::Device, clips: &[ClipNode]) {
        let need = size_of_val(clips) as u64;
        if need > self.clip_nodes.size() {
            let capacity = need.next_power_of_two().min(device.limits().max_storage_buffer_binding_size);
            self.clip_nodes = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara clips"),
                size: capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    pub(super) fn instances(&self) -> &wgpu::Buffer {
        self.instances.as_ref().expect("a prepared instance buffer")
    }

    pub(super) fn projection_offset(&self, pass: usize) -> u32 {
        u32::try_from(pass * self.globals_stride).expect("pass uniform offset")
    }

    pub(super) fn bind_texture(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        atlases: [&wgpu::TextureView; 2],
        layer: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let [mask, images] = atlases;
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octara"),
            layout,
            entries: &[
                self.bind_globals(),
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(mask) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(images) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(layer) },
                wgpu::BindGroupEntry { binding: 5, resource: self.clip_nodes.as_entire_binding() },
            ],
        })
    }

    pub(super) fn bind_glyphs(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        glyphs: &[GlyphInst],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("octara glyphs"),
            layout,
            entries: &[
                self.bind_globals(),
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: self.glyphs.as_ref().expect("a prepared glyph buffer"),
                        offset: 0,
                        size: wgpu::BufferSize::new(size_of_val(glyphs) as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 5, resource: self.clip_nodes.as_entire_binding() },
            ],
        })
    }

    fn bind_globals(&self) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.globals,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<Globals>() as u64),
            }),
        }
    }

    pub(super) fn stage<'a>(&'a self, textures: &'a mut SceneTextures, data: FrameData) -> FrameUploads<'a> {
        FrameUploads {
            textures,
            instance_buffer: self.instances(),
            glyph_buffer: self.glyphs.as_ref(),
            uniform_buffer: &self.globals,
            clip_buffer: &self.clip_nodes,
            data,
        }
    }
}

pub(super) struct FrameData {
    pub(super) instances: Vec<Inst>,
    pub(super) glyphs: Vec<GlyphInst>,
    pub(super) uniforms: Vec<u8>,
    pub(super) clips: Vec<ClipNode>,
}

pub(super) struct FrameUploads<'renderer> {
    textures: &'renderer mut SceneTextures,
    instance_buffer: &'renderer wgpu::Buffer,
    glyph_buffer: Option<&'renderer wgpu::Buffer>,
    uniform_buffer: &'renderer wgpu::Buffer,
    clip_buffer: &'renderer wgpu::Buffer,
    data: FrameData,
}

impl FrameUploads<'_> {
    pub(super) fn bytes(&self) -> u64 {
        self.textures.atlases.upload_bytes
            + self.data.uniforms.len() as u64
            + size_of_val(self.data.instances.as_slice()) as u64
            + size_of_val(self.data.glyphs.as_slice()) as u64
            + size_of_val(self.data.clips.as_slice()) as u64
    }

    pub(super) fn queue(&self, queue: &wgpu::Queue) {
        self.textures.atlases.queue_uploads(queue);
        queue.write_buffer(self.uniform_buffer, 0, &self.data.uniforms);
        if !self.data.instances.is_empty() {
            queue.write_buffer(self.instance_buffer, 0, bytemuck::cast_slice(&self.data.instances));
        }
        if !self.data.glyphs.is_empty() {
            let buffer = self.glyph_buffer.expect("a prepared glyph buffer");
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.data.glyphs));
        }
        if !self.data.clips.is_empty() {
            queue.write_buffer(self.clip_buffer, 0, bytemuck::cast_slice(&self.data.clips));
        }
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
