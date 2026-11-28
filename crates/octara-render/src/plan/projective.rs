use octara_model::geometry::Rect;
use octara_model::style::{Mat4, PlaneInverse};

const NEAR: f64 = 0.000_01;

type Homography = [[f64; 3]; 3];

fn homography(matrix: &Mat4) -> Homography {
    [0, 1, 3].map(|column| [0, 1, 3].map(|row| f64::from(matrix[column][row])))
}

fn point(matrix: Homography, x: f32, y: f32) -> [f64; 3] {
    std::array::from_fn(|row| matrix[0][row] * f64::from(x) + matrix[1][row] * f64::from(y) + matrix[2][row])
}

/// Projects the source rectangle after clipping its edges to positive homogeneous w.
pub(crate) fn bounds(rect: Rect, matrix: &Mat4) -> Rect {
    projected(rect, homography(matrix), NEAR)
}

fn projected(rect: Rect, matrix: Homography, near: f64) -> Rect {
    if rect.is_empty() {
        return Rect::default();
    }
    let vertices = [
        point(matrix, rect.x, rect.y),
        point(matrix, rect.right(), rect.y),
        point(matrix, rect.right(), rect.bottom()),
        point(matrix, rect.x, rect.bottom()),
    ];
    if !vertices.iter().flatten().all(|v| v.is_finite()) {
        return Rect::default();
    }
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    let mut include = |[x, y, w]: [f64; 3]| {
        for (axis, v) in [x / w, y / w].into_iter().enumerate() {
            min[axis] = min[axis].min(v);
            max[axis] = max[axis].max(v);
        }
    };
    for (from, to) in vertices.into_iter().zip(vertices.into_iter().cycle().skip(1)) {
        let a = from[2] >= near;
        let b = to[2] >= near;
        if a {
            include(from);
        }
        if a != b {
            let t = (near - from[2]) / (to[2] - from[2]);
            include([from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t, near]);
        }
    }
    if min[0] >= max[0] || min[1] >= max[1] {
        return Rect::default();
    }
    // Preserve finite planning bounds even for a face almost at the viewpoint.
    let finite = |v: f64| v.clamp(-f64::from(f32::MAX) / 4.0, f64::from(f32::MAX) / 4.0) as f32;
    let [x, y] = min.map(finite);
    let [right, bottom] = max.map(finite);
    Rect::new(x, y, right - x, bottom - y)
}

/// The portion of the source plane that can reach the parent target. Bounds
/// grow for sampling and are limited to an allocatable texture around the pivot.
pub(crate) fn source_window(matrix: &Mat4, parent: Rect, border: Rect, limit: u32) -> Rect {
    let Some(inverse) = PlaneInverse::new(matrix) else {
        return Rect::default();
    };
    let source = projected(parent, inverse.columns(), f64::EPSILON);
    if source.is_empty() {
        return source;
    }
    let limit = limit as f32;
    let crop = |start: f32, end: f32, center: f32| {
        let first = (start - 1.0).floor();
        let last = (end + 1.0).ceil();
        let size = (last - first).min(limit);
        let start = (center - size * 0.5).floor().clamp(first, (last - size).max(first));
        (start, size)
    };
    let (x, width) = crop(source.x, source.right(), border.x + border.w * 0.5);
    let (y, height) = crop(source.y, source.bottom(), border.y + border.h * 0.5);
    Rect::new(x, y, width, height)
}
