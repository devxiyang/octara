struct GradientInst {
    start: vec4f,
    end: vec4f,
    border_color: vec4f,
    params: vec4f, // angle (degrees), from and to offsets, presence of a gradient
    radii: vec4f,  // exact radii when the quad payload cannot represent them
};

@group(0) @binding(7) var<storage, read> gradients: array<GradientInst>;

// The gradient line passes through the center of the box and interpolates premultiplied color.
fn gradient_at(p: vec2f, rect: vec4f, gradient: GradientInst) -> vec4f {
    let a = gradient.params.x * (3.14159265358979 / 180.0);
    let d = vec2f(sin(a), -cos(a));
    let len = dot(rect.zw, abs(d));
    var t = 0.5;
    if len > 0.0 {
        t = dot(p - (rect.xy + rect.zw * 0.5), d) / len + 0.5;
    }
    let span = gradient.params.z - gradient.params.y;
    var stop = select(0.0, 1.0, t >= gradient.params.y);
    if span > 0.0 {
        stop = clamp((t - gradient.params.y) / span, 0.0, 1.0);
    }
    return mix(premultiply(gradient.start), premultiply(gradient.end), stop);
}
