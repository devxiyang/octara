struct ResolveOptions {
    uint output_color;
    uint srgb_attachment;
};

static_assert(sizeof(ResolveOptions) == 8, "ResolveOptions layout");

// These values match octara_render::plan::OutputColor.
constant uint LINEAR_OUTPUT = 0u;
constant uint SRGB_OPAQUE_OUTPUT = 1u;
constant uint SRGB_STRAIGHT_OUTPUT = 2u;

static float3 linear_to_srgb(float3 color) {
    float3 c = clamp(color, 0.0f, 1.0f);
    return select(1.055f * pow(c, float3(1.0f / 2.4f)) - 0.055f, 12.92f * c, c <= 0.0031308f);
}

vertex float4 resolve_vertex(uint vertex_id [[vertex_id]]) {
    const float2 points[3] = {float2(-1.0f, -1.0f), float2(3.0f, -1.0f), float2(-1.0f, 3.0f)};
    return float4(points[vertex_id], 0.0f, 1.0f);
}

fragment float4 resolve_fragment(
    float4 pixel [[position]], constant ResolveOptions& options [[buffer(0)]],
    texture2d<float, access::read> source [[texture(0)]]
) {
    float4 color = source.read(uint2(pixel.xy));
    bool srgb_attachment = options.srgb_attachment != 0u;
    if (options.output_color == LINEAR_OUTPUT || (options.output_color == SRGB_OPAQUE_OUTPUT && srgb_attachment)) {
        return color;
    }
    if (options.output_color == SRGB_OPAQUE_OUTPUT) {
        return float4(linear_to_srgb(color.rgb), color.a);
    }
    // Alpha below half an 8-bit step rounds to zero and clears the RGB channels.
    if (color.a < 0.5f / 255.0f) {
        return float4(0.0f);
    }
    float3 rgb = color.rgb / color.a;
    if (options.output_color == SRGB_STRAIGHT_OUTPUT || color.a >= 1.0f) {
        return float4(srgb_attachment ? rgb : linear_to_srgb(rgb), color.a);
    }
    // An sRGB attachment applies its transfer function in hardware.
    float3 associated = linear_to_srgb(rgb) * color.a;
    return float4(srgb_attachment ? srgb_to_linear(associated) : associated, color.a);
}

fragment float4 clear_fragment(constant float4& background [[buffer(0)]]) {
    return background;
}
