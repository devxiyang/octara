struct Placement {
    output: vec4f,
    old: vec4f,
    incoming: vec4f,
}

@group(0) @binding(0) var<uniform> placement: Placement;
@group(0) @binding(1) var old_image: texture_2d<f32>;
@group(0) @binding(2) var new_image: texture_2d<f32>;
@group(0) @binding(3) var output: texture_storage_2d<rgba16float, write>;

fn sample(image: texture_2d<f32>, bounds: vec4f, point: vec2f) -> vec4f {
    let px = vec2i(floor(point * bounds.z - bounds.xy));
    if any(px < vec2i(0)) || any(px >= vec2i(textureDimensions(image))) {
        return vec4f(0.0);
    }
    return textureLoad(image, px, 0);
}

@compute @workgroup_size(8, 8)
fn crossfade(@builtin(global_invocation_id) pixel: vec3u) {
    if any(pixel.xy >= textureDimensions(output)) {
        return;
    }
    let point = placement.output.xy + (vec2f(pixel.xy) + 0.5) / placement.output.z;
    textureStore(output, vec2i(pixel.xy), mix(
        sample(old_image, placement.old, point),
        sample(new_image, placement.incoming, point),
        placement.output.w,
    ));
}
