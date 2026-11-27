@group(0) @binding(6) var<storage, read> draws: array<DrawInst>;

struct MaterialInst {
    rect: vec4f,
    radius: vec4f,
    params: vec4f,
    opacity: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

@group(0) @binding(2) var<storage, read> materials: array<MaterialInst>;

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> VsOut {
    let draw_inst = draws[id];
    return draw_vertex(vi, id, draw_inst.bounds, draw_inst);
}

@fragment
fn fs_material(v: VsOut) -> @location(0) vec4f {
    let world = v.pos.xy + globals.origin;
    let draw_inst = draws[v.inst_id];
    let p = world - draw_offset(draw_inst);
    let paint = materials[draw_inst.payload];
    let outer = coverage(sd_round_rect(p, paint.rect, paint.radius));
    let color = material(quad_uv(p, paint.rect), paint.params);
    let alpha = clamp(color.a, 0.0, 1.0) * paint.opacity * outer * clip_coverage(world, draw_inst);
    return vec4f(color.rgb * alpha, alpha);
}
