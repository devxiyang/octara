fn material(uv: vec2f, params: vec4f) -> vec4f {
    let bend = uv.y + uv.x * 0.93 + 0.20 * sin(6.5 * uv.x + 2.7 * uv.y + params.y);
    let t = clamp(0.05 + 0.88 * uv.x + 0.17 * sin(5.4 * bend), 0.0, 1.0);
    let fine = pow(0.5 + 0.5 * cos(52.0 * bend + 2.5 * uv.x), 6.0);
    let fold = 0.5 + 0.5 * sin(15.0 * bend - 2.4);
    let center = bend - 0.82;
    let light = 0.20 + 0.64 * exp(-9.0 * center * center) + 0.13 * fold + 0.16 * fine;
    let color = lab_palette(t, params.x, params.w) * light + vec3f(0.10, 0.11, 0.17) * fine * fold;
    return vec4f(color, 0.97);
}
