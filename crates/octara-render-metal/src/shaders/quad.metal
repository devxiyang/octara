struct QuadInst {
    float4 rect;
    float4 border;  // top, right, bottom, left
    uint2 radii;    // packed pairs of half floats, clockwise from the top left
    uint fill;
    uint border_color;
    uint gradient_clip;
    uint flags;
    uint2 pad;
};

static_assert(sizeof(QuadInst) == 64, "QuadInst stride");

constant uint FILLED = 1u;
constant uint EXACT_RADII = 2u;

static uint gradient_index(QuadInst quad) {
    return (quad.gradient_clip & 0xffffu) | (quad.flags & 0xffff0000u);
}

static float4 unpack_radii(uint2 packed) {
    // Decode the nonnegative half floats in full precision, including their subnormals.
    uint4 halves = uint4(packed.x, packed.x >> 16u, packed.y, packed.y >> 16u) & 0x7fffu;
    float4 normal = as_type<float4>((halves << 13u) + 0x38000000u);
    float4 subnormal = float4(halves) * (1.0f / 16777216.0f);
    return select(normal, subnormal, halves < 0x0400u);
}

static float4 quad_fill(float2 p, QuadInst quad, uint index, device const GradientInst* gradients) {
    if (index == 0u) {
        return premultiply(unpack_color(quad.fill));
    }
    GradientInst gradient = gradients[index];
    return gradient.params.w > 0.5f ? gradient_at(p, quad.rect, gradient) : premultiply(gradient.start);
}

fragment float4 quad_core_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const QuadInst* quads [[buffer(2)]], device const DrawInst* draws [[buffer(6)]],
    device const GradientInst* gradients [[buffer(7)]], device const float4* spatials [[buffer(8)]]
) {
    DrawInst draw = draws[v.instance];
    QuadInst quad = quads[draw.payload];
    float2 p = v.position.xy + globals.origin - draw_offset(draw, spatials);
    return quad_fill(p, quad, gradient_index(quad), gradients);
}

fragment float4 quad_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const QuadInst* quads [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const GradientInst* gradients [[buffer(7)]],
    device const float4* spatials [[buffer(8)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    QuadInst quad = quads[draw.payload];
    float4 widths = quad.border;
    bool has_border = any(widths > 0.0f);
    bool on_core = in_core(p, draw.core);
    if (on_core && !has_border) {
        return float4(0.0f);
    }

    uint gradient = gradient_index(quad);
    float4 radii = (quad.flags & EXACT_RADII) != 0u ? gradients[gradient].radii : unpack_radii(quad.radii);
    float outer = coverage(sd_round_rect(p, quad.rect, radii));
    uint clip_id = quad.gradient_clip >> 16u;
    if (clip_id != 0xffffu) {
        draw.clip_head = clip_id;
    }
    float clip = clip_coverage(world, draw, clips, spatials);
    float4 fill = float4(0.0f);
    if ((quad.flags & FILLED) != 0u && !on_core) {
        fill = quad_fill(p, quad, gradient, gradients) * (outer * clip);
    }
    if (!has_border) {
        return fill;
    }

    float4 inner = float4(quad.rect.xy + widths.wx, max(quad.rect.zw - widths.wx - widths.yz, float2(0.0f)));
    float inside = 0.0f;
    if (all(inner.zw > 0.0f)) {
        float4 inset = max(widths.xxzz, widths.wyyw);
        inside = coverage(sd_round_rect(p, inner, fit_radii(inner.zw, max(radii - inset, float4(0.0f)))));
    }
    float border_coverage = max(outer - inside, 0.0f) * clip;
    if (on_core && border_coverage == 0.0f) {
        return float4(0.0f);
    }
    float4 border_color = gradient == 0u ? unpack_color(quad.border_color) : gradients[gradient].border_color;
    float4 border = premultiply(border_color) * border_coverage;
    return border + fill * (1.0f - border.a);
}
