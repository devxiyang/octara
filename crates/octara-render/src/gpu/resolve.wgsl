@group(0) @binding(0) var source: texture_2d<f32>;

// Matches OutputColor in Rust. An sRGB attachment encodes its fragment output in hardware.
override output_color: u32 = 0;
override srgb_attachment: bool = false;

fn encode_srgb(rgb: vec3f) -> vec3f {
    let c = clamp(rgb, vec3f(0.0), vec3f(1.0));
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, 12.92 * c, c <= vec3f(0.0031308));
}

fn decode_srgb(rgb: vec3f) -> vec3f {
    return select(pow((rgb + 0.055) / 1.055, vec3f(2.4)), rgb / 12.92, rgb <= vec3f(0.04045));
}

@vertex
fn vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4f {
    let points = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(points[vertex], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pixel: vec4f) -> @location(0) vec4f {
    let color = textureLoad(source, vec2i(pixel.xy), 0);
    if output_color == 0u || (output_color == 1u && srgb_attachment) {
        return color;
    }
    if output_color == 1u {
        return vec4f(encode_srgb(color.rgb), color.a);
    }
    // Alpha is quantized to 8 bits at this boundary. Zero-alpha pixels have zero RGB.
    if color.a < 0.5 / 255.0 {
        return vec4f(0.0);
    }
    let rgb = color.rgb / color.a;
    if output_color == 2u || color.a >= 1.0 {
        return vec4f(select(encode_srgb(rgb), rgb, srgb_attachment), color.a);
    }
    let associated = encode_srgb(rgb) * color.a;
    return vec4f(select(associated, decode_srgb(associated), srgb_attachment), color.a);
}
