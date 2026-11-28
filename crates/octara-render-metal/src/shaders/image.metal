struct ImageInst {
    float4 rect;
    float4 image_bounds;
    float4 radius;
    float4 uv;  // atlas origin and size in base-level texels
    float4 fill;
    uint layer;
    float max_lod;
    uint kind;
    uint gradient;
};

static_assert(sizeof(ImageInst) == 96, "ImageInst stride");

constant uint IMAGE = 0u;
constant uint PATH = 1u;

static float4 image_sample(float2 p, ImageInst image, texture2d_array<float> atlas, sampler image_sampler) {
    float2 footprint = image.uv.zw / image.rect.zw;
    float lod = clamp(log2(max(footprint.x, footprint.y)), 0.0f, image.max_lod);
    float2 texel = image.uv.xy + quad_uv(p, image.rect) * image.uv.zw;
    float2 uv = texel / float2(atlas.get_width(), atlas.get_height());
    // The premultiplied atlas has an edge-replicating gutter at each mip level.
    return atlas.sample(image_sampler, uv, image.layer, level(lod));
}

fragment float4 image_core_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const ImageInst* images [[buffer(2)]], device const DrawInst* draws [[buffer(6)]],
    device const float4* spatials [[buffer(8)]], texture2d_array<float> atlas [[texture(1)]],
    sampler image_sampler [[sampler(3)]]
) {
    DrawInst draw = draws[v.instance];
    float2 p = v.position.xy + globals.origin - draw_offset(draw, spatials);
    return image_sample(p, images[draw.payload], atlas, image_sampler);
}

fragment float4 image_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const ImageInst* images [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const GradientInst* gradients [[buffer(7)]],
    device const float4* spatials [[buffer(8)]], texture2d_array<float> atlas [[texture(1)]],
    sampler image_sampler [[sampler(3)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    ImageInst image = images[draw.payload];
    float clip = clip_coverage(world, draw, clips, spatials);

    switch (image.kind) {
        case IMAGE: {
            if (in_core(p, draw.core)) {
                return float4(0.0f);
            }
            // Antialias placement and the rounded image bounds independently.
            float edge = coverage(sd_round_rect(p, image.rect, float4(0.0f)));
            float inside = coverage(sd_round_rect(p, image.image_bounds, image.radius));
            float alpha = edge * inside * image.fill.a * clip;
            if (alpha <= 0.0f) {
                return float4(0.0f);
            }
            return image_sample(p, image, atlas, image_sampler) * alpha;
        }
        case PATH: {
            float4 fill = image.gradient == 0u ? premultiply(image.fill)
                                               : gradient_at(p, image.image_bounds, gradients[image.gradient]);
            uint2 texel = uint2(image.uv.xy + floor(p - image.rect.xy));
            float mask = atlas.read(texel, image.layer).r;
            return fill * (mask * clip);
        }
        default:
            return float4(0.0f);
    }
}

fragment float4 layer_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const ImageInst* images [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const float4* spatials [[buffer(8)]],
    texture2d<float, access::read> layer [[texture(4)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    ImageInst image = images[draw.payload];
    // An isolated layer holds premultiplied pixels at the target resolution.
    return layer.read(uint2(p - image.rect.xy)) * (image.fill.a * clip_coverage(world, draw, clips, spatials));
}

fragment float4 canvas_fragment(
    DrawVertex v [[stage_in]], constant Globals& globals [[buffer(0)]],
    device const ImageInst* images [[buffer(2)]], device const ClipNode* clips [[buffer(5)]],
    device const DrawInst* draws [[buffer(6)]], device const float4* spatials [[buffer(8)]],
    texture2d<float> canvas [[texture(4)]], sampler image_sampler [[sampler(3)]]
) {
    float2 world = v.position.xy + globals.origin;
    DrawInst draw = draws[v.instance];
    float2 p = world - draw_offset(draw, spatials);
    ImageInst image = images[draw.payload];
    float outer = coverage(sd_round_rect(p, image.rect, image.radius));
    float4 color = canvas.sample(image_sampler, quad_uv(p, image.rect), level(0.0f));
    return color * (outer * image.fill.a * clip_coverage(world, draw, clips, spatials));
}
