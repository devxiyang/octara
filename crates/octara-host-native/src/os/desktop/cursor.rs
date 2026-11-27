use octara_model::input::CursorStyle;
use winit::window::CursorIcon;

pub(super) fn cursor_icon(style: CursorStyle) -> Option<CursorIcon> {
    Some(match style {
        CursorStyle::Arrow | CursorStyle::Auto => CursorIcon::Default,
        CursorStyle::IBeam => CursorIcon::Text,
        CursorStyle::PointingHand => CursorIcon::Pointer,
        CursorStyle::Crosshair => CursorIcon::Crosshair,
        CursorStyle::OpenHand => CursorIcon::Grab,
        CursorStyle::ClosedHand => CursorIcon::Grabbing,
        CursorStyle::ResizeLeftRight => CursorIcon::EwResize,
        CursorStyle::ResizeUpDown => CursorIcon::NsResize,
        CursorStyle::ResizeColumn => CursorIcon::ColResize,
        CursorStyle::ResizeRow => CursorIcon::RowResize,
        CursorStyle::OperationNotAllowed => CursorIcon::NotAllowed,
        CursorStyle::Wait => CursorIcon::Wait,
        CursorStyle::Move => CursorIcon::Move,
        CursorStyle::Help => CursorIcon::Help,
        CursorStyle::None => return None,
        CursorStyle::ContextMenu => CursorIcon::ContextMenu,
        CursorStyle::Progress => CursorIcon::Progress,
        CursorStyle::Cell => CursorIcon::Cell,
        CursorStyle::VerticalText => CursorIcon::VerticalText,
        CursorStyle::Alias => CursorIcon::Alias,
        CursorStyle::Copy => CursorIcon::Copy,
        CursorStyle::NoDrop => CursorIcon::NoDrop,
        CursorStyle::AllScroll => CursorIcon::AllScroll,
        CursorStyle::ResizeNorth => CursorIcon::NResize,
        CursorStyle::ResizeEast => CursorIcon::EResize,
        CursorStyle::ResizeSouth => CursorIcon::SResize,
        CursorStyle::ResizeWest => CursorIcon::WResize,
        CursorStyle::ResizeNorthEast => CursorIcon::NeResize,
        CursorStyle::ResizeNorthWest => CursorIcon::NwResize,
        CursorStyle::ResizeSouthEast => CursorIcon::SeResize,
        CursorStyle::ResizeSouthWest => CursorIcon::SwResize,
        CursorStyle::ResizeNorthEastSouthWest => CursorIcon::NeswResize,
        CursorStyle::ResizeNorthWestSouthEast => CursorIcon::NwseResize,
        CursorStyle::ZoomIn => CursorIcon::ZoomIn,
        CursorStyle::ZoomOut => CursorIcon::ZoomOut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_visible_and_hidden_cursors_for_winit() {
        assert_eq!(cursor_icon(CursorStyle::Auto), Some(CursorIcon::Default));
        assert_eq!(cursor_icon(CursorStyle::Arrow), Some(CursorIcon::Default));
        assert_eq!(cursor_icon(CursorStyle::IBeam), Some(CursorIcon::Text));
        assert_eq!(cursor_icon(CursorStyle::PointingHand), Some(CursorIcon::Pointer));
        assert_eq!(cursor_icon(CursorStyle::OperationNotAllowed), Some(CursorIcon::NotAllowed));
        assert_eq!(cursor_icon(CursorStyle::ResizeLeftRight), Some(CursorIcon::EwResize));
        assert_eq!(cursor_icon(CursorStyle::None), None);
    }
}
