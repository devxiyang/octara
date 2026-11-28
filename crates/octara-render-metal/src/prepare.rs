//! Frame-plan resource resolution and immutable shader instance buffers.

use super::atlas::{Atlases, PreparedSamples, SampleBinding};
use super::external;
use super::pipeline::Pipelines;
use super::target::WORKING_FORMAT;
use super::transition::TransitionPlan;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::*;
use octara_model::{
    capture::{PaintOutcome, PaintReceipt},
    geometry::Rect,
    image::ExternalTextureHandle,
    style::Mat4,
};
use octara_render::plan::{
    ClipNode, Draw, DrawSample, DrawSegment, FramePlan, GlyphInst, Instances, Paint, Pass, PassBounds, PlannedBatch, ResourcePlan, Shape,
    bytemuck, schedule_pass,
};

pub(crate) type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;

pub(crate) struct PreparedFrame {
    pub(crate) passes: Vec<PreparedPass>,
    pub(crate) batches: Vec<PlannedBatch>,
    pub(crate) buffers: DrawBuffers,
    pub(crate) external: Vec<Option<ExternalTextureHandle>>,
    pub(crate) receipts: Vec<PaintReceipt>,
    pub(crate) skipped_transitions: Vec<(std::sync::Weak<()>, u64)>,
    pub(crate) atlas_bytes: u64,
    pub(crate) resolve_output: bool,
    pub(crate) supports_damage: bool,
}

pub(crate) struct PreparedPass {
    pub(crate) bounds: PassBounds,
    pub(crate) segments: Vec<DrawSegment>,
    pub(crate) depth: MTLPixelFormat,
    pub(crate) globals: Option<Buffer>,
    pub(crate) transform: Option<Mat4>,
    pub(crate) transition: Option<TransitionPlan>,
}

pub(crate) struct DrawBuffers {
    pub(crate) draws: Option<Buffer>,
    pub(crate) quads: Option<Buffer>,
    pub(crate) shadows: Option<Buffer>,
    pub(crate) images: Option<Buffer>,
    pub(crate) glyphs: Option<Buffer>,
    pub(crate) glyph_draws: Option<Buffer>,
    pub(crate) glyph_clips: Option<Buffer>,
    pub(crate) gradients: Option<Buffer>,
    pub(crate) materials: Option<Buffer>,
    pub(crate) clips: Option<Buffer>,
}

impl PreparedFrame {
    pub(crate) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        atlases: &mut Atlases,
        plan: FramePlan,
        root_format: MTLPixelFormat,
        pipelines: &mut Pipelines,
        transitions: Vec<Option<TransitionPlan>>,
    ) -> Result<(Self, u64, u64), String> {
        let FramePlan { mut passes, mut requests, clips, mut resources, spatial, bounds, resolve_output, .. } = plan;
        let PreparedSamples { samples, atlas_bytes, upload_bytes } = atlases.prepare(device, command, &resources)?;
        let external = std::mem::take(&mut resources.external_textures)
            .into_iter()
            .map(|handle| external::compatible(&handle, device).then_some(handle))
            .collect::<Vec<_>>();
        for pass in &mut passes {
            resolve(pass, &resources, &samples, &external, &requests);
        }
        let mut instances = Instances::default();
        let mut batches = Vec::new();
        let mut instance_bytes = 0;
        let mut transitions = transitions.into_iter();
        let passes = passes
            .iter_mut()
            .zip(bounds)
            .enumerate()
            .map(|(index, (pass, bounds))| {
                let depth = depth_format(device, pass);
                let format = if index == 0 && !resolve_output { root_format } else { WORKING_FORMAT };
                for batch in &mut pass.batches {
                    let Draw::RequestedMaterial(material) = batch.draw else { continue };
                    let request = &resources.materials[material];
                    let receipt = &mut requests.receipts[request.receipt];
                    match pipelines.prepare_material(device, request.material, format, depth) {
                        Ok(()) => {
                            batch.draw = Draw::Material(request.material);
                            receipt.outcome = PaintOutcome::Rendered;
                        }
                        Err(reason) => {
                            batch.draw = Draw::Quad;
                            pass.paints[batch.instances.clone()].fill(request.fallback);
                            receipt.outcome = PaintOutcome::Fallback { reason };
                        }
                    }
                }
                let [x, y, w, h] = bounds.rect();
                let segments = schedule_pass(pass, Rect::new(x, y, w, h), &clips, &mut instances, &mut batches);
                let globals = if segments.iter().any(|segment| !segment.cores.is_empty() || !segment.blended.is_empty()) {
                    let globals = [w, h, x, y];
                    let bytes = bytemuck::bytes_of(&globals);
                    instance_bytes += bytes.len() as u64;
                    Some(buffer(device, bytes)?)
                } else {
                    None
                };
                let transform = pass.projection.map(|index| *spatial.projection(index));
                Ok(PreparedPass { bounds, segments, depth, globals, transform, transition: transitions.next().flatten() })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let supports_damage = passes.len() == 1
            && external.is_empty()
            && requests.receipts.is_empty()
            && batches.iter().all(|batch| matches!(batch.draw, Draw::Quad | Draw::Shadow | Draw::Image(_) | Draw::Glyph(_)));
        let buffers = DrawBuffers::new(device, &instances, &clips.into_nodes(), &mut instance_bytes)?;
        Ok((
            Self {
                passes,
                batches,
                buffers,
                external,
                receipts: requests.receipts,
                skipped_transitions: requests.skipped_transitions,
                atlas_bytes,
                resolve_output,
                supports_damage,
            },
            instance_bytes,
            upload_bytes,
        ))
    }
}

fn depth_format(device: &ProtocolObject<dyn MTLDevice>, pass: &Pass) -> MTLPixelFormat {
    if pass.spatial || pass.paints.is_empty() {
        return MTLPixelFormat::Invalid;
    }
    // A 16-bit attachment must distinguish all ranks, including the zero clear value.
    if pass.paints.len() < u16::MAX as usize && !cfg!(target_abi = "sim") && device.supportsFamily(MTLGPUFamily::Apple1) {
        MTLPixelFormat::Depth16Unorm
    } else {
        MTLPixelFormat::Depth32Float
    }
}

fn resolve(
    pass: &mut Pass,
    resources: &ResourcePlan,
    samples: &[SampleBinding],
    external: &[Option<ExternalTextureHandle>],
    requests: &octara_render::plan::PaintRequests,
) {
    let batches = std::mem::take(&mut pass.batches);
    let paints = std::mem::take(&mut pass.paints);
    for batch in batches {
        for &paint in &paints[batch.instances] {
            match batch.draw {
                Draw::Sample(index) => match &samples[index] {
                    SampleBinding::Missing => {}
                    SampleBinding::Glyph(region) => {
                        let DrawSample::Glyph(glyph) = &resources.samples[index] else { unreachable!("glyph sample") };
                        let shape = Shape::Glyph(Some(GlyphInst::new(glyph.pos, *region, glyph.color, glyph.image.color)));
                        pass.push(Paint { shape, ..paint }, Draw::Glyph(region.array));
                    }
                    SampleBinding::Image(region) => {
                        let Shape::Image { bounds, opacity, .. } = paint.shape else { unreachable!("image sample") };
                        let shape = Shape::Image { bounds, opacity, region: Some(*region) };
                        pass.push(Paint { shape, ..paint }, Draw::Image(region.array));
                    }
                    SampleBinding::Path(tiles) => {
                        let Shape::Path { bounds, color, gradient, .. } = paint.shape else { unreachable!("path sample") };
                        for tile in tiles {
                            let [dx, dy] = tile.offset;
                            let [w, h] = tile.region.size;
                            let paint = Paint {
                                rect: [paint.rect[0] + dx as f32, paint.rect[1] + dy as f32, w as f32, h as f32],
                                shape: Shape::Path { bounds, color, gradient, region: Some(tile.region) },
                                ..paint
                            };
                            pass.push(paint, Draw::Image(tile.region.array));
                        }
                    }
                },
                Draw::Canvas(index) => pass.push(requests.canvases[index].fallback, Draw::Quad),
                Draw::Backdrop(index) => pass.push(requests.backdrops[index].fallback, Draw::Quad),
                Draw::External(index) if external[index].is_none() => {}
                draw => pass.push(paint, draw),
            }
        }
    }
}

impl DrawBuffers {
    fn new(device: &ProtocolObject<dyn MTLDevice>, instances: &Instances, clips: &[ClipNode], size: &mut u64) -> Result<Self, String> {
        let mut upload = |bytes: &[u8]| {
            *size += bytes.len() as u64;
            if bytes.is_empty() { Ok(None) } else { buffer(device, bytes).map(Some) }
        };
        Ok(Self {
            draws: upload(bytemuck::cast_slice(&instances.draws))?,
            quads: upload(bytemuck::cast_slice(&instances.quads))?,
            shadows: upload(bytemuck::cast_slice(&instances.shadows))?,
            images: upload(bytemuck::cast_slice(&instances.images))?,
            glyphs: upload(bytemuck::cast_slice(&instances.glyphs))?,
            glyph_draws: upload(bytemuck::cast_slice(&instances.glyph_draws))?,
            glyph_clips: upload(bytemuck::cast_slice(&instances.glyph_clips))?,
            gradients: upload(bytemuck::cast_slice(&instances.gradients))?,
            materials: upload(bytemuck::cast_slice(&instances.materials))?,
            clips: upload(bytemuck::cast_slice(clips))?,
        })
    }
}

pub(crate) fn buffer(device: &ProtocolObject<dyn MTLDevice>, bytes: &[u8]) -> Result<Buffer, String> {
    let ptr = std::ptr::NonNull::from(bytes).cast();
    // Metal copies this initialized slice before newBufferWithBytes returns.
    unsafe { device.newBufferWithBytes_length_options(ptr, bytes.len(), MTLResourceOptions::StorageModeShared) }
        .ok_or_else(|| format!("Could not allocate an Octara Metal buffer of {} bytes", bytes.len()))
}
