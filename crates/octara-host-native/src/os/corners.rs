use objc2_core_graphics::CGMutablePath;
use objc2_quartz_core::{CALayer, CAShapeLayer};

pub(super) fn apply(layer: &CALayer, [tl, tr, br, bl]: [f32; 4], top_left: bool) {
    if tl == tr && tl == br && tl == bl {
        // SAFETY: This layer owns its mask; clearing it detaches it from the layer.
        unsafe { layer.setMask(None) };
        layer.setCornerRadius(f64::from(tl));
        return;
    }

    let radii = if top_left { [tl, tr, br, bl] } else { [bl, br, tr, tl] };
    let [tl, tr, br, bl] = radii.map(f64::from);
    let bounds = layer.bounds();
    let (w, h) = (bounds.size.width, bounds.size.height);
    let path = CGMutablePath::new();
    let transform = std::ptr::null();
    let p = Some(&*path);
    // SAFETY: The path is owned here and a null transform uses the supplied local coordinates.
    unsafe {
        CGMutablePath::move_to_point(p, transform, tl, 0.0);
        for (x, y, next_x, next_y, radius) in [(w, 0.0, w, h, tr), (w, h, 0.0, h, br), (0.0, h, 0.0, 0.0, bl), (0.0, 0.0, w, 0.0, tl)] {
            if radius > 0.0 {
                CGMutablePath::add_arc_to_point(p, transform, x, y, next_x, next_y, radius);
            } else {
                CGMutablePath::add_line_to_point(p, transform, x, y);
            }
        }
    }
    CGMutablePath::close_subpath(p);

    let mask = CAShapeLayer::layer();
    mask.setFrame(bounds);
    mask.setPath(Some(&path));
    layer.setCornerRadius(0.0);
    // SAFETY: The fresh mask has no superlayer and is only installed on this layer.
    unsafe { layer.setMask(Some(&mask)) };
}
