struct GlyphInst {
    float2 pos;
    uint size;
    uint atlas_origin;
    uint color_rgba8;
    uint layer_flags;
};

struct GlyphDraw {
    float depth;
    uint clip;
};

struct GlyphClip {
    float4 rect;
    uint head;
    uint flags;
    uint2 pad;
};

static_assert(sizeof(GlyphInst) == 24, "GlyphInst stride");
static_assert(sizeof(GlyphDraw) == 8, "GlyphDraw stride");
static_assert(sizeof(GlyphClip) == 32, "GlyphClip stride");

constant uint COLOR_GLYPH = 0x80000000u;

static uint2 unpack_pair(uint pair) {
    return uint2(pair & 0xffffu, pair >> 16u);
}

vertex DrawVertex glyph_vertex(
    uint vertex_id [[vertex_id]], uint id [[instance_id]], constant Globals& globals [[buffer(0)]],
    device const GlyphInst* glyphs [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const GlyphDraw* draws [[buffer(6)]], device const GlyphClip* glyph_clips [[buffer(7)]],
    device const float4* spatials [[buffer(8)]]
) {
    GlyphDraw draw = draws[id];
    GlyphInst glyph = glyphs[id];
    float4 rect = float4(glyph.pos, float2(unpack_pair(glyph.size)));
    GlyphClip clip = {float4(-1.0e6f, -1.0e6f, 2.0e6f, 2.0e6f), 0u, 0u, uint2(0u)};
    if (draw.clip != 0u) {
        clip = glyph_clips[draw.clip];
    }
    DrawInst geometry = {rect, clip.rect, float4(0.0f), draw.depth, id, clip.head, clip.flags};
    return pull_vertex(vertex_id, id, rect, geometry, globals, clips, spatials);
}

fragment float4 glyph_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const GlyphInst* glyphs [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const GlyphDraw* draws [[buffer(6)]], device const GlyphClip* glyph_clips [[buffer(7)]],
    device const float4* spatials [[buffer(8)]], texture2d_array<float, access::read> atlas [[texture(1)]]
) {
    GlyphInst glyph = glyphs[v.instance];
    float2 p = v.position.xy + globals.origin;
    uint clip_index = draws[v.instance].clip;
    float2 delta = float2(0.0f);
    float clip_alpha = 1.0f;
    if (clip_index != 0u) {
        GlyphClip clip = glyph_clips[clip_index];
        delta = spatial_offset(clip.flags >> SPATIAL_SHIFT, spatials);
        DrawInst geometry = {float4(0.0f), clip.rect, float4(0.0f), 0.0f, 0u, clip.head, clip.flags};
        clip_alpha = clip_coverage(p, geometry, clips, spatials);
    }
    uint2 texel = unpack_pair(glyph.atlas_origin) + uint2(floor(p - delta - glyph.pos));
    float4 sample = atlas.read(texel, glyph.layer_flags & 0xffffu);
    float4 color = unpack_unorm4x8_to_float(glyph.color_rgba8);
    float alpha = sample.a * color.a * clip_alpha;
    float3 rgb = (glyph.layer_flags & COLOR_GLYPH) != 0u ? sample.rgb : srgb_to_linear(color.rgb);
    return float4(rgb * alpha, alpha);
}
