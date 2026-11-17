//! Input values shared by retained UI and platform hosts.

/// How the pointer looks (GPUI's names).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Arrow,
    /// Over text that can be selected or typed in.
    IBeam,
    /// Over a link.
    PointingHand,
    Crosshair,
    /// Over something that can be dragged.
    OpenHand,
    /// While dragging it.
    ClosedHand,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeColumn,
    ResizeRow,
    OperationNotAllowed,
}
