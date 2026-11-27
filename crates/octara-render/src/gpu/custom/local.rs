//! Records a local program for composition outside a display list.

use super::super::{FrameInput, FrameTarget, OutputColor, SurfaceRenderer, resolve::Blit, surface::record_surface};
use super::{
    Canvas, CanvasMode, CanvasPaint, CanvasSource, ErrorScopes, PaintProgram, PreparedCanvas, canvas_size, canvas_target, record_canvas,
};
use octara_model::capture::PaintSample;
use octara_model::geometry::Size;
use octara_model::paint::{CustomPaint, DisplayList, Item, Part};
use octara_model::theme::Color;
use rustc_hash::FxHashMap;
use std::sync::Weak;
use std::time::Duration;

pub fn supports(program: &PaintProgram) -> bool {
    program.downcast_ref::<Canvas>().is_some_and(|canvas| matches!(canvas.mode, CanvasMode::Volatile | CanvasMode::Retained))
}

/// Frozen arguments belonging to an accepted layout. Retained programs ignore movement and time.
pub struct Frame {
    paint: CustomPaint,
    scale: f32,
    sampled_at: Option<Duration>,
}

impl Frame {
    pub fn new(mut paint: CustomPaint, scale: f32, sampled_at: Duration) -> Option<Self> {
        let rect = &mut paint.fallback.rect;
        let sampled_at = match paint.program.downcast_ref::<Canvas>()?.mode {
            CanvasMode::Retained => {
                rect.x = 0.0;
                rect.y = 0.0;
                None
            }
            CanvasMode::Volatile => Some(sampled_at),
            _ => return None,
        };
        paint.fallback.radius = rect.corner_radii(paint.fallback.radius);
        Some(Self { paint, scale, sampled_at })
    }

    pub fn extent(&self, max: u32) -> Option<[u32; 2]> {
        let rect = self.paint.fallback.rect;
        canvas_size([0.0, 0.0, rect.w * self.scale, rect.h * self.scale], max)
    }

    fn display_list(&self) -> DisplayList {
        let mut paint = self.paint.clone();
        let rect = &mut paint.fallback.rect;
        rect.x = 0.0;
        rect.y = 0.0;
        DisplayList { size: Size::new(rect.w, rect.h), background: Color::TRANSPARENT, parts: vec![Part::Item(Item::Custom(paint))] }
    }
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.paint.fallback == other.paint.fallback
            && self.scale == other.scale
            && self.sampled_at == other.sampled_at
            && self.paint.input == other.paint.input
            && self.paint.mount == other.paint.mount
            && Weak::ptr_eq(&self.paint.program.downgrade(), &other.paint.program.downgrade())
    }
}

/// The device's reusable programs and one private working texture, independent of browser surfaces.
#[derive(Default)]
pub struct Renderer {
    programs: FxHashMap<usize, PreparedCanvas>,
    output: FxHashMap<wgpu::TextureFormat, Blit>,
    working: Option<wgpu::Texture>,
    surface_renderer: Option<Box<dyn SurfaceRenderer>>,
}

impl Renderer {
    pub fn with_surface_renderer(surface_renderer: Box<dyn SurfaceRenderer>) -> Self {
        Self { surface_renderer: Some(surface_renderer), ..Self::default() }
    }

    pub fn prune(&mut self) {
        self.programs.retain(|_, program| program.owner.strong_count() != 0);
    }

    /// Validates and submits to a private texture. Only the host may make it visible.
    pub async fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        size: [u32; 2],
        revision: u64,
    ) -> Result<wgpu::TextureView, String> {
        if let Some(renderer) = &mut self.surface_renderer {
            let texture = self
                .working
                .clone()
                .filter(|texture| [texture.width(), texture.height()] == size)
                .unwrap_or_else(|| canvas_target(device, size[0], size[1]));
            let list = frame.display_list();
            let sample = frame.sampled_at.map(|sampled_at| PaintSample { revision, sampled_at });
            let input = FrameInput { list: &list, scale: frame.scale, sample };
            let target = FrameTarget { texture: &texture, color: OutputColor::Linear };
            if let Some((commands, _)) =
                record_surface(renderer.as_mut(), device, target, input).await.map_err(|error| error.to_string())?
            {
                let view = texture.create_view(&Default::default());
                queue.submit([commands]);
                self.working = Some(texture);
                return Ok(view);
            }
        }
        let owner = frame.paint.program.downgrade();
        let key = owner.as_ptr().cast::<()>().addr();
        if let std::collections::hash_map::Entry::Vacant(entry) = self.programs.entry(key) {
            let canvas = frame.paint.program.downcast_ref::<Canvas>().expect("a local canvas program");
            let scopes = ErrorScopes::new(device);
            let draw = (canvas.prepare)(device);
            let error = scopes.pop().await;
            entry.insert(PreparedCanvas { owner, draw: error.is_none().then_some(draw), reported_failure: false });
            if let Some(error) = error {
                return Err(format!("preparing a local Canvas: {error}"));
            }
        }
        let draw = self.programs[&key].draw.as_deref().ok_or("the local Canvas program failed preparation")?;
        let paint = CanvasPaint {
            program: frame.paint.program.clone(),
            source: frame.paint.fallback.source,
            size,
            input: frame.paint.input.clone(),
            rect: frame.paint.fallback.rect,
            radius: frame.paint.fallback.radius,
            sampled_at: frame.sampled_at,
        };
        let scopes = ErrorScopes::new(device);
        let ((view, commands), _) = record_canvas(draw, device, &paint, frame.scale, self.working.as_ref(), &mut CanvasSource::Local, None);
        queue.submit([commands]);
        if let Some(error) = scopes.pop().await {
            return Err(format!("drawing a local Canvas: {error}"));
        }
        self.working = Some(view.texture().clone());
        Ok(view)
    }

    pub fn resolve(&mut self, device: &wgpu::Device, source: &wgpu::TextureView, target: &wgpu::TextureView) -> wgpu::CommandBuffer {
        let format = target.texture().format();
        let pipeline = self.output.entry(format).or_insert_with(|| Blit::new(device, format, OutputColor::SrgbPremultiplied));
        let bind = pipeline.bind(device, source);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara local Canvas output") });
        pipeline.encode(&mut encoder, &bind, target, None);
        encoder.finish()
    }
}
