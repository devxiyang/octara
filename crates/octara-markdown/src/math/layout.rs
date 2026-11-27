use super::{
    MathError, MathLayout, MathRenderStyle,
    parser::{Class, Expr, Font, GridAlign, Limits},
};
use octara::prelude::*;
use std::{collections::HashMap, sync::Arc};
use swash::{
    FontRef,
    scale::{ScaleContext, Scaler},
    zeno::{Command, PathData},
};

const FONT: &[u8] = include_bytes!("../../fonts/STIXTwoMath-subset.otf");
// Axis and rule thickness in em units, from STIX Two Math's MATH constants.
const AXIS: f32 = 0.258;
const RULE: f32 = 0.068;

pub(super) struct Geometry {
    path: Path,
    size: Size,
    baseline: f32,
}

impl Geometry {
    pub(super) fn render(&self, style: MathRenderStyle) -> MathLayout {
        let em = if style.font_size.is_finite() { style.font_size.clamp(1.0, 4096.0) } else { 16.0 };
        let size = Size::new(self.size.width * em, self.size.height * em);
        let mut canvas = Canvas::new(size.width, size.height);
        if !self.path.commands().is_empty() {
            canvas.transform(Transform::scale(em, em), |canvas| canvas.fill(&self.path, style.color));
        }
        MathLayout { size, baseline: self.baseline * em, ops: Arc::from(canvas.into_ops()) }
    }
}

pub(super) fn layout(expr: &Expr, display: bool) -> Result<Geometry, MathError> {
    let font = FontRef::from_index(FONT, 0).expect("bundled math font");
    let mut scale = ScaleContext::new();
    let scaler = scale.builder(font).build();
    let mut typesetter = Typesetter { font, scaler, glyphs: HashMap::new() };
    let style = Style { em: if display { 1.12 } else { 1.0 }, font: Font::Math, display };
    let node = typesetter.node(expr, style)?;
    let pad = 0.045;
    let size = Size::new(node.width + 2.0 * pad, node.ascent + node.depth + 2.0 * pad);
    let baseline = node.ascent + pad;
    let path = node.path.transform(Transform::translate(pad, baseline));
    Ok(Geometry { path, size, baseline })
}

struct Typesetter<'a> {
    font: FontRef<'a>,
    scaler: Scaler<'a>,
    glyphs: HashMap<char, MathBox>,
}

#[derive(Clone, Copy)]
struct Style {
    em: f32,
    font: Font,
    display: bool,
}

impl Style {
    fn script(self, factor: f32) -> Self {
        Self { em: (self.em * factor).max(0.28), display: false, ..self }
    }
}

/// Ink expressed relative to its baseline, with descent measured downward.
#[derive(Clone, Default)]
struct MathBox {
    path: Path,
    width: f32,
    ascent: f32,
    depth: f32,
}

impl MathBox {
    fn space(width: f32) -> Self {
        Self { width, ..Self::default() }
    }

    fn rule(width: f32, height: f32) -> Self {
        Self { path: Path::rect(Rect::new(0.0, -height, width, height)), width, ascent: height, depth: 0.0 }
    }

    fn scale(self, x: f32, y: f32) -> Self {
        Self { path: self.path.transform(Transform::scale(x, y)), width: self.width * x, ascent: self.ascent * y, depth: self.depth * y }
    }

    /// Includes the next element at `x` with its baseline at `y`.
    fn place(&mut self, next: Self, x: f32, y: f32) {
        self.width = self.width.max(x + next.width);
        self.ascent = self.ascent.max(next.ascent - y);
        self.depth = self.depth.max(next.depth + y);
        let path = next.path.transform(Transform::translate(x, y));
        self.path = std::mem::take(&mut self.path).append(&path);
    }
}

impl Typesetter<'_> {
    fn glyph(&mut self, ch: char, style: Style) -> Result<MathBox, MathError> {
        let styled = font_variant(ch, style.font);
        if let Some(glyph) = self.glyphs.get(&styled) {
            return Ok(glyph.clone().scale(style.em, style.em));
        }
        let map = self.font.charmap();
        let mut id = map.map(styled);
        if id == 0 {
            id = map.map(ch);
        }
        if id == 0 {
            return Err(MathError::new(format!("no math glyph for {ch:?}")));
        }
        let scale = 1.0 / f32::from(self.font.metrics(&[]).units_per_em);
        let mut glyph = MathBox::space(self.font.glyph_metrics(&[]).advance_width(id) * scale);
        if let Some(outline) = self.scaler.scale_outline(id)
            && !outline.points().is_empty()
        {
            let bounds = outline.bounds();
            let left = bounds.min.x.min(0.0);
            let point = |p: swash::zeno::Point| Point::new((p.x - left) * scale, -p.y * scale);
            glyph.width = glyph.width.max(bounds.max.x * scale) - left * scale;
            glyph.ascent = bounds.max.y.max(0.0) * scale;
            glyph.depth = -bounds.min.y.min(0.0) * scale;
            glyph.path = Path::from_commands(outline.path().commands().map(|command| match command {
                Command::MoveTo(p) => PathCmd::MoveTo(point(p)),
                Command::LineTo(p) => PathCmd::LineTo(point(p)),
                Command::QuadTo(a, p) => PathCmd::QuadTo(point(a), point(p)),
                Command::CurveTo(a, b, p) => PathCmd::CurveTo(point(a), point(b), point(p)),
                Command::Close => PathCmd::Close,
            }));
        }
        self.glyphs.insert(styled, glyph.clone());
        Ok(glyph.scale(style.em, style.em))
    }

    fn node(&mut self, expr: &Expr, style: Style) -> Result<MathBox, MathError> {
        let em = style.em;
        Ok(match expr {
            Expr::Row(children) => self.row(children, style)?,
            Expr::Symbol(ch, class, _) => {
                let mut glyph = self.glyph(*ch, style)?;
                if *class == Class::Operator {
                    let factor = if style.display { 1.40 } else { 1.10 };
                    glyph = glyph.scale(factor, factor);
                    let y = (glyph.ascent - glyph.depth) * 0.5 - AXIS * em;
                    let mut centered = MathBox::default();
                    centered.place(glyph, 0.0, y);
                    centered
                } else {
                    glyph
                }
            }
            Expr::Space(width) => MathBox::space(width * em),
            Expr::Text(text) => {
                let style = Style { font: if matches!(style.font, Font::Math) { Font::Upright } else { style.font }, ..style };
                let mut line = MathBox::default();
                for ch in text.chars() {
                    line.place(self.glyph(ch, style)?, line.width, 0.0);
                }
                line
            }
            Expr::Font(font, body) => self.node(body, Style { font: *font, ..style })?,
            Expr::Fraction(numerator, denominator) => {
                let child = style.script(if style.display { 0.90 } else { 0.72 });
                let num = self.node(numerator, child)?;
                let den = self.node(denominator, child)?;
                fraction(num, den, em)
            }
            Expr::Radical(degree, body) => self.radical(degree.as_deref(), body, style)?,
            Expr::Scripts { base, sub, sup, limits } => {
                let base = self.node(base, style)?;
                let child = style.script(0.70);
                let sub = sub.as_deref().map(|sub| self.node(sub, child)).transpose()?;
                let sup = sup.as_deref().map(|sup| self.node(sup, child)).transpose()?;
                scripts(base, sub, sup, *limits == Limits::Always || (*limits == Limits::Display && style.display), em)
            }
            Expr::Delimited(left, body, right) => {
                let body = self.node(body, style)?;
                self.delimited(*left, body, *right, style)?
            }
            Expr::Grid { rows, left, right, align } => {
                let grid = self.grid(rows, *align, style)?;
                self.delimited(*left, grid, *right, style)?
            }
            Expr::Overline(body) => {
                let mut body = self.node(body, style)?;
                let offset = -body.ascent - 0.09 * em;
                body.place(MathBox::rule(body.width, RULE * em), 0.0, offset);
                body
            }
        })
    }

    fn row(&mut self, children: &[Expr], style: Style) -> Result<MathBox, MathError> {
        let mut row = MathBox::default();
        let mut previous = None;
        let mut x = 0.0;
        for (index, expr) in children.iter().enumerate() {
            if let Expr::Space(width) = expr {
                x = (x + width * style.em).max(0.0);
                continue;
            }
            let mut class = expr.class();
            if class == Class::Binary
                && (matches!(previous, None | Some(Class::Binary | Class::Relation | Class::Open | Class::Operator | Class::Punctuation))
                    || children
                        .get(index + 1)
                        .is_none_or(|next| matches!(next.class(), Class::Close | Class::Relation | Class::Punctuation)))
            {
                class = Class::Ordinary;
            }
            x += previous.map_or(0.0, |prev| spacing(prev, class)) * style.em;
            let element = self.node(expr, style)?;
            let width = element.width;
            row.place(element, x, 0.0);
            x += width;
            previous = Some(class);
        }
        row.width = row.width.max(x);
        Ok(row)
    }

    fn radical(&mut self, degree: Option<&Expr>, expr: &Expr, style: Style) -> Result<MathBox, MathError> {
        let em = style.em;
        let body = self.node(expr, style)?;
        let top = body.ascent.max(0.50 * em) + 0.09 * em;
        let bottom = body.depth + 0.04 * em;
        let mut sign = self.glyph('√', style)?;
        let total = sign.ascent + sign.depth;
        let extend = ((top + bottom + RULE * em) / total).max(1.0);
        sign = sign.scale(1.0, extend);
        let mut result = MathBox::default();
        let mut x = 0.0;
        if let Some(degree) = degree {
            let degree = self.node(degree, style.script(0.56))?;
            x = (degree.width - sign.width * 0.36).max(0.0);
            result.place(degree, 0.0, -top * 0.55);
        }
        let join = x + sign.width - 0.075 * em;
        let y = -top - RULE * em + sign.ascent;
        result.place(sign, x, y);
        result.place(MathBox::rule(body.width + 0.13 * em, RULE * em), join, -top);
        result.place(body, join + 0.09 * em, 0.0);
        Ok(result)
    }

    fn delimited(&mut self, left: char, body: MathBox, right: char, style: Style) -> Result<MathBox, MathError> {
        let axis = AXIS * style.em;
        let total = 2.0 * (body.ascent - axis).max(body.depth + axis) + 0.10 * style.em;
        let mut row = MathBox::default();
        if left != '.' {
            let glyph = self.delimiter(left, total, style)?;
            let y = (glyph.ascent - glyph.depth) * 0.5 - axis;
            row.place(glyph, 0.0, y);
            row.width += 0.06 * style.em;
        }
        row.place(body, row.width, 0.0);
        if right != '.' {
            let glyph = self.delimiter(right, total, style)?;
            let y = (glyph.ascent - glyph.depth) * 0.5 - axis;
            row.place(glyph, row.width + 0.06 * style.em, y);
        }
        Ok(row)
    }

    fn delimiter(&mut self, ch: char, height: f32, style: Style) -> Result<MathBox, MathError> {
        let glyph = self.glyph(ch, style)?;
        let scale = (height / (glyph.ascent + glyph.depth)).max(1.0);
        Ok(glyph.scale(1.0 + (scale - 1.0).min(2.0) * 0.10, scale))
    }

    fn grid(&mut self, rows: &[Vec<Expr>], align: GridAlign, style: Style) -> Result<MathBox, MathError> {
        let em = style.em;
        let child = Style { display: false, ..style };
        let gap = 0.25 * em;
        let mut cells = Vec::new();
        let mut widths = Vec::<f32>::new();
        let mut height = -gap;
        for row in rows {
            widths.resize(widths.len().max(row.len()), 0.0);
            let mut ascent = 0.62 * em;
            let mut depth = 0.17 * em;
            let cell_row = row
                .iter()
                .zip(&mut widths)
                .map(|(expr, width)| {
                    let cell = self.node(expr, child)?;
                    *width = width.max(cell.width);
                    ascent = ascent.max(cell.ascent);
                    depth = depth.max(cell.depth);
                    Ok(cell)
                })
                .collect::<Result<Vec<_>, MathError>>()?;
            cells.push((cell_row, height + gap + ascent));
            height += ascent + depth + gap;
        }
        let y = -height * 0.5 - AXIS * em;
        let mut grid = MathBox::default();
        for (row, baseline) in cells {
            let mut x = 0.0;
            for (index, (cell, &width)) in row.into_iter().zip(&widths).enumerate() {
                let align = match align {
                    GridAlign::Center => 0.5,
                    GridAlign::Left => 0.0,
                    GridAlign::Alternating if index % 2 == 0 => 1.0,
                    GridAlign::Alternating => 0.0,
                };
                let left = x + (width - cell.width) * align;
                grid.place(cell, left, y + baseline);
                x += width + 0.58 * em;
            }
        }
        Ok(grid)
    }
}

fn fraction(num: MathBox, den: MathBox, em: f32) -> MathBox {
    let margin = 0.11 * em;
    let axis = AXIS * em;
    let line = RULE * em;
    let gap = 0.10 * em;
    let width = num.width.max(den.width);
    let num_at = (margin + (width - num.width) * 0.5, -axis - line * 0.5 - gap - num.depth);
    let den_at = (margin + (width - den.width) * 0.5, -axis + line * 0.5 + gap + den.ascent);
    let mut result = MathBox::default();
    result.place(MathBox::rule(width + 2.0 * margin, line), 0.0, -axis + line * 0.5);
    result.place(num, num_at.0, num_at.1);
    result.place(den, den_at.0, den_at.1);
    result
}

fn scripts(base: MathBox, sub: Option<MathBox>, sup: Option<MathBox>, limits: bool, em: f32) -> MathBox {
    let gap = 0.09 * em;
    let width =
        if limits { base.width.max(sub.as_ref().map_or(0.0, |s| s.width)).max(sup.as_ref().map_or(0.0, |s| s.width)) } else { base.width };
    let base_x = (width - base.width) * 0.5;
    let ascent = base.ascent;
    let depth = base.depth;
    let mut result = MathBox::default();
    result.place(base, base_x, 0.0);
    let y_sup = sup
        .as_ref()
        .map_or(0.0, |sup| if limits { -ascent - gap - sup.depth } else { -(0.39 * em).max(ascent * 0.65).max(sup.depth + 0.28 * em) });
    let y_sub = sub.as_ref().map_or(0.0, |sub| {
        if limits {
            depth + gap + sub.ascent
        } else {
            (0.23 * em).max(depth + 0.1 * em).max(y_sup + sup.as_ref().map_or(0.0, |s| s.depth) + gap + sub.ascent)
        }
    });
    if let Some(sup) = sup {
        let x = if limits { (width - sup.width) * 0.5 } else { width + 0.03 * em };
        result.place(sup, x, y_sup);
    }
    if let Some(sub) = sub {
        let x = if limits { (width - sub.width) * 0.5 } else { width + 0.03 * em };
        result.place(sub, x, y_sub);
    }
    result
}

fn spacing(left: Class, right: Class) -> f32 {
    use Class::*;
    match (left, right) {
        (Open, _) | (_, Close | Punctuation) => 0.0,
        (Relation, _) | (_, Relation) => 5.0 / 18.0,
        (Binary, _) | (_, Binary) => 2.0 / 9.0,
        (Punctuation | Operator, _) | (_, Operator) => 1.0 / 6.0,
        _ => 0.0,
    }
}

fn font_variant(ch: char, font: Font) -> char {
    let offset = match (font, ch) {
        (Font::Math | Font::Italic, 'h') => return 'ℎ',
        (Font::Math | Font::Italic, 'a'..='z') => (0x1D44E, 'a'),
        (Font::Italic, 'A'..='Z') | (Font::Math, 'A'..='Z') => (0x1D434, 'A'),
        (Font::Math | Font::Italic, 'α'..='ω') => (0x1D6FC, 'α'),
        (Font::Bold, 'A'..='Z') => (0x1D400, 'A'),
        (Font::Bold, 'a'..='z') => (0x1D41A, 'a'),
        (Font::Bold, '0'..='9') => (0x1D7CE, '0'),
        (Font::Bold, 'Α'..='Ω') => (0x1D6A8, 'Α'),
        (Font::Bold, 'α'..='ω') => (0x1D6C2, 'α'),
        (Font::Blackboard, 'C') => return 'ℂ',
        (Font::Blackboard, 'H') => return 'ℍ',
        (Font::Blackboard, 'N') => return 'ℕ',
        (Font::Blackboard, 'P') => return 'ℙ',
        (Font::Blackboard, 'Q') => return 'ℚ',
        (Font::Blackboard, 'R') => return 'ℝ',
        (Font::Blackboard, 'Z') => return 'ℤ',
        (Font::Blackboard, 'A'..='Z') => (0x1D538, 'A'),
        (Font::Blackboard, 'a'..='z') => (0x1D552, 'a'),
        (Font::Blackboard, '0'..='9') => (0x1D7D8, '0'),
        _ => return ch,
    };
    char::from_u32(offset.0 + ch as u32 - offset.1 as u32).unwrap_or(ch)
}
