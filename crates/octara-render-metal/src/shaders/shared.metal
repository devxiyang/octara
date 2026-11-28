#include <metal_stdlib>
using namespace metal;

// These storage rows mirror the #[repr(C)] tables in octara-render's frame plan.
struct Globals {
    float2 size;
    float2 origin;
};

struct ClipNode {
    float4 rect;
    float4 radius;
    uint parent;
    uint spatial;
    uint flags;
    uint pad;
};

struct DrawInst {
    float4 bounds;
    float4 clip;
    float4 core;
    float depth;
    uint payload;
    uint clip_head;
    uint flags;
};

struct DrawVertex {
    float4 position [[position]];
    uint instance [[flat]];
};

static_assert(sizeof(Globals) == 16, "Globals layout");
static_assert(sizeof(ClipNode) == 48, "ClipNode stride");
static_assert(sizeof(DrawInst) == 64, "DrawInst stride");

constant uint FRACTIONAL_CLIP = 1u;
constant uint SPATIAL_CLIP = 2u;
constant uint SPATIAL_SHIFT = 2u;
constant uint RECT_PREFIX = 1u;
constant uint CLIP_HORIZONTAL = 2u;
constant uint CLIP_VERTICAL = 4u;

static float2 spatial_offset(uint id, device const float4* spatials) {
    return id == 0u ? float2(0.0f) : spatials[id].xy;
}

static float2 draw_offset(DrawInst draw, device const float4* spatials) {
    return spatial_offset(draw.flags >> SPATIAL_SHIFT, spatials);
}

static float4 world_clip(ClipNode node, device const float4* spatials) {
    float2 pos = node.rect.xy + spatial_offset(node.spatial, spatials);
    if ((node.flags & CLIP_HORIZONTAL) != 0u) {
        return float4(pos.x, node.rect.y, node.rect.zw);
    }
    if ((node.flags & CLIP_VERTICAL) != 0u) {
        return float4(node.rect.x, pos.y, node.rect.zw);
    }
    return float4(pos, node.rect.zw);
}

static float4 intersect_clip(float4 a, float4 b) {
    float2 lo = max(a.xy, b.xy);
    return float4(lo, min(a.xy + a.zw, b.xy + b.zw) - lo);
}

static DrawVertex pull_vertex(
    uint vertex_id, uint id, float4 rect, DrawInst draw, constant Globals& globals,
    device const ClipNode* clips, device const float4* spatials
) {
    float2 corner = float2(vertex_id & 1u, vertex_id >> 1u);
    float2 origin = rect.xy + draw_offset(draw, spatials);
    float4 clip = draw.clip;
    float pad = (draw.flags & FRACTIONAL_CLIP) != 0u ? 0.5f : 0.0f;
    if ((draw.flags & SPATIAL_CLIP) != 0u) {
        ClipNode inner = clips[draw.clip_head - 1u];
        clip = world_clip(inner, spatials);
        uint head = inner.parent;
        while (head != 0u && all(clip.zw > 0.0f)) {
            ClipNode outer = clips[head - 1u];
            clip = intersect_clip(clip, world_clip(outer, spatials));
            head = outer.parent;
        }
        pad = 0.5f;
    }
    float2 lo = max(origin, clip.xy - pad);
    float2 hi = max(lo, min(origin + rect.zw, clip.xy + clip.zw + pad));
    float2 point = any(clip.zw <= 0.0f) ? lo : lo + corner * (hi - lo);
    float2 local = (point - globals.origin) / globals.size;
    return {float4(local.x * 2.0f - 1.0f, 1.0f - local.y * 2.0f, draw.depth, 1.0f), id};
}

vertex DrawVertex draw_vertex(
    uint vertex_id [[vertex_id]], uint id [[instance_id]], constant Globals& globals [[buffer(0)]],
    device const ClipNode* clips [[buffer(5)]], device const DrawInst* draws [[buffer(6)]],
    device const float4* spatials [[buffer(8)]]
) {
    DrawInst draw = draws[id];
    return pull_vertex(vertex_id, id, draw.bounds, draw, globals, clips, spatials);
}

vertex DrawVertex draw_core_vertex(
    uint vertex_id [[vertex_id]], uint id [[instance_id]], constant Globals& globals [[buffer(0)]],
    device const ClipNode* clips [[buffer(5)]], device const DrawInst* draws [[buffer(6)]],
    device const float4* spatials [[buffer(8)]]
) {
    DrawInst draw = draws[id];
    return pull_vertex(vertex_id, id, draw.core, draw, globals, clips, spatials);
}

static bool in_core(float2 p, float4 core) {
    // Core bounds are pixel-aligned, so sample centers fall on neither shared edge.
    return all(p >= core.xy) && all(p < core.xy + core.zw);
}

static float2 quad_uv(float2 p, float4 rect) {
    return clamp((p - rect.xy) / max(rect.zw, float2(0.000001f)), 0.0f, 1.0f);
}

static float3 srgb_to_linear(float3 color) {
    return select(pow((color + 0.055f) / 1.055f, float3(2.4f)), color / 12.92f, color <= 0.04045f);
}

static float4 unpack_color(uint rgba8) {
    float4 color = unpack_unorm4x8_to_float(rgba8);
    return float4(srgb_to_linear(color.rgb), color.a);
}

static float4 premultiply(float4 color) {
    return float4(color.rgb * color.a, color.a);
}

static float corner_distance(float distance, float2 p, float radius) {
    if (all(p < radius)) {
        return max(distance, length(radius - p) - radius);
    }
    return distance;
}

static float sd_round_rect(float2 p, float4 rect, float4 radii) {
    float2 near_edge = p - rect.xy;
    float2 far_edge = rect.zw - near_edge;
    float2 edge = -min(near_edge, far_edge);
    float distance = length(max(edge, float2(0.0f))) + min(max(edge.x, edge.y), 0.0f);
    distance = corner_distance(distance, near_edge, radii.x);
    distance = corner_distance(distance, float2(far_edge.x, near_edge.y), radii.y);
    distance = corner_distance(distance, far_edge, radii.z);
    return corner_distance(distance, float2(near_edge.x, far_edge.y), radii.w);
}

static float4 fit_radii(float2 size, float4 radii) {
    float horizontal = size.x / max(size.x, max(radii.x + radii.y, radii.w + radii.z));
    float vertical = size.y / max(size.y, max(radii.x + radii.w, radii.y + radii.z));
    return radii * min(horizontal, vertical);
}

static float coverage(float distance) {
    return clamp(0.5f - distance, 0.0f, 1.0f);
}

static float clip_chain_coverage(
    float2 p, uint head, bool spatial, device const ClipNode* clips, device const float4* spatials
) {
    float alpha = 1.0f;
    while (head != 0u && alpha > 0.0f) {
        ClipNode node = clips[head - 1u];
        float4 rect = node.rect;
        if (spatial) {
            rect = world_clip(node, spatials);
            // Combine the rectangular ancestors up to the first rounded clip.
            if ((node.flags & RECT_PREFIX) != 0u) {
                head = node.parent;
                while (head != 0u) {
                    ClipNode outer = clips[head - 1u];
                    rect = intersect_clip(rect, world_clip(outer, spatials));
                    head = outer.parent;
                }
                return min(alpha, coverage(sd_round_rect(p, rect, float4(0.0f))));
            }
        }
        alpha = min(alpha, coverage(sd_round_rect(p, rect, node.radius)));
        head = node.parent;
    }
    return alpha;
}

static float clip_coverage(float2 p, DrawInst draw, device const ClipNode* clips, device const float4* spatials) {
    if (draw.clip_head != 0u) {
        return clip_chain_coverage(p, draw.clip_head, (draw.flags & SPATIAL_CLIP) != 0u, clips, spatials);
    }
    // The vertex clamp already resolves integral rectangular clips.
    if ((draw.flags & FRACTIONAL_CLIP) != 0u) {
        return coverage(sd_round_rect(p, draw.clip, float4(0.0f)));
    }
    return 1.0f;
}

struct GradientInst {
    float4 start;
    float4 end;
    float4 border_color;
    float4 params;  // angle in degrees, start and end offsets, presence of a gradient
    float4 radii;   // exact radii when the quad payload cannot represent them
};

static_assert(sizeof(GradientInst) == 80, "GradientInst stride");

static float4 gradient_at(float2 p, float4 rect, GradientInst gradient) {
    float angle = gradient.params.x * (3.14159265358979f / 180.0f);
    float2 direction = float2(sin(angle), -cos(angle));
    float length = dot(rect.zw, abs(direction));
    float t = 0.5f;
    if (length > 0.0f) {
        t += dot(p - (rect.xy + rect.zw * 0.5f), direction) / length;
    }
    float span = gradient.params.z - gradient.params.y;
    float stop = span > 0.0f ? clamp((t - gradient.params.y) / span, 0.0f, 1.0f)
                             : float(t >= gradient.params.y);
    return mix(premultiply(gradient.start), premultiply(gradient.end), stop);
}
