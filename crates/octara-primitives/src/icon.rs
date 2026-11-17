//! Icons: single-color vector drawings tinted by the current text color.
//!
//! The bundled set is in the `octara-icons` crate (`icon(icons::SEARCH)`); apps add their own
//! with [`Icon::svg`] or [`Icon::path`]. An icon is decorative (hidden from assistive technology)
//! unless it has a `label`, which makes it an image.

use std::borrow::Cow;
use std::sync::Arc;

use crate::paint::Rect;
use crate::path::{FillRule, LineCap, LineJoin, Path, PathStyle, Stroke};

/// One shape of a bundled icon: SVG path data on the icon's grid, filled and / or stroked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IconPart {
    pub data: &'static str,
    pub fill: bool,
    pub stroke: bool,
}

/// A shape of an icon, ready to paint (coordinates on the icon's view box).
#[derive(Clone, PartialEq, Debug)]
pub struct IconShape {
    pub path: Path,
    pub style: PathStyle,
}

#[derive(Clone, Debug)]
enum Parts {
    Static(&'static [IconPart]),
    Shapes(Arc<[IconShape]>),
}

/// A single-color vector icon: shapes on a view box. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct Icon {
    name: Cow<'static, str>,
    view_box: Rect,
    parts: Parts,
}

/// SVG an icon cannot be made from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IconError {
    /// Not well-formed XML, or no `<svg>` root.
    Xml(String),
    /// An element or attribute value outside the supported subset (paths and basic shapes,
    /// groups, fill and stroke, transforms).
    Unsupported(String),
    /// Bad path data in a `d` attribute.
    PathData(String),
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::Xml(e) => write!(f, "not an SVG document: {e}"),
            IconError::Unsupported(what) => write!(f, "unsupported SVG: {what}"),
            IconError::PathData(e) => write!(f, "invalid path data: {e}"),
        }
    }
}

impl std::error::Error for IconError {}

/// The grid bundled icons are drawn on, and their stroke width on it.
pub const GRID: f32 = 24.0;
pub const STROKE_WIDTH: f32 = 2.0;

impl Icon {
    /// A bundled icon: path data on the 24-unit grid, stroked 2 units wide with round caps and
    /// joins (Lucide's form), some parts also filled.
    pub const fn stroked(name: &'static str, parts: &'static [IconPart]) -> Icon {
        Icon { name: Cow::Borrowed(name), view_box: Rect { x: 0.0, y: 0.0, w: GRID, h: GRID }, parts: Parts::Static(parts) }
    }

    /// An icon from one path, filled (non-zero) on the 24-unit grid; change the grid with
    /// [`view_box`](Self::view_box).
    pub fn path(path: Path) -> Icon {
        let shape = IconShape { path, style: PathStyle::Fill(FillRule::NonZero) };
        Icon { name: Cow::Borrowed("path"), view_box: Rect { x: 0.0, y: 0.0, w: GRID, h: GRID }, parts: Parts::Shapes(Arc::new([shape])) }
    }

    /// An icon from SVG source: `<path>`, `<rect>`, `<circle>`, `<ellipse>`, `<line>`,
    /// `<polyline>`, `<polygon>` in `<g>` groups, with `fill`, `stroke`, `stroke-width`,
    /// `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit`, `fill-rule` and `transform`.
    /// Colors are ignored: the icon takes the text color. Needs the `svg` feature (default).
    #[cfg(feature = "svg")]
    pub fn svg(source: &str) -> Result<Icon, IconError> {
        svg::parse(source)
    }

    /// Names the icon (shown by `inspect`).
    pub fn named(mut self, name: impl Into<Cow<'static, str>>) -> Icon {
        self.name = name.into();
        self
    }

    /// The area of the icon's coordinates that is shown, fitted into the element.
    pub fn view_box(mut self, view_box: Rect) -> Icon {
        self.view_box = view_box;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn bounds(&self) -> Rect {
        self.view_box
    }

    /// The shapes to paint; `stroke_width` (on the view box's units) replaces every stroke's.
    pub fn shapes(&self, stroke_width: Option<f32>) -> Vec<IconShape> {
        let mut out = match &self.parts {
            Parts::Static(parts) => {
                let mut out = Vec::new();
                for p in parts.iter() {
                    let Ok(path) = Path::svg(p.data) else { continue };
                    if p.fill {
                        out.push(IconShape { path: path.clone(), style: PathStyle::Fill(FillRule::NonZero) });
                    }
                    if p.stroke {
                        let s = Stroke::new(STROKE_WIDTH).cap(LineCap::Round).join(LineJoin::Round);
                        out.push(IconShape { path, style: PathStyle::Stroke(s) });
                    }
                }
                out
            }
            Parts::Shapes(s) => s.to_vec(),
        };
        if let Some(w) = stroke_width {
            for s in &mut out {
                if let PathStyle::Stroke(st) = &mut s.style {
                    st.width = w;
                }
            }
        }
        out
    }
}

/// The SVG subset icons are made from.
#[cfg(feature = "svg")]
mod svg {
    use super::*;
    use crate::path::Transform;

    #[derive(Clone)]
    struct Inherited {
        fill: Option<FillRule>,
        stroke: bool,
        width: f32,
        cap: LineCap,
        join: LineJoin,
        miter: f32,
        rule: FillRule,
        transform: Transform,
    }

    fn unsupported(what: impl Into<String>) -> IconError {
        IconError::Unsupported(what.into())
    }

    fn numbers(s: &str) -> Result<Vec<f32>, IconError> {
        s.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f32>().map_err(|_| unsupported(format!("number `{t}`"))))
            .collect()
    }

    fn number(n: roxmltree::Node, attr: &str) -> Result<f32, IconError> {
        match n.attribute(attr) {
            None => Ok(0.0),
            Some(v) => v.trim().trim_end_matches("px").parse().map_err(|_| unsupported(format!("{attr}=\"{v}\""))),
        }
    }

    /// `translate`, `scale`, `rotate` (about a point too), `matrix`, `skewX` and `skewY`, in
    /// sequence.
    fn transform(s: &str) -> Result<Transform, IconError> {
        let mut t = Transform::IDENTITY;
        let mut rest = s.trim();
        while !rest.is_empty() {
            let open = rest.find('(').ok_or_else(|| unsupported(format!("transform=\"{s}\"")))?;
            let close = rest.find(')').ok_or_else(|| unsupported(format!("transform=\"{s}\"")))?;
            let name = rest[..open].trim().trim_start_matches(',').trim();
            let a = numbers(&rest[open + 1..close])?;
            let step = match (name, a.as_slice()) {
                ("translate", [x]) => Transform::translate(*x, 0.0),
                ("translate", [x, y]) => Transform::translate(*x, *y),
                ("scale", [k]) => Transform::scale(*k, *k),
                ("scale", [x, y]) => Transform::scale(*x, *y),
                ("rotate", [deg]) => Transform::rotate(*deg),
                ("rotate", [deg, cx, cy]) => {
                    Transform::translate(-cx, -cy).then(&Transform::rotate(*deg)).then(&Transform::translate(*cx, *cy))
                }
                ("matrix", [a, b, c, d, e, f]) => Transform { xx: *a, xy: *b, yx: *c, yy: *d, x: *e, y: *f },
                ("skewX", [deg]) => Transform { yx: deg.to_radians().tan(), ..Transform::IDENTITY },
                ("skewY", [deg]) => Transform { xy: deg.to_radians().tan(), ..Transform::IDENTITY },
                _ => return Err(unsupported(format!("transform=\"{s}\""))),
            };
            // Listed transforms apply right to left: the last one first.
            t = step.then(&t);
            rest = rest[close + 1..].trim();
        }
        Ok(t)
    }

    fn inherit(n: roxmltree::Node, parent: &Inherited) -> Result<Inherited, IconError> {
        let mut s = parent.clone();
        if let Some(v) = n.attribute("fill") {
            s.fill = (v.trim() != "none").then_some(s.rule);
        }
        if let Some(v) = n.attribute("fill-rule") {
            s.rule = match v.trim() {
                "evenodd" => FillRule::EvenOdd,
                "nonzero" => FillRule::NonZero,
                _ => return Err(unsupported(format!("fill-rule=\"{v}\""))),
            };
            s.fill = s.fill.map(|_| s.rule);
        }
        if let Some(v) = n.attribute("stroke") {
            s.stroke = v.trim() != "none";
        }
        if n.attribute("stroke-width").is_some() {
            s.width = number(n, "stroke-width")?;
        }
        if let Some(v) = n.attribute("stroke-linecap") {
            s.cap = match v.trim() {
                "butt" => LineCap::Butt,
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                _ => return Err(unsupported(format!("stroke-linecap=\"{v}\""))),
            };
        }
        if let Some(v) = n.attribute("stroke-linejoin") {
            s.join = match v.trim() {
                "miter" | "miter-clip" => LineJoin::Miter,
                "round" => LineJoin::Round,
                "bevel" => LineJoin::Bevel,
                _ => return Err(unsupported(format!("stroke-linejoin=\"{v}\""))),
            };
        }
        if n.attribute("stroke-miterlimit").is_some() {
            s.miter = number(n, "stroke-miterlimit")?;
        }
        if let Some(v) = n.attribute("transform") {
            s.transform = transform(v)?.then(&s.transform);
        }
        Ok(s)
    }

    fn shape(n: roxmltree::Node) -> Result<Option<Path>, IconError> {
        let f = |a: &str| number(n, a);
        let pts = |closed: bool| -> Result<Path, IconError> {
            let v = numbers(n.attribute("points").unwrap_or(""))?;
            let p: Vec<_> = v.as_chunks::<2>().0.iter().map(|[x, y]| crate::path::point(*x, *y)).collect();
            Ok(Path::new().add_polygon(&p, closed))
        };
        Ok(Some(match n.tag_name().name() {
            "path" => Path::svg(n.attribute("d").unwrap_or("")).map_err(|e| IconError::PathData(e.0))?,
            "rect" => {
                let r = Rect { x: f("x")?, y: f("y")?, w: f("width")?, h: f("height")? };
                let rx = if n.has_attribute("rx") { f("rx")? } else { f("ry")? };
                Path::rounded_rect(r, rx)
            }
            "circle" => Path::circle((f("cx")?, f("cy")?), f("r")?),
            "ellipse" => Path::ellipse((f("cx")?, f("cy")?), f("rx")?, f("ry")?),
            "line" => Path::new().move_to((f("x1")?, f("y1")?)).line_to((f("x2")?, f("y2")?)),
            "polyline" => pts(false)?,
            "polygon" => pts(true)?,
            _ => return Ok(None),
        }))
    }

    fn walk(n: roxmltree::Node, parent: &Inherited, out: &mut Vec<IconShape>) -> Result<(), IconError> {
        for c in n.children().filter(|c| c.is_element()) {
            let tag = c.tag_name().name();
            if matches!(tag, "title" | "desc" | "metadata" | "defs") {
                continue;
            }
            let s = inherit(c, parent)?;
            if tag == "g" {
                walk(c, &s, out)?;
                continue;
            }
            let Some(path) = shape(c)? else { return Err(unsupported(format!("<{tag}>"))) };
            let path = if s.transform == Transform::IDENTITY { path } else { path.transform(s.transform) };
            if let Some(rule) = s.fill {
                out.push(IconShape { path: path.clone(), style: PathStyle::Fill(rule) });
            }
            if s.stroke && s.width > 0.0 {
                // A transform scales the stroke as it scales the shape (on average along the axes).
                let k = ((s.transform.xx * s.transform.yy - s.transform.xy * s.transform.yx).abs()).sqrt();
                let st = Stroke::new(s.width * k).cap(s.cap).join(s.join).miter_limit(s.miter);
                out.push(IconShape { path, style: PathStyle::Stroke(st) });
            }
        }
        Ok(())
    }

    pub(super) fn parse(source: &str) -> Result<Icon, IconError> {
        let doc = roxmltree::Document::parse(source).map_err(|e| IconError::Xml(e.to_string()))?;
        let root = doc.root_element();
        if root.tag_name().name() != "svg" {
            return Err(IconError::Xml(format!("the root is <{}>, not <svg>", root.tag_name().name())));
        }
        let view_box = match root.attribute("viewBox") {
            Some(v) => match numbers(v)?.as_slice() {
                [x, y, w, h] if *w > 0.0 && *h > 0.0 => Rect { x: *x, y: *y, w: *w, h: *h },
                _ => return Err(unsupported(format!("viewBox=\"{v}\""))),
            },
            None => Rect { x: 0.0, y: 0.0, w: number(root, "width")?.max(1.0), h: number(root, "height")?.max(1.0) },
        };
        let base = Inherited {
            fill: Some(FillRule::NonZero),
            stroke: false,
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 4.0,
            rule: FillRule::NonZero,
            transform: Transform::IDENTITY,
        };
        let top = inherit(root, &base)?;
        let mut shapes = Vec::new();
        walk(root, &top, &mut shapes)?;
        Ok(Icon { name: Cow::Borrowed("svg"), view_box, parts: Parts::Shapes(shapes.into()) })
    }
}

#[cfg(all(test, feature = "svg"))]
mod tests {
    use super::*;

    #[test]
    fn svg_subset() {
        let lucide = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m21 21-4.34-4.34" /><circle cx="11" cy="11" r="8" />
            <circle cx="7.5" cy="7.5" r=".5" fill="currentColor"/></svg>"#;
        let icon = Icon::svg(lucide).unwrap();
        let shapes = icon.shapes(None);
        assert_eq!(shapes.len(), 4, "two strokes, and a dot both filled and stroked");
        assert!(matches!(&shapes[0].style, PathStyle::Stroke(s) if s.width == 2.0 && s.cap == LineCap::Round));
        assert!(matches!(shapes[2].style, PathStyle::Fill(FillRule::NonZero)));
        assert!(matches!(&icon.shapes(Some(1.5))[1].style, PathStyle::Stroke(s) if s.width == 1.5));

        let logo = Icon::svg(
            r#"<svg viewBox="0 0 10 10"><g transform="translate(5 0) scale(2)"><rect width="1" height="1" fill-rule="evenodd"/></g></svg>"#,
        )
        .unwrap();
        let s = logo.shapes(None);
        assert_eq!(s.len(), 1, "filled by default, not stroked");
        assert_eq!(s[0].style, PathStyle::Fill(FillRule::EvenOdd));
        let b = s[0].path.bounds().unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (5.0, 0.0, 2.0, 2.0));
        assert_eq!(logo.bounds(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });

        assert!(matches!(Icon::svg("<svg><image href='x.png'/></svg>"), Err(IconError::Unsupported(e)) if e == "<image>"));
        assert!(matches!(Icon::svg("<svg"), Err(IconError::Xml(_))));
        assert!(matches!(Icon::svg("<svg><path d='M0 0 K'/></svg>"), Err(IconError::PathData(_))));
    }
}
