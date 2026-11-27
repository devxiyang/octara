struct GlyphInst {
    pos: vec2f,
    size: u32,
    atlas_origin: u32,
    color_rgba8: u32,
    layer_flags: u32,
};

@group(0) @binding(1) var atlas: texture_2d_array<f32>;
@group(0) @binding(2) var<storage, read> glyphs: array<GlyphInst>;

struct GlyphDraw {
    depth: f32,
    clip: u32,
};

struct GlyphClip {
    rect: vec4f,
    head: u32,
    flags: u32,
};

@group(0) @binding(6) var<storage, read> glyph_draws: array<GlyphDraw>;
@group(0) @binding(7) var<storage, read> glyph_clips: array<GlyphClip>;

const COLOR_GLYPH: u32 = 0x80000000u;

fn unpack_pair(pair: u32) -> vec2u {
    return vec2u(pair & 0xffffu, pair >> 16u);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) id: u32) -> VsOut {
    let draw = glyph_draws[id];
    let glyph = glyphs[id];
    let rect = vec4f(glyph.pos, vec2f(unpack_pair(glyph.size)));
    var clip = GlyphClip(vec4f(-1e6, -1e6, 2e6, 2e6), 0u, 0u);
    if draw.clip != 0u {
        clip = glyph_clips[draw.clip];
    }
    return draw_vertex(vi, id, rect, DrawInst(rect, clip.rect, vec4f(0.0), draw.depth, id, clip.head, clip.flags));
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4f {
    let glyph = glyphs[v.inst_id];
    let p = v.pos.xy + globals.origin;
    let clip_index = glyph_draws[v.inst_id].clip;
    var delta = vec2f(0.0);
    var clip_alpha = 1.0;
    if clip_index != 0u {
        let clip = glyph_clips[clip_index];
        delta = spatial_offset(clip.flags >> SPATIAL_SHIFT);
        clip_alpha = clip_coverage(p, DrawInst(vec4f(0.0), clip.rect, vec4f(0.0), 0.0, 0u, clip.head, clip.flags));
    }
    let texel = vec2i(unpack_pair(glyph.atlas_origin)) + vec2i(floor(p - delta - glyph.pos));
    let sample = textureLoad(atlas, texel, i32(glyph.layer_flags & 0xffffu), 0);
    let color = unpack4x8unorm(glyph.color_rgba8);
    let alpha = sample.a * color.a * clip_alpha;
    if (glyph.layer_flags & COLOR_GLYPH) != 0u {
        return vec4f(sample.rgb * alpha, alpha);
    }
    return vec4f(srgb_to_linear(color.rgb) * alpha, alpha);
}
