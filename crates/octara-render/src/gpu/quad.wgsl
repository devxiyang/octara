@group(0) @binding(6) var<storage, read> draws: array<DrawInst>;

struct QuadInst {
    rect: vec4f,
    border: vec4f, // top, right, bottom, left
    radii: vec2u, // two packed pairs of half floats, clockwise from the top left
    fill: u32,
    border_color: u32,
    gradient_clip: u32,
    flags: u32,
    pad: vec2u,
};

@group(0) @binding(2) var<storage, read> quads: array<QuadInst>;

const FILLED: u32 = 1u;
const EXACT_RADII: u32 = 2u;

fn gradient_index(quad: QuadInst) -> u32 {
    return (quad.gradient_clip & 0xffffu) | (quad.flags & 0xffff0000u);
}

fn unpack_radii(packed: vec2u) -> vec4f {
    // The radii are nonnegative. Decode their half-float bits on adapters that don't support
    // unpack2x16float by rebiasing the exponent and scaling the subnormal mantissas directly.
    let halves = vec4u(packed.x, packed.x >> 16u, packed.y, packed.y >> 16u) & vec4u(0x7fffu);
    let normal = bitcast<vec4f>((halves << vec4u(13u)) + vec4u(0x38000000u));
    let subnormal = vec4f(halves) * (1.0 / 16777216.0);
    return select(normal, subnormal, halves < vec4u(0x0400u));
}

fn quad_fill(p: vec2f, quad: QuadInst, index: u32) -> vec4f {
    if index == 0u {
        return premultiply(unpack_color(quad.fill));
    }
    let gradient = gradients[index];
    if gradient.params.w > 0.5 {
        return gradient_at(p, quad.rect, gradient);
    }
    return premultiply(gradient.start);
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
    let quad = quads[draw_inst.payload];
    return quad_fill(v.pos.xy + globals.origin - draw_offset(draw_inst), quad, gradient_index(quad));
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4f {
    let world = v.pos.xy + globals.origin;
    let draw_inst = draws[v.inst_id];
    let p = world - draw_offset(draw_inst);
    let quad = quads[draw_inst.payload];
    let b = quad.border;
    let has_border = any(b > vec4f(0.0));
    let on_core = in_core(p, draw_inst.core);
    if on_core && !has_border {
        return vec4f(0.0);
    }

    let grad = gradient_index(quad);
    var radii = unpack_radii(quad.radii);
    if (quad.flags & EXACT_RADII) != 0u {
        radii = gradients[grad].radii;
    }
    let outer = coverage(sd_round_rect(p, quad.rect, radii));
    var clipped = draw_inst;
    let clip_id = quad.gradient_clip >> 16u;
    if clip_id != 0xffffu {
        clipped.clip_head = clip_id;
    }
    let clip = clip_coverage(world, clipped);
    var fill = vec4f(0.0);
    if (quad.flags & FILLED) != 0u && !on_core {
        fill = quad_fill(p, quad, grad) * (outer * clip);
    }

    if !has_border {
        return fill;
    }
    let inner = vec4f(quad.rect.xy + b.wx, max(quad.rect.zw - b.wx - b.yz, vec2f(0.0)));
    var inside = 0.0;
    if all(inner.zw > vec2f(0.0)) {
        let inset = max(b.xxzz, b.wyyw);
        inside = coverage(sd_round_rect(p, inner, fit_radii(inner.zw, max(radii - inset, vec4f(0.0)))));
    }
    let border_coverage = max(outer - inside, 0.0) * clip;
    if on_core && border_coverage == 0.0 {
        return vec4f(0.0);
    }
    var border_color: vec4f;
    if grad == 0u {
        border_color = unpack_color(quad.border_color);
    } else {
        border_color = gradients[grad].border_color;
    }
    let border = premultiply(border_color) * border_coverage;
    return border + fill * (1.0 - border.a);
}
