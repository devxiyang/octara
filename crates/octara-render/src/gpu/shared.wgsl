struct Globals {
    size: vec2f,
    origin: vec2f,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct ClipNode {
    rect: vec4f,
    radius: vec4f,
    parent: u32,
};

@group(0) @binding(5) var<storage, read> clip_nodes: array<ClipNode>;

fn corner_distance(d: f32, p: vec2f, radius: f32) -> f32 {
    if all(p < vec2f(radius)) {
        return max(d, length(vec2f(radius) - p) - radius);
    }
    return d;
}

fn sd_round_rect(p: vec2f, r: vec4f, radius: vec4f) -> f32 {
    let near = p - r.xy;
    let far = r.zw - near;
    let edge = -min(near, far);
    var d = length(max(edge, vec2f(0.0))) + min(max(edge.x, edge.y), 0.0);
    d = corner_distance(d, near, radius.x);
    d = corner_distance(d, vec2f(far.x, near.y), radius.y);
    d = corner_distance(d, far, radius.z);
    return corner_distance(d, vec2f(near.x, far.y), radius.w);
}

// Inputs are finite and nonnegative.
fn fit_radii(size: vec2f, r: vec4f) -> vec4f {
    let horizontal = size.x / max(size.x, max(r.x + r.y, r.w + r.z));
    let vertical = size.y / max(size.y, max(r.x + r.w, r.y + r.z));
    return r * min(horizontal, vertical);
}

fn coverage(d: f32) -> f32 {
    return clamp(0.5 - d, 0.0, 1.0);
}

fn clip_chain_coverage(p: vec2f, clip_head: u32) -> f32 {
    var head = clip_head;
    var alpha = 1.0;
    while head != 0u && alpha > 0.0 {
        let node = clip_nodes[head - 1u];
        alpha = min(alpha, coverage(sd_round_rect(p, node.rect, node.radius)));
        head = node.parent;
    }
    return alpha;
}
