// Append after shared.metal and the author-provided float4 material(float2, float4).
// The callable receives normalized box coordinates and returns straight linear-light RGBA.
struct MaterialInst {
    float4 rect;
    float4 radius;
    float4 params;
    float opacity;
    float pad0;
    float pad1;
    float pad2;
};

static_assert(sizeof(MaterialInst) == 64, "MaterialInst stride");

fragment float4 material_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const MaterialInst* materials [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const float4* spatials [[buffer(8)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    MaterialInst paint = materials[draw.payload];
    float outer = coverage(sd_round_rect(p, paint.rect, paint.radius));
    float4 color = material(quad_uv(p, paint.rect), paint.params);
    float alpha = clamp(color.a, 0.0f, 1.0f) * paint.opacity * outer * clip_coverage(world, draw, clips, spatials);
    return float4(color.rgb * alpha, alpha);
}
