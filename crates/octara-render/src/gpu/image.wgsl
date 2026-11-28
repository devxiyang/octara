@group(0) @binding(6) var<storage, read> draws: array<DrawInst>;

struct ImageInst {
    rect: vec4f,
    image_bounds: vec4f,
    radius: vec4f,
    uv: vec4f, // atlas origin and size, in base-level texels
    fill: vec4f,
    layer: u32,
    max_lod: f32,
    kind: u32,
    gradient: u32,
};

@group(0) @binding(1) var atlas: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read> images: array<ImageInst>;
@group(0) @binding(3) var image_sampler: sampler;
@group(0) @binding(4) var layer: texture_2d<f32>;

const IMAGE: u32 = 0u;
const PATH: u32 = 1u;
const LAYER: u32 = 2u;
const CANVAS: u32 = 3u;
const EXTERNAL: u32 = 4u;

fn image_sample(p: vec2f, image: ImageInst) -> vec4f {
    let footprint = image.uv.zw / image.rect.zw;
    let lod = clamp(log2(max(footprint.x, footprint.y)), 0.0, image.max_lod);
    let texel = image.uv.xy + quad_uv(p, image.rect) * image.uv.zw;
    let uv = texel / vec2f(textureDimensions(atlas));
    // The uploaded texels are premultiplied, with an edge-replicating gutter at each mip level.
    return textureSampleLevel(atlas, image_sampler, uv, i32(image.layer), lod);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> VsOut {
    let draw_inst = draws[id];
    return draw_vertex(vi, id, draw_inst.bounds, draw_inst);
}

@vertex
fn vs_core(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> VsOut {
    let draw_inst = draws[id];
    return draw_vertex(vi, id, draw_inst.core, draw_inst);
}

@fragment
fn fs_core(v: VsOut) -> @location(0) vec4f {
    let draw_inst = draws[v.inst_id];
    return image_sample(v.pos.xy + globals.origin - draw_offset(draw_inst), images[draw_inst.payload]);
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4f {
    let world = v.pos.xy + globals.origin;
    let draw_inst = draws[v.inst_id];
    let p = world - draw_offset(draw_inst);
    let image = images[draw_inst.payload];
    let clip = clip_coverage(world, draw_inst);

    switch image.kind {
        case IMAGE: {
            if in_core(p, draw_inst.core) {
                return vec4f(0.0);
            }
            // Independently antialias the placed image and its rounded bounding box.
            let edge = coverage(sd_round_rect(p, image.rect, vec4f(0.0)));
            let inside = coverage(sd_round_rect(p, image.image_bounds, image.radius));
            let alpha = edge * inside * image.fill.a * clip;
            if alpha <= 0.0 {
                return vec4f(0.0);
            }
            return image_sample(p, image) * alpha;
        }
        case PATH: {
            var fill = premultiply(image.fill);
            if image.gradient != 0u {
                fill = gradient_at(p, image.image_bounds, gradients[image.gradient]);
            }
            let texel = vec2i(image.uv.xy + floor(p - image.rect.xy));
            let mask = textureLoad(atlas, texel, i32(image.layer), 0).r;
            return fill * (mask * clip);
        }
        case LAYER: {
            // An isolated layer already contains premultiplied pixels at the target resolution.
            return textureLoad(layer, vec2i(p - image.rect.xy), 0) * (image.fill.a * clip);
        }
        case CANVAS: {
            let outer = coverage(sd_round_rect(p, image.rect, image.radius));
            let color = textureSampleLevel(layer, image_sampler, quad_uv(p, image.rect), 0.0);
            return color * (outer * image.fill.a * clip);
        }
        case EXTERNAL: {
            let edge = coverage(sd_round_rect(p, image.rect, vec4f(0.0)));
            let inside = coverage(sd_round_rect(p, image.image_bounds, image.radius));
            let alpha = edge * inside * image.fill.a * clip;
            if alpha <= 0.0 {
                return vec4f(0.0);
            }
            let color = textureSampleLevel(layer, image_sampler, quad_uv(p, image.rect), 0.0);
            return color * alpha;
        }
        default: { return vec4f(0.0); }
    }
}

struct LayerProjection {
    matrix: mat4x4f,
    source: vec4f,
};

@group(0) @binding(9) var<uniform> projection: LayerProjection;

const PROJECTIVE_NEAR_W: f32 = 0.00001;

struct ProjectedLayerVertex {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) inst_id: u32,
    @location(1) source: vec2f,
};

@vertex
fn vs_projected_layer(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> ProjectedLayerVertex {
    let corner = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let source = corner * projection.source.zw;
    let pos = projection.matrix * vec4f(projection.source.xy + source, 0.0, 1.0);
    let local = (pos.xy - globals.origin * pos.w) / globals.size;
    // Keep the painter depth constant after the perspective divide.
    let clip = vec4f(2.0 * local.x - pos.w, pos.w - 2.0 * local.y, draws[id].depth * pos.w, pos.w);
    return ProjectedLayerVertex(clip, id, source);
}

@fragment
fn fs_projected_layer(v: ProjectedLayerVertex) -> @location(0) vec4f {
    // The rasterizer clips the eye plane. Apply the planner's positive-W guard as well.
    if v.pos.w * PROJECTIVE_NEAR_W > 1.0 {
        discard;
    }
    let world = v.pos.xy + globals.origin;
    let draw_inst = draws[v.inst_id];
    // A projected quad cannot clamp its vertices to a clip in the parent's pixel space.
    if (draw_inst.flags & (FRACTIONAL_CLIP | SPATIAL_CLIP)) == 0u &&
        (any(world < draw_inst.clip.xy) || any(world >= draw_inst.clip.xy + draw_inst.clip.zw)) {
        discard;
    }
    let clip = clip_coverage(world, draw_inst);
    let image = images[draw_inst.payload];
    // The sample footprint stops at the source raster even when the pooled texture is larger.
    let pixel = clamp(v.source, vec2f(0.5), projection.source.zw - vec2f(0.5));
    let color = textureSampleLevel(layer, image_sampler, pixel / vec2f(textureDimensions(layer)), 0.0);
    return color * (image.fill.a * clip);
}
