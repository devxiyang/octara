struct GlyphInst {
    pos: vec2f,
    size: u32,
    atlas_origin: u32,
    color_rgba8: u32,
    clip_id_flags: u32,
};

@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> glyphs: array<GlyphInst>;

const COLOR_GLYPH: u32 = 0x80000000u;

struct GlyphVertex {
    @builtin(position) pos: vec4f,
    @location(0) @interpolate(flat) inst_id: u32,
};

fn unpack_pair(pair: u32) -> vec2u {
    return vec2u(pair & 0xffffu, pair >> 16u);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> GlyphVertex {
    let glyph = glyphs[id];
    let corners = array(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0), vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0));
    let p = glyph.pos + corners[vi] * vec2f(unpack_pair(glyph.size));
    let local = (p - globals.origin) / globals.size;
    return GlyphVertex(vec4f(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0, 0.0, 1.0), id);
}

fn srgb_to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

@fragment
fn fs(v: GlyphVertex) -> @location(0) vec4f {
    let glyph = glyphs[v.inst_id];
    let p = v.pos.xy + globals.origin;
    let texel = vec2i(unpack_pair(glyph.atlas_origin)) + vec2i(floor(p - glyph.pos));
    let sample = textureLoad(atlas, texel, 0);
    let color = unpack4x8unorm(glyph.color_rgba8);
    let clip = clip_chain_coverage(p, glyph.clip_id_flags & ~COLOR_GLYPH);
    let opacity = color.a * clip;
    if (glyph.clip_id_flags & COLOR_GLYPH) != 0u {
        let a = sample.a * opacity;
        return vec4f(sample.rgb * a, a);
    }
    let a = sample.r * opacity;
    return vec4f(srgb_to_linear(color.rgb) * a, a);
}
