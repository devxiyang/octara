struct Transition {
    output_origin: vec2f,
    old_origin: vec2f,
    new_origin: vec2f,
    progress: f32,
}

@group(0) @binding(0) var<uniform> transition: Transition;
@group(0) @binding(1) var old_image: texture_2d<f32>;
@group(0) @binding(2) var new_image: texture_2d<f32>;
@group(0) @binding(3) var output_image: texture_storage_2d<rgba16float, write>;

fn sample_panel(image: texture_2d<f32>, position: vec2f, origin: vec2f) -> vec4f {
    let texel = vec2i(floor(position - origin));
    if any(texel < vec2i(0)) || any(texel >= vec2i(textureDimensions(image))) {
        return vec4f(0.0);
    }
    return textureLoad(image, texel, 0);
}

@compute @workgroup_size(8, 8)
fn crossfade(@builtin(global_invocation_id) pixel: vec3u) {
    if any(pixel.xy >= textureDimensions(output_image)) {
        return;
    }
    let position = transition.output_origin + vec2f(pixel.xy) + 0.5;
    let old_color = sample_panel(old_image, position, transition.old_origin);
    let new_color = sample_panel(new_image, position, transition.new_origin);
    textureStore(output_image, vec2i(pixel.xy), mix(old_color, new_color, transition.progress));
}
