@group(0) @binding(0) var<uniform> background: vec4f;

@vertex
fn vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4f {
    let points = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(points[vertex], 0.0, 1.0);
}

@fragment
fn fs() -> @location(0) vec4f {
    return background;
}
