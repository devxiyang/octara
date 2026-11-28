//! Native Metal pipeline states shared by the frame and output passes.

use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::*;
use octara_model::capture::PaintFallback;
use octara_render::plan::Material;
use std::collections::HashMap;

const SHARED_SOURCE: &str = include_str!("shaders/shared.metal");

const SOURCE: &str = concat!(
    include_str!("shaders/shared.metal"),
    "\n",
    include_str!("shaders/quad.metal"),
    "\n",
    include_str!("shaders/glyph.metal"),
    "\n",
    include_str!("shaders/shadow.metal"),
    "\n",
    include_str!("shaders/image.metal"),
    "\n",
    include_str!("shaders/resolve.metal")
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pipeline {
    Quad,
    QuadCore,
    Cutout,
    Glyph,
    Shadow,
    Image,
    ImageCore,
    Layer,
    Clear,
    Resolve,
    Crossfade,
}

type State = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
type Library = Retained<ProtocolObject<dyn MTLLibrary>>;
type MaterialKey = (Material, MTLPixelFormat, MTLPixelFormat);

pub(crate) struct Pipelines {
    library: Library,
    states: HashMap<(Pipeline, MTLPixelFormat, MTLPixelFormat), State>,
    materials: HashMap<MaterialKey, Result<State, PaintFallback>>,
    material_libraries: HashMap<Material, Result<Library, PaintFallback>>,
    pub(crate) core: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    pub(crate) blend: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    pub(crate) always: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    pub(crate) sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
}

impl Pipelines {
    pub(crate) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self, String> {
        let library = compile_library(device, &format!("{SOURCE}\n{CROSSFADE_FRAGMENT}"))?;
        let depth = |comparison, writes| {
            let desc = MTLDepthStencilDescriptor::new();
            desc.setDepthCompareFunction(comparison);
            desc.setDepthWriteEnabled(writes);
            device.newDepthStencilStateWithDescriptor(&desc).ok_or("Could not create an Octara Metal depth state")
        };

        let desc = MTLSamplerDescriptor::new();
        desc.setMinFilter(MTLSamplerMinMagFilter::Linear);
        desc.setMagFilter(MTLSamplerMinMagFilter::Linear);
        desc.setMipFilter(MTLSamplerMipFilter::Linear);
        desc.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
        desc.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
        let sampler = device.newSamplerStateWithDescriptor(&desc).ok_or("Could not create the Octara Metal sampler")?;

        Ok(Self {
            library,
            states: HashMap::new(),
            materials: HashMap::new(),
            material_libraries: HashMap::new(),
            core: depth(MTLCompareFunction::Greater, true)?,
            blend: depth(MTLCompareFunction::GreaterEqual, false)?,
            always: depth(MTLCompareFunction::Always, false)?,
            sampler,
        })
    }

    pub(crate) fn get(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        kind: Pipeline,
        color: MTLPixelFormat,
        depth: MTLPixelFormat,
    ) -> Result<&ProtocolObject<dyn MTLRenderPipelineState>, String> {
        let key = (kind, color, depth);
        let state = match self.states.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (vertex, fragment, blend) = kind.functions();
                let state = render_pipeline(device, &self.library, vertex, fragment, color, depth, blend)?;
                entry.insert(state)
            }
        };
        Ok(state)
    }

    pub(crate) fn prepare_material(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        material: Material,
        color: MTLPixelFormat,
        depth: MTLPixelFormat,
    ) -> Result<(), PaintFallback> {
        self.materials
            .entry((material, color, depth))
            .or_insert_with(|| {
                let library = self.material_libraries.entry(material).or_insert_with(|| material_library(device, material));
                let library = library.as_ref().map_err(|reason| *reason)?;
                render_pipeline(device, library, "draw_vertex", "material_fragment", color, depth, Blend::Over)
                    .map_err(|_| PaintFallback::PreparationFailed)
            })
            .as_ref()
            .map(|_| ())
            .map_err(|reason| *reason)
    }

    pub(crate) fn material_state(
        &self,
        material: Material,
        color: MTLPixelFormat,
        depth: MTLPixelFormat,
    ) -> &ProtocolObject<dyn MTLRenderPipelineState> {
        self.materials[&(material, color, depth)].as_ref().expect("prepared author material")
    }

    pub(crate) fn state(
        &self,
        kind: Pipeline,
        color: MTLPixelFormat,
        depth: MTLPixelFormat,
    ) -> &ProtocolObject<dyn MTLRenderPipelineState> {
        &self.states[&(kind, color, depth)]
    }
}

impl Pipeline {
    fn functions(self) -> (&'static str, &'static str, Blend) {
        use Pipeline::*;
        match self {
            Quad => ("draw_vertex", "quad_fragment", Blend::Over),
            QuadCore => ("draw_core_vertex", "quad_core_fragment", Blend::Replace),
            Cutout => ("draw_vertex", "quad_fragment", Blend::Cutout),
            Glyph => ("glyph_vertex", "glyph_fragment", Blend::Over),
            Shadow => ("draw_vertex", "shadow_fragment", Blend::Over),
            Image => ("draw_vertex", "image_fragment", Blend::Over),
            ImageCore => ("draw_core_vertex", "image_core_fragment", Blend::Replace),
            Layer => ("draw_vertex", "layer_fragment", Blend::Over),
            Clear => ("resolve_vertex", "clear_fragment", Blend::Replace),
            Resolve => ("resolve_vertex", "resolve_fragment", Blend::Replace),
            Crossfade => ("resolve_vertex", "crossfade_fragment", Blend::Replace),
        }
    }
}

pub(crate) enum Blend {
    Over,
    Cutout,
    Replace,
}

pub(crate) fn compile_library(
    device: &ProtocolObject<dyn MTLDevice>,
    source: &str,
) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, String> {
    let options = MTLCompileOptions::new();
    options.setMathMode(MTLMathMode::Safe);
    device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|error| format!("Could not compile Octara Metal shaders: {error}"))
}

pub(crate) fn render_pipeline(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    vertex: &str,
    fragment: &str,
    color: MTLPixelFormat,
    depth: MTLPixelFormat,
    blend: Blend,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, String> {
    let desc = MTLRenderPipelineDescriptor::new();
    let function = |name| library.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| format!("Missing Metal shader {name}"));
    let vertex_function = function(vertex)?;
    let fragment_function = function(fragment)?;
    desc.setVertexFunction(Some(&vertex_function));
    desc.setFragmentFunction(Some(&fragment_function));
    desc.setDepthAttachmentPixelFormat(depth);
    // The attachment array contains the fixed Metal color attachment slots.
    let attachment = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setPixelFormat(color);
    if !matches!(blend, Blend::Replace) {
        let source = if matches!(blend, Blend::Cutout) { MTLBlendFactor::Zero } else { MTLBlendFactor::One };
        attachment.setBlendingEnabled(true);
        attachment.setSourceRGBBlendFactor(source);
        attachment.setSourceAlphaBlendFactor(source);
        attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    }
    desc.setLabel(Some(&NSString::from_str(fragment)));
    device.newRenderPipelineStateWithDescriptor_error(&desc).map_err(|error| format!("Could not create Metal pipeline {fragment}: {error}"))
}

fn material_library(device: &ProtocolObject<dyn MTLDevice>, material: Material) -> Result<Library, PaintFallback> {
    let source = match material.msl() {
        Some(source) => std::borrow::Cow::Borrowed(source),
        None => {
            #[cfg(feature = "custom-wgsl")]
            {
                let wgsl = material.wgsl().ok_or(PaintFallback::UnsupportedProgram)?;
                std::borrow::Cow::Owned(super::material::translate(wgsl).map_err(|_| PaintFallback::PreparationFailed)?)
            }
            #[cfg(not(feature = "custom-wgsl"))]
            return Err(PaintFallback::UnsupportedProgram);
        }
    };
    compile_library(device, &format!("{SHARED_SOURCE}\n{source}\n{MATERIAL_FRAGMENT}")).map_err(|_| PaintFallback::PreparationFailed)
}

const MATERIAL_FRAGMENT: &str = include_str!("shaders/material.metal");

const CROSSFADE_FRAGMENT: &str = r#"
struct CrossfadeParams {
    float4 output; // world origin, old/current scale ratio, progress
    float4 old_rect;
    float4 new_rect;
};

static float4 crossfade_sample(texture2d<float> source, sampler filtering, float2 world, float4 rect) {
    const float2 local = world - rect.xy;
    if (any(local < 0.0f) || any(local >= rect.zw)) {
        return float4(0.0f);
    }
    return source.sample(filtering, local / rect.zw, level(0));
}

fragment float4 crossfade_fragment(
    float4 position [[position]], constant CrossfadeParams& params [[buffer(0)]],
    texture2d<float> old_image [[texture(0)]], texture2d<float> new_image [[texture(1)]], sampler filtering [[sampler(3)]]
) {
    const float2 world = position.xy + params.output.xy;
    const float4 old_color = crossfade_sample(old_image, filtering, world * params.output.z, params.old_rect);
    const float4 new_color = crossfade_sample(new_image, filtering, world, params.new_rect);
    return mix(old_color, new_color, params.output.w);
}
"#;
