struct ShadowInst {
    float4 rect;
    float4 radii;
    float4 color;
    float sigma;
    float pad0;
    float pad1;
    float pad2;
};

static_assert(sizeof(ShadowInst) == 64, "ShadowInst stride");

static float gaussian(float x, float sigma) {
    return exp(-(x * x) / (2.0f * sigma * sigma)) / (sqrt(2.0f * 3.14159265358979f) * sigma);
}

static float shadow_erf(float value) {
    float s = sign(value);
    float a = abs(value);
    float r1 = 1.0f + (0.278393f + (0.230389f + (0.000972f + 0.078108f * a) * a) * a) * a;
    float r2 = r1 * r1;
    return s - s / (r2 * r2);
}

static float shadow_inset(float radius, float distance) {
    float delta = max(radius - distance, 0.0f);
    return radius - sqrt(max(radius * radius - delta * delta, 0.0f));
}

static float blur_along_x(float x, float y, float sigma, float4 radii, float2 half_size) {
    float top = half_size.y + y;
    float bottom = half_size.y - y;
    float left = -half_size.x + shadow_inset(radii.x, top) + shadow_inset(radii.w, bottom);
    float right = half_size.x - shadow_inset(radii.y, top) - shadow_inset(radii.z, bottom);
    float k = sqrt(0.5f) / sigma;
    return (0.5f + 0.5f * shadow_erf((x - left) * k)) - (0.5f + 0.5f * shadow_erf((x - right) * k));
}

// Use the separable Gaussian and four row samples from octara-render's reference renderer.
static float shadow_alpha(float2 p, float4 rect, float4 radii, float sigma) {
    if (sigma <= 0.0f) {
        return coverage(sd_round_rect(p, rect, radii));
    }
    float2 half_size = rect.zw * 0.5f;
    float2 center = p - (rect.xy + half_size);
    float low = center.y - half_size.y;
    float high = center.y + half_size.y;
    float start = clamp(-3.0f * sigma, low, high);
    float end = clamp(3.0f * sigma, low, high);
    float step = (end - start) / 4.0f;
    float y = start + step * 0.5f;
    float alpha = 0.0f;
    for (uint index = 0u; index < 4u; ++index) {
        alpha += blur_along_x(center.x, center.y - y, sigma, radii, half_size) * gaussian(y, sigma) * step;
        y += step;
    }
    return clamp(alpha, 0.0f, 1.0f);
}

fragment float4 shadow_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const ShadowInst* shadows [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const float4* spatials [[buffer(8)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    ShadowInst shadow = shadows[draw.payload];
    float alpha = shadow_alpha(p, shadow.rect, shadow.radii, shadow.sigma) * clip_coverage(world, draw, clips, spatials);
    return premultiply(shadow.color) * alpha;
}
