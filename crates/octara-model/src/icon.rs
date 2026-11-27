//! Monochrome vector shapes tinted by the host's text color.

use super::{FillRule, Path, PathStyle};
use crate::geometry::Rect;
use std::sync::Arc;

/// One paintable shape in the icon's coordinate system.
#[derive(Clone, PartialEq, Debug)]
pub struct IconShape {
    pub path: Path,
    pub style: PathStyle,
}

/// A reusable icon on its own coordinate grid.
#[derive(Clone, PartialEq, Debug)]
pub struct Icon {
    name: Arc<str>,
    view_box: Rect,
    shapes: Arc<[IconShape]>,
}

impl Icon {
    pub fn new(name: impl Into<Arc<str>>, view_box: Rect, shapes: impl Into<Arc<[IconShape]>>) -> Self {
        Self { name: name.into(), view_box, shapes: shapes.into() }
    }

    /// An icon from a single filled path on a 24-unit grid.
    pub fn path(path: Path) -> Self {
        Self::new("path", Rect::new(0.0, 0.0, 24.0, 24.0), [IconShape { path, style: PathStyle::Fill(FillRule::NonZero) }])
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn view_box(&self) -> Rect {
        self.view_box
    }

    pub fn shapes(&self) -> &[IconShape] {
        &self.shapes
    }
}

/// The resolved shapes a node draws, including its optional stroke width override.
#[derive(Clone, PartialEq, Debug)]
pub struct NodeIcon {
    pub name: String,
    pub view_box: Rect,
    pub shapes: Vec<IconShape>,
}

impl NodeIcon {
    pub fn new(icon: &Icon, stroke_width: Option<f32>) -> Self {
        let mut shapes = icon.shapes.to_vec();
        if let Some(width) = stroke_width {
            for shape in &mut shapes {
                if let PathStyle::Stroke(stroke) = &mut shape.style {
                    stroke.width = width;
                }
            }
        }
        Self { name: icon.name.to_string(), view_box: icon.view_box, shapes }
    }
}
