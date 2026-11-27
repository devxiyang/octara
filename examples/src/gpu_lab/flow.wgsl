struct FrameSettings {
    hue: f32,
    phase: f32,
    show_grid: f32,
    spread: f32,
    scale: vec4f,
}

@group(0) @binding(0) var write_field: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(1) var<uniform> settings: FrameSettings;
@group(0) @binding(2) var read_field: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn vector_field(@builtin(global_invocation_id) pixel: vec3u) {
    let size = textureDimensions(write_field);
    if any(pixel.xy >= size) {
        return;
    }

    let uv = (vec2f(pixel.xy) + 0.5) / vec2f(size);
    let x = 2.0 * uv.x - 1.0;
    let y = 1.0 - 2.0 * uv.y;
    let phase = 2.25 * (settings.phase - 0.38);

    // Two warped ribbons intersect without needing mesh tessellation.
    let center = 0.33 * sin(3.78 * x + 0.63 + phase) + 0.24 * x + 0.05;
    let width = 0.17 + 0.085 * cos(3.8 * x + phase) + 0.034 * sin(7.0 * x - 0.68);
    let warp = 0.043 * sin(10.3 * x + 7.5 * y) + 0.018 * cos(19.0 * x - 9.7 * y + phase);
    let ribbon = (y - center + warp) / width;
    let body = exp(-1.33 * ribbon * ribbon);
    let folds = sin(ribbon * 16.0 + 1.55 * sin(6.8 * x + 8.0 * y + phase));
    let threads = pow(0.5 + 0.5 * cos(ribbon * 63.0 + 3.9 * sin(11.6 * x + 6.6 * y)), 9.0);
    let taper = 0.38 + 0.62 * exp(-1.90 * x * x);
    let primary = clamp(body * (0.55 + 0.24 * folds + 0.30 * threads) * taper, 0.0, 1.0);

    let return_center = -0.26 * sin(4.85 * x - 0.82 + 0.70 * phase) - 0.28 * x + 0.12;
    let return_width = 0.16 + 0.021 * cos(4.0 * x + phase);
    let curtain = (y - return_center + 0.59 * warp) / return_width;
    let return_threads = pow(0.5 + 0.5 * cos(9.0 * x + 72.0 * curtain + 1.3 * sin(5.0 * y)), 7.0);
    let secondary = exp(-0.79 * curtain * curtain) * (0.22 + 0.25 * return_threads) * taper;

    let spectrum = clamp(0.53 + 0.40 * x + 0.20 * sin(4.0 * y - 2.7 * x + phase) + 0.09 * ribbon, 0.0, 1.0);
    let glow = max(exp(-0.15 * ribbon * ribbon), 0.61 * exp(-0.17 * curtain * curtain)) * taper;
    textureStore(write_field, vec2i(pixel.xy), vec4f(primary, spectrum, glow, secondary));
}

@vertex
fn fullscreen_triangle(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let points = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(points[index], 0.0, 1.0);
}

@fragment
fn color_resolve(@builtin(position) pixel: vec4f) -> @location(0) vec4f {
    let sample = textureLoad(read_field, vec2i(pixel.xy), 0);
    let uv = pixel.xy / vec2f(textureDimensions(read_field));
    let center = length((uv - vec2f(0.51, 0.48)) * vec2f(0.87, 1.35));
    let base = mix(vec3f(0.047, 0.055, 0.086), vec3f(0.096, 0.100, 0.17), max(0.0, 1.0 - 1.24 * center));
    let color = lab_palette(sample.g, settings.hue, settings.spread);
    let back_color = lab_palette(0.14 + 0.68 * (1.0 - sample.g), settings.hue, settings.spread);
    let highlight = pow(sample.r, 6.0) * vec3f(0.32, 0.34, 0.40);
    let glow = sample.b * sample.b * mix(color, vec3f(0.22, 0.31, 0.77), 0.35);
    let grain = (fract(52.9829189 * fract(dot(floor(pixel.xy), vec2f(0.06711056, 0.00583715)))) - 0.5) * 0.021;
    let light = base + 0.13 * glow + 0.79 * (back_color * sample.a) + 0.96 * (color * sample.r) + highlight + grain;
    return vec4f(linear_srgb(light), 1.0);
}

@fragment
fn reference_grid(@builtin(position) pixel: vec4f) -> @location(0) vec4f {
    let point = pixel.xy / settings.scale.x;
    let lines = abs(fract(point / 25.0 + 0.5) - 0.5) * 25.0;
    let minor = 1.0 - smoothstep(0.1, 0.8, min(lines.x, lines.y));
    let cross = 1.0 - smoothstep(0.23, 1.18, length(lines));
    let field = textureLoad(read_field, vec2i(pixel.xy), 0);
    let visibility = 1.0 - 0.94 * smoothstep(0.025, 0.39, field.r + field.a);
    let alpha = (0.12 * minor + 0.21 * cross) * visibility;
    return vec4f(linear_srgb(vec3f(0.38, 0.42, 0.58)) * alpha, alpha);
}
