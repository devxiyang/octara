fn lab_palette(sample: f32, hue: f32, spread: f32) -> vec3f {
    let angle = hue + (sample - 0.50) * spread;
    let wheel = clamp(abs(fract(angle + vec3f(0.0, 0.666667, 0.333333)) * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
    let eased = wheel * wheel * (3.0 - 2.0 * wheel);
    return vec3f(0.24, 0.22, 0.31) + eased * vec3f(0.75, 0.74, 0.68);
}
