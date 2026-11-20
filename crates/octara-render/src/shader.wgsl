// The instanced shader for standard UI draws and canvas composites. The shape formulas also
// apply to the CPU reference renderer.

@group(0) @binding(1) var mask_atlas: texture_2d<f32>;
@group(0) @binding(2) var color_atlas: texture_2d<f32>;
@group(0) @binding(3) var image_atlas: texture_2d<f32>;
@group(0) @binding(4) var layer: texture_2d<f32>;

const PI: f32 = 3.14159265358979;

fn image_texel(v: VsOut, x: f32, y: f32) -> vec4f {
    let c = vec2f(clamp(x, 0.0, v.uv.z - 1.0), clamp(y, 0.0, v.uv.w - 1.0));
    let t = textureLoad(image_atlas, vec2i(v.uv.xy + c), 0);
    return vec4f(t.rgb * t.a, t.a);
}

// The formula of `shape::image_color`: bilinear between premultiplied texels, clamped at the
// image's edges, antialiased where it ends, clipped to its rounded box.
fn image_color(p: vec2f, v: VsOut) -> vec4f {
    let edge = coverage(sd_round_rect(p, v.rect, 0.0));
    let inside = coverage(sd_round_rect(p, v.border, v.params.x));
    let cov = edge * inside;
    if cov <= 0.0 || v.rect.z <= 0.0 || v.rect.w <= 0.0 {
        return vec4f(0.0);
    }
    let u = (p.x - v.rect.x) / v.rect.z * v.uv.z - 0.5;
    let w = (p.y - v.rect.y) / v.rect.w * v.uv.w - 0.5;
    let fu = floor(u);
    let fw = floor(w);
    let tx = u - fu;
    let ty = w - fw;
    let a = image_texel(v, fu, fw);
    let b = image_texel(v, fu + 1.0, fw);
    let c = image_texel(v, fu, fw + 1.0);
    let d = image_texel(v, fu + 1.0, fw + 1.0);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return (top + (bottom - top) * ty) * cov;
}

// Canvas texels are already premultiplied. Interpolate when a canvas has a fractional position
// or its logical extent rounds up to a whole pixel in the render attachment.
fn canvas_color(p: vec2f, v: VsOut) -> vec4f {
    let size = textureDimensions(layer);
    let texel = clamp(quad_uv(p, v.rect) * vec2f(size) - 0.5, vec2f(0.0), vec2f(size) - 1.0);
    let at = vec2i(floor(texel));
    let hi = min(at + 1, vec2i(size) - 1);
    let weight = fract(texel);
    let top = mix(textureLoad(layer, at, 0), textureLoad(layer, vec2i(hi.x, at.y), 0), weight.x);
    let bottom = mix(textureLoad(layer, vec2i(at.x, hi.y), 0), textureLoad(layer, hi, 0), weight.x);
    let outer = coverage(sd_round_rect(p, v.rect, v.params.x));
    return mix(top, bottom, weight.y) * outer;
}

// The formula of `octara_primitives::paint::Gradient::at`: the CSS gradient line through the box's center,
// colors mixed premultiplied. Returns a straight color.
fn gradient_at(p: vec2f, r: vec4f, g: vec4f, c0: vec4f, c1: vec4f) -> vec4f {
    let a = g.x * PI / 180.0;
    let d = vec2f(sin(a), -cos(a));
    let len = abs(r.z * d.x) + abs(r.w * d.y);
    var t = 0.5;
    if len > 0.0 {
        t = dot(p - (r.xy + r.zw * 0.5), d) / len + 0.5;
    }
    var k = 0.0;
    if g.z > g.y {
        k = clamp((t - g.y) / (g.z - g.y), 0.0, 1.0);
    } else if t >= g.y {
        k = 1.0;
    }
    let a0 = vec4f(c0.rgb * c0.a, c0.a);
    let a1 = vec4f(c1.rgb * c1.a, c1.a);
    let m = a0 + (a1 - a0) * k;
    if m.a <= 0.0 {
        return vec4f(0.0);
    }
    return vec4f(m.rgb / m.a, m.a);
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * PI) * sigma);
}

fn erf(v: f32) -> f32 {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2f) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let k = sqrt(0.5) / sigma;
    return (0.5 + 0.5 * erf((x + curved) * k)) - (0.5 + 0.5 * erf((x - curved) * k));
}

// The formula of `shape::shadow_alpha`.
fn shadow_alpha(p: vec2f, r: vec4f, radius: f32, sigma: f32) -> f32 {
    if sigma <= 0.0 {
        return coverage(sd_round_rect(p, r, radius));
    }
    let half = r.zw * 0.5;
    let c = p - (r.xy + half);
    let corner = max(min(min(radius, half.x), half.y), 0.0);
    let low = c.y - half.y;
    let high = c.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var alpha = 0.0;
    for (var i = 0; i < 4; i += 1) {
        alpha += blur_along_x(c.x, c.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return clamp(alpha, 0.0, 1.0);
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4f {
    let p = v.pos.xy + globals.origin; // pixel center in window coordinates
    let clip = coverage(sd_round_rect(p, v.clip, v.params.z));
    let kind = v.params.y;
    if kind == KIND_SHADOW {
        let a = v.fill.a * shadow_alpha(p, v.rect, v.params.x, v.border.x) * clip;
        return vec4f(v.fill.rgb * a, a);
    }
    if kind == KIND_LAYER {
        // A layer, pixel for pixel, at its opacity (premultiplied already).
        return textureLoad(layer, vec2i(p - v.rect.xy), 0) * v.fill.a;
    }
    if kind == KIND_CANVAS {
        return canvas_color(p, v) * v.fill.a * clip;
    }
    if kind == KIND_IMAGE {
        return image_color(p, v) * v.fill.a * clip;
    }
    if kind == KIND_QUAD {
        let outer = coverage(sd_round_rect(p, v.rect, v.params.x));
        var fill = v.fill;
        if v.grad.w > 0.5 {
            fill = gradient_at(p, v.rect, v.grad, v.fill, v.grad_color);
        }
        var fa = 0.0;
        if v.params.w > 0.5 {
            fa = fill.a * outer * clip;
        }
        // Border: outer minus the inner rounded rect.
        let b = v.border;
        let widest = max(max(b.x, b.y), max(b.z, b.w));
        var ba = 0.0;
        if widest > 0.0 {
            let inner = vec4f(v.rect.x + b.w, v.rect.y + b.x, max(v.rect.z - b.w - b.y, 0.0), max(v.rect.w - b.x - b.z, 0.0));
            var inside = 0.0;
            if inner.z > 0.0 && inner.w > 0.0 {
                inside = coverage(sd_round_rect(p, inner, max(v.params.x - widest, 0.0)));
            }
            ba = v.border_color.a * max(outer - inside, 0.0) * clip;
        }
        // The border over the fill, premultiplied.
        let rgb = v.border_color.rgb * ba + fill.rgb * fa * (1.0 - ba);
        return vec4f(rgb, ba + fa * (1.0 - ba));
    }
    let texel = vec2i(v.uv.xy + floor(p - v.rect.xy));
    if kind == KIND_MASK {
        let a = textureLoad(mask_atlas, texel, 0).r * v.fill.a * clip;
        return vec4f(v.fill.rgb * a, a);
    }
    if kind == KIND_PATH {
        // A path's coverage, filled with its color or its gradient (over the box in `border`).
        var fill = v.fill;
        if v.grad.w > 0.5 {
            fill = gradient_at(p, v.border, v.grad, v.fill, v.grad_color);
        }
        let a = textureLoad(mask_atlas, texel, 0).r * fill.a * clip;
        return vec4f(fill.rgb * a, a);
    }
    let t = textureLoad(color_atlas, texel, 0);
    let a = t.a * v.fill.a * clip;
    return vec4f(t.rgb * a, a);
}
