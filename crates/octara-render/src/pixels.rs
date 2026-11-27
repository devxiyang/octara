//! Checked dimensions shared by the capture renderers.

/// Captures preserve the existing one-pixel minimum, including an empty logical viewport.
pub(crate) fn dimensions(size: (f32, f32), scale: f32) -> Result<(u32, u32), String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("capture scale must be finite and positive".into());
    }
    if !size.0.is_finite() || !size.1.is_finite() || size.0 < 0.0 || size.1 < 0.0 {
        return Err("capture dimensions must be finite and non-negative".into());
    }
    let width = (size.0 * scale).ceil().max(1.0);
    let height = (size.1 * scale).ceil().max(1.0);
    if !width.is_finite() || !height.is_finite() || f64::from(width) > f64::from(u32::MAX) || f64::from(height) > f64::from(u32::MAX) {
        return Err("capture dimensions exceed the pixel coordinate range".into());
    }
    Ok((width as u32, height as u32))
}

pub(crate) fn count(width: u32, height: u32) -> Option<usize> {
    (width as usize).checked_mul(height as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_round_and_keep_the_one_pixel_minimum() {
        assert_eq!(dimensions((3.25, 2.5), 2.0).unwrap(), (7, 5));
        assert_eq!(dimensions((0.0, 0.0), 1.0).unwrap(), (1, 1));
        assert_eq!(dimensions((0.0, 3.25), 2.0).unwrap(), (1, 7));
    }

    #[test]
    fn dimensions_reject_invalid_or_unrepresentable_inputs() {
        for scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
            assert!(dimensions((1.0, 1.0), scale).is_err());
        }
        for size in [
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
            (-1.0, 1.0),
            (1.0, -1.0),
            (u32::MAX as f32, 1.0),
            (1.0, u32::MAX as f32),
            (f32::MAX, f32::MAX),
        ] {
            assert!(dimensions(size, 1.0).is_err());
        }
        assert!(dimensions((f32::MAX, 1.0), 2.0).is_err());
    }
}
