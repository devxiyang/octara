@group(0) @binding(6) var<storage, read> draws: array<DrawInst>;

struct ShadowInst {
    rect: vec4f,
    radius: vec4f,
    color: vec4f,
    sigma: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

@group(0) @binding(2) var<storage, read> shadows: array<ShadowInst>;

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265358979) * sigma);
}

fn erf(v: f32) -> f32 {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

fn shadow_inset(radius: f32, dist: f32) -> f32 {
    let delta = max(radius - dist, 0.0);
    return radius - sqrt(max(radius * radius - delta * delta, 0.0));
}

fn blur_along_x(x: f32, y: f32, sigma: f32, radius: vec4f, half: vec2f) -> f32 {
    let top = half.y + y;
    let bottom = half.y - y;
    let left = -half.x + shadow_inset(radius.x, top) + shadow_inset(radius.w, bottom);
    let right = half.x - shadow_inset(radius.y, top) - shadow_inset(radius.z, bottom);
    let k = sqrt(0.5) / sigma;
    return (0.5 + 0.5 * erf((x - left) * k)) - (0.5 + 0.5 * erf((x - right) * k));
}

// Same separable Gaussian and four row samples as shape::shadow_alpha.
fn shadow_alpha(p: vec2f, r: vec4f, radius: vec4f, sigma: f32) -> f32 {
    if sigma <= 0.0 {
        return coverage(sd_round_rect(p, r, radius));
    }
    let half = r.zw * 0.5;
    let c = p - (r.xy + half);
    let low = c.y - half.y;
    let high = c.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var alpha = 0.0;
    for (var i = 0; i < 4; i += 1) {
        alpha += blur_along_x(c.x, c.y - y, sigma, radius, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return clamp(alpha, 0.0, 1.0);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> VsOut {
    let draw_inst = draws[id];
    return draw_vertex(vi, id, draw_inst.bounds, draw_inst);
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4f {
    let world = v.pos.xy + globals.origin;
    let draw_inst = draws[v.inst_id];
    let p = world - draw_offset(draw_inst);
    let shadow = shadows[draw_inst.payload];
    let alpha = shadow_alpha(p, shadow.rect, shadow.radius, shadow.sigma) * clip_coverage(world, draw_inst);
    return premultiply(shadow.color) * alpha;
}
