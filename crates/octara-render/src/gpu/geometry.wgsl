struct Inst {
    @location(0) rect: vec4f,          // device px: x, y, w, h
    @location(1) fill: vec4f,          // straight RGBA
    @location(2) border_color: vec4f,
    @location(3) border: vec4f,        // top, right, bottom, left
    @location(4) clip: vec4f,          // x, y, w, h
    @location(5) params: vec3u,        // kind, clip head, has fill
    @location(6) uv: vec4f,            // atlas texel origin
    @location(7) grad_color: vec4f,    // a gradient's end color (fill is its start)
    @location(8) grad: vec4f,          // angle (degrees), start, end, has gradient
    @location(9) radius: vec4f,        // top-left, top-right, bottom-right, bottom-left
};

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) rect: vec4f,
    @location(1) @interpolate(flat) fill: vec4f,
    @location(2) @interpolate(flat) border_color: vec4f,
    @location(3) @interpolate(flat) border: vec4f,
    @location(4) @interpolate(flat) clip: vec4f,
    @location(5) @interpolate(flat) params: vec3u,
    @location(6) @interpolate(flat) uv: vec4f,
    @location(7) @interpolate(flat) grad_color: vec4f,
    @location(8) @interpolate(flat) grad: vec4f,
    @location(9) @interpolate(flat) radius: vec4f,
};

const KIND_QUAD: u32 = 0u;
const KIND_SHADOW: u32 = 3u;
const KIND_IMAGE: u32 = 4u;
const KIND_LAYER: u32 = 5u;
const KIND_PATH: u32 = 6u;
const KIND_CANVAS: u32 = 7u;

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    // Two triangles; quads grow by a pixel for the antialiasing ramp.
    var corners = array<vec2f, 6>(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0), vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0));
    let c = corners[vi];
    var grow = 0.0;
    if inst.params.x == KIND_QUAD || inst.params.x == KIND_IMAGE || inst.params.x == KIND_CANVAS {
        grow = 1.0;
    }
    if inst.params.x == KIND_SHADOW {
        grow = 3.0 * inst.border.x + 1.0;
    }
    let p = inst.rect.xy - vec2f(grow) + c * (inst.rect.zw + vec2f(2.0 * grow));
    var out: VsOut;
    let local = (p - globals.origin) / globals.size;
    out.pos = vec4f(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 0.0, 1.0);
    out.rect = inst.rect;
    out.fill = inst.fill;
    out.border_color = inst.border_color;
    out.border = inst.border;
    out.clip = inst.clip;
    out.params = inst.params;
    out.uv = inst.uv;
    out.grad_color = inst.grad_color;
    out.grad = inst.grad;
    out.radius = inst.radius;
    return out;
}

fn clip_coverage(p: vec2f, v: VsOut) -> f32 {
    if any(v.clip.zw <= vec2f(0.0)) {
        return 0.0;
    }
    let head = v.params.y;
    if head == 0u {
        return coverage(sd_round_rect(p, v.clip, vec4f(0.0)));
    }
    if any(p < v.clip.xy - 0.5) || any(p > v.clip.xy + v.clip.zw + 0.5) {
        return 0.0;
    }
    return clip_chain_coverage(p, head);
}

fn quad_uv(p: vec2f, rect: vec4f) -> vec2f {
    return clamp((p - rect.xy) / max(rect.zw, vec2f(0.000001)), vec2f(0.0), vec2f(1.0));
}
