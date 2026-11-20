struct Globals {
    size: vec2f,
    origin: vec2f,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Inst {
    @location(0) rect: vec4f,          // device px: x, y, w, h
    @location(1) fill: vec4f,          // straight RGBA (glyph color for masks)
    @location(2) border_color: vec4f,
    @location(3) border: vec4f,        // top, right, bottom, left
    @location(4) clip: vec4f,          // x, y, w, h
    @location(5) params: vec4f,        // radius, kind, clip radius, has fill
    @location(6) uv: vec4f,            // atlas texel origin
    @location(7) grad_color: vec4f,    // a gradient's end color (fill is its start)
    @location(8) grad: vec4f,          // angle (degrees), start, end, has gradient
};

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) rect: vec4f,
    @location(1) @interpolate(flat) fill: vec4f,
    @location(2) @interpolate(flat) border_color: vec4f,
    @location(3) @interpolate(flat) border: vec4f,
    @location(4) @interpolate(flat) clip: vec4f,
    @location(5) @interpolate(flat) params: vec4f,
    @location(6) @interpolate(flat) uv: vec4f,
    @location(7) @interpolate(flat) grad_color: vec4f,
    @location(8) @interpolate(flat) grad: vec4f,
};

const KIND_QUAD: f32 = 0.0;
const KIND_MASK: f32 = 1.0;
const KIND_SHADOW: f32 = 3.0;
const KIND_IMAGE: f32 = 4.0;
const KIND_LAYER: f32 = 5.0;
const KIND_PATH: f32 = 6.0;
const KIND_CANVAS: f32 = 7.0;

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    // Two triangles; quads grow by a pixel for the antialiasing ramp.
    var corners = array<vec2f, 6>(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0), vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0));
    let c = corners[vi];
    var grow = 0.0;
    if inst.params.y == KIND_QUAD || inst.params.y == KIND_IMAGE || inst.params.y == KIND_CANVAS {
        grow = 1.0;
    }
    if inst.params.y == KIND_SHADOW {
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
    return out;
}

fn sd_round_rect(p: vec2f, r: vec4f, radius: f32) -> f32 {
    let hw = r.z * 0.5;
    let hh = r.w * 0.5;
    let rr = max(min(min(radius, hw), hh), 0.0);
    let qx = abs(p.x - (r.x + hw)) - (hw - rr);
    let qy = abs(p.y - (r.y + hh)) - (hh - rr);
    let ox = max(qx, 0.0);
    let oy = max(qy, 0.0);
    return sqrt(ox * ox + oy * oy) + min(max(qx, qy), 0.0) - rr;
}

fn coverage(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

fn quad_uv(p: vec2f, rect: vec4f) -> vec2f {
    return clamp((p - rect.xy) / max(rect.zw, vec2f(0.000001)), vec2f(0.0), vec2f(1.0));
}
