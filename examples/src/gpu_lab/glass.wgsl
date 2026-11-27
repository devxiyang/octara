struct Lens {
    size: vec2f,
    scale: f32,
    strength: f32,
    scene_offset: vec2f,
    radius: vec4f,
}

@group(0) @binding(0) var<uniform> lens: Lens;
@group(0) @binding(1) var scene: texture_2d<f32>;

@vertex
fn fullscreen_triangle(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let points = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(points[index], 0.0, 1.0);
}

fn read_scene(point: vec2f) -> vec3f {
    let texel = vec2i(floor(point - lens.scene_offset));
    let last_texel = vec2i(textureDimensions(scene)) - vec2i(1);
    return textureLoad(scene, clamp(texel, vec2i(0), last_texel), 0).rgb;
}

fn corner_distance(d: f32, point: vec2f, radius: f32) -> f32 {
    if all(point < vec2f(radius)) {
        return max(d, length(vec2f(radius) - point) - radius);
    }
    return d;
}

@fragment
fn refract(@builtin(position) pixel: vec4f) -> @location(0) vec4f {
    let uv = pixel.xy / lens.size;
    let half_size = 0.5 * lens.size / lens.scale;
    let point = (pixel.xy - 0.5 * lens.size) / lens.scale;
    let near = half_size + point;
    let far = half_size - point;
    let edge = -min(near, far);
    var distance = length(max(edge, vec2f(0.0))) + min(max(edge.x, edge.y), 0.0);
    distance = corner_distance(distance, near, lens.radius.x);
    distance = corner_distance(distance, vec2f(far.x, near.y), lens.radius.y);
    distance = corner_distance(distance, far, lens.radius.z);
    let distance_to_edge = -corner_distance(distance, vec2f(near.x, far.y), lens.radius.w);
    let bevel = 1.0 - smoothstep(0.0, 26.0, distance_to_edge);

    var split: vec3f;
    if lens.strength == 0.0 {
        split = read_scene(pixel.xy);
    } else {
        // Bend rays toward the lens center, with more displacement at the bevel.
        let normal = point / max(half_size, vec2f(1.0));
        let displacement = (0.10 * point + (6.0 + 19.0 * bevel) * normal) * lens.strength * lens.scale;
        let ray = pixel.xy - displacement;
        let fringe = (vec2f(1.3, -0.6) + 3.2 * bevel * normal) * lens.strength * lens.scale;
        let soften = (0.6 + 1.7 * lens.strength) * lens.scale;
        let green = 0.25 * (
            read_scene(ray + vec2f(soften, 0.0)).g + read_scene(ray - vec2f(soften, 0.0)).g +
            read_scene(ray + vec2f(0.0, soften)).g + read_scene(ray - vec2f(0.0, soften)).g
        );
        split = vec3f(read_scene(ray + fringe).r, green, read_scene(ray - fringe).b);
    }

    // A lavender tint keeps the white controls readable while leaving the flow visible.
    let tint = linear_srgb(mix(vec3f(0.11, 0.079, 0.17), vec3f(0.17, 0.12, 0.25), 1.0 - uv.y));
    let glass = mix(split, tint, 0.51 + 0.19 * (1.0 - uv.y));
    let rim = 1.0 - smoothstep(0.35, 1.9, abs(distance_to_edge - 1.45));
    let light = max(0.12, 0.72 - 0.35 * uv.x - 0.56 * uv.y);
    return vec4f(mix(glass, linear_srgb(vec3f(0.79, 0.69, 0.99)), rim * light), 1.0);
}
