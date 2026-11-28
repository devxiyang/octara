use super::LayoutLength;
use crate::geometry::{Point, Rect};

/// A 4×4 matrix indexed by column, then row, for use with column vectors.
pub type Mat4 = [[f32; 4]; 4];

/// The inverse projection of a transform's z = 0 source plane.
#[derive(Clone, Copy, Debug)]
pub struct PlaneInverse([[f64; 3]; 3]);

impl PlaneInverse {
    /// Invert the planar homography obtained from a column-major transform matrix.
    pub fn new(matrix: &Mat4) -> Option<Self> {
        let [a, b, c] = [0, 1, 3].map(|column| [0, 1, 3].map(|row| f64::from(matrix[column][row])));
        let cross = |[ax, ay, az]: [f64; 3], [bx, by, bz]: [f64; 3]| [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx];
        let rows = [cross(b, c), cross(c, a), cross(a, b)];
        let determinant: f64 = a.into_iter().zip(rows[0]).map(|(a, b)| a * b).sum();
        (determinant != 0.0 && determinant.is_finite()).then(|| Self(rows.map(|row| row.map(|value| value / determinant))))
    }

    /// Columns of the inverse homography, including its homogeneous scale.
    pub fn columns(&self) -> [[f64; 3]; 3] {
        std::array::from_fn(|column| self.0.map(|row| row[column]))
    }

    /// Unproject a destination point, rejecting source points at or beyond the near plane.
    pub fn source_point(&self, destination: Point) -> Option<Point> {
        let [x, y, w] = self.0.map(|[a, b, c]| a * f64::from(destination.x) + b * f64::from(destination.y) + c);
        // The output w of the inverse is the reciprocal of the forward w.
        if !(0.0 < w && w <= 100_000.0) {
            return None;
        }
        let point = Point::new((x / w) as f32, (y / w) as f32);
        (point.x.is_finite() && point.y.is_finite()).then_some(point)
    }
}

/// The pivot within the border box. Percentages are fractions of the corresponding box dimension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformOrigin {
    pub x: LayoutLength,
    pub y: LayoutLength,
    /// Depth in logical pixels, positive toward the viewer.
    pub z: f32,
}

impl Default for TransformOrigin {
    fn default() -> Self {
        Self { x: LayoutLength::Pct(0.5), y: LayoutLength::Pct(0.5), z: 0.0 }
    }
}

impl TransformOrigin {
    /// Resolves the pivot to world coordinates in logical pixels.
    pub fn resolve(self, rect: Rect) -> [f32; 3] {
        let length = |value, basis| match value {
            LayoutLength::Px(px) => px,
            LayoutLength::Pct(fraction) => fraction * basis,
        };
        [rect.x + length(self.x, rect.w), rect.y + length(self.y, rect.h), self.z]
    }
}

/// A CSS-ordered 3D transform of a visual layer. The default is the identity transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform3D {
    /// Perspective distance in logical pixels. Nonnegative distances are clamped to at least one pixel.
    pub perspective: Option<f32>,
    /// X, Y, and Z translations in logical pixels.
    pub translation: [f32; 3],
    /// X, Y, and Z rotations in degrees.
    pub rotation: [f32; 3],
    /// X, Y, and Z scale factors.
    pub scale: [f32; 3],
    pub origin: TransformOrigin,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self { perspective: None, translation: [0.0; 3], rotation: [0.0; 3], scale: [1.0; 3], origin: TransformOrigin::default() }
    }
}

impl Transform3D {
    pub const fn perspective(mut self, distance: f32) -> Self {
        self.perspective = Some(distance);
        self
    }

    pub const fn translate(mut self, x: f32, y: f32, z: f32) -> Self {
        self.translation = [x, y, z];
        self
    }

    pub const fn rotate_x(mut self, degrees: f32) -> Self {
        self.rotation[0] = degrees;
        self
    }

    pub const fn rotate_y(mut self, degrees: f32) -> Self {
        self.rotation[1] = degrees;
        self
    }

    pub const fn rotate_z(mut self, degrees: f32) -> Self {
        self.rotation[2] = degrees;
        self
    }

    pub const fn scale(mut self, x: f32, y: f32, z: f32) -> Self {
        self.scale = [x, y, z];
        self
    }

    pub const fn origin(mut self, origin: TransformOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// The local logical-pixel matrix, before applying the origin.
    /// Its columns are the arguments to CSS `matrix3d` in order.
    /// The product is `P × T × Rx × Ry × Rz × S`, so scale acts first on a column vector.
    pub fn matrix(&self) -> Mat4 {
        let [(sx, cx), (sy, cy), (sz, cz)] = self.rotation.map(|degrees| degrees.to_radians().sin_cos());
        let rotate_x = [[1.0, 0.0, 0.0, 0.0], [0.0, cx, sx, 0.0], [0.0, -sx, cx, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let rotate_y = [[cy, 0.0, -sy, 0.0], [0.0, 1.0, 0.0, 0.0], [sy, 0.0, cy, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let rotate_z = [[cz, sz, 0.0, 0.0], [-sz, cz, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let mut matrix = multiply(rotate_x, multiply(rotate_y, rotate_z));
        matrix[3][..3].copy_from_slice(&self.translation);
        for (column, scale) in matrix.iter_mut().zip(self.scale) {
            *column = column.map(|value| value * scale);
        }
        if let Some(distance) = self.perspective.filter(|distance| *distance >= 0.0) {
            for column in &mut matrix {
                column[3] -= column[2] / distance.max(1.0);
            }
        }
        matrix
    }

    /// Transforms world device-pixel coordinates about the origin of a world logical border box.
    /// For points on the input plane use `[x, y, 0, 1]`, then divide by the resulting `w` to project.
    /// `scale` is the positive number of device pixels per logical pixel.
    pub fn device_matrix(&self, rect: Rect, scale: f32) -> Mat4 {
        let pivot = self.origin.resolve(rect).map(|value| value * scale);
        let mut matrix = self.matrix();
        // Convert between logical and device units on both sides of the matrix.
        for column in &mut matrix[..3] {
            column[3] /= scale;
        }
        for value in &mut matrix[3][..3] {
            *value *= scale;
        }
        multiply(translation(pivot), multiply(matrix, translation(pivot.map(|value| -value))))
    }
}

fn translation([x, y, z]: [f32; 3]) -> Mat4 {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, y, z, 1.0]]
}

fn multiply(left: Mat4, right: Mat4) -> Mat4 {
    right.map(|column| std::array::from_fn(|row| left.iter().zip(column).map(|(basis, value)| basis[row] * value).sum()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_point(matrix: Mat4, point: [f32; 4], expected: [f32; 4]) {
        for (row, expected) in expected.into_iter().enumerate() {
            let actual: f32 = matrix.iter().zip(point).map(|(column, value)| column[row] * value).sum();
            assert!((actual - expected).abs() < 0.000_02, "row {row}: {actual} != {expected}");
        }
    }

    #[test]
    fn matrix_multiplies_css_operations_in_declared_order() {
        let transform = Transform3D::default()
            .perspective(26.0)
            .translate(5.0, 7.0, 11.0)
            .rotate_x(90.0)
            .rotate_y(90.0)
            .rotate_z(90.0)
            .scale(2.0, 3.0, 4.0);

        assert_point(transform.matrix(), [1.0, 2.0, 3.0, 1.0], [17.0, 1.0, 13.0, 0.5]);
    }

    #[test]
    fn device_matrix_resolves_the_origin_and_scales_translation_and_perspective() {
        let transform = Transform3D::default().perspective(10.0).translate(8.0, -2.0, 4.0).rotate_y(90.0).origin(TransformOrigin {
            x: LayoutLength::Pct(0.25),
            y: LayoutLength::Px(12.0),
            z: 3.0,
        });
        let matrix = transform.device_matrix(Rect::new(10.0, 20.0, 40.0, 60.0), 2.0);

        assert_point(matrix, [52.0, 74.0, 0.0, 1.0], [58.0, 82.8, 3.2, 1.2]);
    }
}
