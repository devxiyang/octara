@fragment
fn fs_material(v: VsOut) -> @location(0) vec4f {
    let p = v.pos.xy + globals.origin;
    let clip = coverage(sd_round_rect(p, v.clip, v.params.z));
    let outer = coverage(sd_round_rect(p, v.rect, v.params.x));
    let color = material(quad_uv(p, v.rect), v.uv);
    let alpha = clamp(color.a, 0.0, 1.0) * v.fill.a * outer * clip;
    return vec4f(color.rgb * alpha, alpha);
}
