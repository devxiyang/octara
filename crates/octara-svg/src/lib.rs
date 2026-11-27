//! Optional SVG decoding for materializing monochrome Octara icons.

use octara_model::geometry::{Point, Rect};
use octara_model::paint::{FillRule, Icon, IconShape, LineCap, LineJoin, Path, PathCmd, PathStyle, Stroke, Transform};

/// SVG an icon cannot be made from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SvgError {
    /// Not well-formed XML, or no `<svg>` root.
    Xml(String),
    /// An element or attribute value outside the supported subset (paths and basic shapes,
    /// groups, fill and stroke, transforms).
    Unsupported(String),
    /// Bad path data in a `d` attribute.
    PathData(String),
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SvgError::Xml(e) => write!(f, "not an SVG document: {e}"),
            SvgError::Unsupported(what) => write!(f, "unsupported SVG: {what}"),
            SvgError::PathData(e) => write!(f, "invalid path data: {e}"),
        }
    }
}

impl std::error::Error for SvgError {}

/// Decode an SVG path's `d` attribute into format-independent path commands.
pub fn parse_path(data: &str) -> Result<Path, SvgError> {
    let bezier = kurbo::BezPath::from_svg(data).map_err(|error| SvgError::PathData(error.to_string()))?;
    let point = |point: kurbo::Point| Point::new(point.x as f32, point.y as f32);
    let path = Path::from_commands(bezier.elements().iter().map(|command| match *command {
        kurbo::PathEl::MoveTo(at) => PathCmd::MoveTo(point(at)),
        kurbo::PathEl::LineTo(at) => PathCmd::LineTo(point(at)),
        kurbo::PathEl::QuadTo(control, at) => PathCmd::QuadTo(point(control), point(at)),
        kurbo::PathEl::CurveTo(first, second, at) => PathCmd::CurveTo(point(first), point(second), point(at)),
        kurbo::PathEl::ClosePath => PathCmd::Close,
    }));
    if path.commands().iter().all(finite_command) { Ok(path) } else { Err(SvgError::PathData("coordinates must be finite".into())) }
}

fn finite_command(command: &PathCmd) -> bool {
    let valid = |point: &Point| point.x.is_finite() && point.y.is_finite();
    match command {
        PathCmd::MoveTo(at) | PathCmd::LineTo(at) => valid(at),
        PathCmd::QuadTo(control, at) => valid(control) && valid(at),
        PathCmd::CurveTo(first, second, at) => valid(first) && valid(second) && valid(at),
        PathCmd::Close => true,
    }
}

#[derive(Clone, Copy)]
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

fn unsupported(what: impl Into<String>) -> SvgError {
    SvgError::Unsupported(what.into())
}

fn finite(s: &str) -> Option<f32> {
    s.parse::<f32>().ok().filter(|value| value.is_finite())
}

fn numbers(s: &str) -> Result<Vec<f32>, SvgError> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| finite(t).ok_or_else(|| unsupported(format!("number `{t}`"))))
        .collect()
}

fn number(n: roxmltree::Node, attr: &str) -> Result<f32, SvgError> {
    match n.attribute(attr) {
        None => Ok(0.0),
        Some(v) => finite(v.trim().trim_end_matches("px")).ok_or_else(|| unsupported(format!("{attr}=\"{v}\""))),
    }
}

/// `translate`, `scale`, `rotate` (about a point too), `matrix`, `skewX` and `skewY`, in
/// sequence.
fn transform(s: &str) -> Result<Transform, SvgError> {
    let mut t = Transform::IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = rest.find('(').ok_or_else(|| unsupported(format!("transform=\"{s}\"")))?;
        let close = rest.find(')').ok_or_else(|| unsupported(format!("transform=\"{s}\"")))?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let arguments = rest.get(open + 1..close).ok_or_else(|| unsupported(format!("transform=\"{s}\"")))?;
        let a = numbers(arguments)?;
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

fn inherit(n: roxmltree::Node, parent: &Inherited) -> Result<Inherited, SvgError> {
    let mut s = *parent;
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

fn shape(n: roxmltree::Node) -> Result<Option<Path>, SvgError> {
    let f = |a: &str| number(n, a);
    let pts = |closed: bool| -> Result<Path, SvgError> {
        let v = numbers(n.attribute("points").unwrap_or(""))?;
        let (pairs, remainder) = v.as_chunks::<2>();
        if !remainder.is_empty() {
            return Err(unsupported("points must contain coordinate pairs"));
        }
        let p: Vec<_> = pairs.iter().map(|[x, y]| Point::new(*x, *y)).collect();
        Ok(Path::new().add_polygon(&p, closed))
    };
    Ok(Some(match n.tag_name().name() {
        "path" => parse_path(n.attribute("d").unwrap_or(""))?,
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

fn walk(n: roxmltree::Node, parent: &Inherited, out: &mut Vec<IconShape>) -> Result<(), SvgError> {
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
        if !path.commands().iter().all(finite_command) {
            return Err(unsupported("transformed path exceeds finite coordinates"));
        }
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

pub fn parse(source: &str) -> Result<Icon, SvgError> {
    let doc = roxmltree::Document::parse(source).map_err(|e| SvgError::Xml(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::Xml(format!("the root is <{}>, not <svg>", root.tag_name().name())));
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
    Ok(Icon::new("svg", view_box, shapes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_subset() {
        let lucide = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m21 21-4.34-4.34" /><circle cx="11" cy="11" r="8" />
            <circle cx="7.5" cy="7.5" r=".5" fill="currentColor"/></svg>"#;
        let icon = parse(lucide).unwrap();
        let shapes = icon.shapes();
        assert_eq!(shapes.len(), 4, "two strokes, and a dot both filled and stroked");
        assert!(matches!(&shapes[0].style, PathStyle::Stroke(s) if s.width == 2.0 && s.cap == LineCap::Round));
        assert!(matches!(shapes[2].style, PathStyle::Fill(FillRule::NonZero)));

        let logo = parse(
            r#"<svg viewBox="0 0 10 10"><g transform="translate(5 0) scale(2)"><rect width="1" height="1" fill-rule="evenodd"/></g></svg>"#,
        )
        .unwrap();
        let s = logo.shapes();
        assert_eq!(s.len(), 1, "filled by default, not stroked");
        assert_eq!(s[0].style, PathStyle::Fill(FillRule::EvenOdd));
        let b = s[0].path.bounds().unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (5.0, 0.0, 2.0, 2.0));
        assert_eq!(logo.view_box(), Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });

        assert!(matches!(parse("<svg><image href='x.png'/></svg>"), Err(SvgError::Unsupported(e)) if e == "<image>"));
        assert!(matches!(parse("<svg"), Err(SvgError::Xml(_))));
        assert!(parse("<svg><path transform=')(' d='M1 2'/></svg>").is_err());
        assert!(parse("<svg><polyline points='1,2,3'/></svg>").is_err());
        assert!(parse("<svg><circle cx='NaN' cy='1' r='2'/></svg>").is_err());
        assert!(matches!(parse("<svg><path d='M0 0 K'/></svg>"), Err(SvgError::PathData(_))));
    }
}
