use octara_model::geometry::{Point, Rect, Size};
use octara_model::identity::NodeId;
use octara_model::paint::{DisplayList, FontFace, FontId, Glyph, GlyphRun, Item, Part, Quad};
use octara_model::theme::Color;
use std::sync::{Arc, LazyLock};

pub fn list(size: (f32, f32), background: Color, items: impl IntoIterator<Item = Item>) -> DisplayList {
    DisplayList { size: Size::from(size), background, parts: items.into_iter().map(Part::Item).collect() }
}

pub fn quad(rect: Rect, radius: f32, fill: Color) -> Quad {
    Quad {
        source: NodeId::default(),
        rect,
        radius: [radius; 4],
        fill: Some(fill),
        gradient: None,
        border: [0.0; 4],
        border_color: Color::TRANSPARENT,
    }
}

pub fn glyphs(text: &str, origin: Point, size: f32, color: Color) -> Item {
    static FONT: LazyLock<FontFace> = LazyLock::new(|| FontFace { id: FontId(1), data: Arc::from(octara_fonts::cjk::REGULAR), index: 0 });
    let font = swash::FontRef::from_index(&FONT.data, 0).unwrap();
    let map = font.charmap();
    let glyphs = text
        .chars()
        .enumerate()
        .map(|(index, character)| {
            let id = map.map(character);
            assert_ne!(id, 0, "the fixture font must contain {character}");
            Glyph { id: u32::from(id), x: index as f32 * size, y: 0.0, source_end: 0 }
        })
        .collect();
    Item::Glyphs(GlyphRun {
        source: NodeId::default(),
        font: FONT.clone(),
        size,
        coords: Vec::new(),
        embolden: false,
        skew: 0.0,
        color,
        colors: None,
        origin,
        glyphs,
    })
}
