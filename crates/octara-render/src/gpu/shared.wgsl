struct Globals {
    size: vec2f,
    origin: vec2f,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct ClipNode {
    rect: vec4f,
    radius: vec4f,
    parent: u32,
};

@group(0) @binding(5) var<storage, read> clip_nodes: array<ClipNode>;

struct DrawInst {
    bounds: vec4f,
    clip: vec4f,
    core: vec4f,
    depth: f32,
    payload: u32,
    clip_head: u32,
    flags: u32,
};

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) inst_id: u32,
};

const FRACTIONAL_CLIP: u32 = 1u;

// Integral rectangular clips are already resolved by the vertex clamp.
override FRAGMENT_CLIPPING: bool = true;

fn draw_vertex(vi: u32, id: u32, rect: vec4f, draw_inst: DrawInst) -> VsOut {
    let corner = vec2f(f32(vi & 1u), f32(vi >> 1u));
    let clip = draw_inst.clip;
    let pad = select(0.0, 0.5, (draw_inst.flags & FRACTIONAL_CLIP) != 0u);
    let lo = max(rect.xy, clip.xy - pad);
    let hi = max(lo, min(rect.xy + rect.zw, clip.xy + clip.zw + pad));
    let p = select(lo + corner * (hi - lo), lo, any(clip.zw <= vec2f(0.0)));
    let local = (p - globals.origin) / globals.size;
    return VsOut(vec4f(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, draw_inst.depth, 1.0), id);
}

fn in_core(p: vec2f, core: vec4f) -> bool {
    // Core bounds are pixel-aligned, so no sample center falls on a shared edge.
    return all(p >= core.xy) && all(p < core.xy + core.zw);
}

fn quad_uv(p: vec2f, rect: vec4f) -> vec2f {
    return clamp((p - rect.xy) / max(rect.zw, vec2f(0.000001)), vec2f(0.0), vec2f(1.0));
}

fn srgb_to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

fn unpack_color(rgba8: u32) -> vec4f {
    let color = unpack4x8unorm(rgba8);
    return vec4f(srgb_to_linear(color.rgb), color.a);
}

fn premultiply(color: vec4f) -> vec4f {
    return vec4f(color.rgb * color.a, color.a);
}

fn corner_distance(d: f32, p: vec2f, radius: f32) -> f32 {
    if all(p < vec2f(radius)) {
        return max(d, length(vec2f(radius) - p) - radius);
    }
    return d;
}

fn sd_round_rect(p: vec2f, r: vec4f, radius: vec4f) -> f32 {
    let near = p - r.xy;
    let far = r.zw - near;
    let edge = -min(near, far);
    var d = length(max(edge, vec2f(0.0))) + min(max(edge.x, edge.y), 0.0);
    d = corner_distance(d, near, radius.x);
    d = corner_distance(d, vec2f(far.x, near.y), radius.y);
    d = corner_distance(d, far, radius.z);
    return corner_distance(d, vec2f(near.x, far.y), radius.w);
}

// Inputs are finite and nonnegative.
fn fit_radii(size: vec2f, r: vec4f) -> vec4f {
    let horizontal = size.x / max(size.x, max(r.x + r.y, r.w + r.z));
    let vertical = size.y / max(size.y, max(r.x + r.w, r.y + r.z));
    return r * min(horizontal, vertical);
}

fn coverage(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

fn clip_chain_coverage(p: vec2f, clip_head: u32) -> f32 {
    var head = clip_head;
    var alpha = 1.0;
    while head != 0u && alpha > 0.0 {
        let node = clip_nodes[head - 1u];
        alpha = min(alpha, coverage(sd_round_rect(p, node.rect, node.radius)));
        head = node.parent;
    }
    return alpha;
}

fn clip_coverage(p: vec2f, draw_inst: DrawInst) -> f32 {
    if !FRAGMENT_CLIPPING {
        return 1.0;
    }
    if draw_inst.clip_head != 0u {
        return clip_chain_coverage(p, draw_inst.clip_head);
    }
    // Integral rectangles have no fractional coverage at device-pixel centers.
    if (draw_inst.flags & FRACTIONAL_CLIP) != 0u {
        return coverage(sd_round_rect(p, draw_inst.clip, vec4f(0.0)));
    }
    return 1.0;
}
