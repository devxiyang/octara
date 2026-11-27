//! Values and capabilities used to author views.

pub use octara_model::ask::{
    Answer, Ask, ClipboardItem, ClipboardKinds, Menu, MenuItem, PathPromptOptions, PickedFile, PromptLevel, ServiceError, ServiceOutcome,
    ServiceRequest,
};
pub use octara_model::geometry::{Align, Edges, LayoutBox, Placement, Point, Rect, Side, Size, Vec2, WindowViewport, point};
pub use octara_model::image::{CachePolicy, HttpMediaEntry, ImageSource, ImageState, VideoSource, VideoSurface};
pub use octara_model::input::{
    Button, Chord, ContextMenuEvent, ContextMenuItem, CopyFormat, CursorStyle, DragPayload, DropAxis, DropEvent, DropHover, DropPosition,
    DroppedFile, Key, KeyEvent, LinkEvent, LinkEventKind, Modifiers, PasteEvent,
};
pub use octara_model::motion::{Curve, Motion, Spring, TextReveal, TextRevealKind};
pub use octara_model::native::{NativeCommand, NativeEvent, NativeSlot};
pub use octara_model::overlay::AnchorTarget;
pub use octara_model::paint::{
    Canvas, CanvasOp, CanvasText, FillRule, Icon, Image, LineCap, LineJoin, NodeIcon, ObjectFit, PaintInput, PaintProgram, Path, PathCmd,
    PathStyle, Stroke, Transform,
};
pub use octara_model::semantics::{A11yAction, Checked, Live, Role, TextPosition};
pub use octara_model::style::{
    AlignItems, Display, FlexDirection, FlexWrap, GridPos, JustifyContent, LayoutLength, Overflow, Position, ScrollbarVisibility,
    StyleProp, UserSelect,
};
pub use octara_model::text::{
    FontStyle, FontWeight, InlineAlign, InlineAttachment, InlineContent, InlinePill, PillStyle, RichSpan, TextAlign, Wrap as TextWrap,
};
pub use octara_model::theme::{
    BoxShadow, Breakpoint, Color, ColorScheme, ColorToken, ColorValue, ContainerWidthToken, Family as FontFamilyToken, Fill, FontSizeToken,
    FontWeightToken, Gradient, LeadingToken, LinearGradient, RadiusToken, ResponsiveBreakpoint, ShadowToken, TextScale, Theme, ThemePatch,
    contrast, linear_color_stop, linear_gradient, over, palette, rgb,
};
pub use octara_model::window::{
    SystemPreferences, TitlebarStyle, UserAttention, VibrancyMaterial, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowMaterial,
    WindowPresentation,
};
pub use octara_runtime::graph::{
    delta::{Changes, Cursor},
    list::{FlatItem, FlatKey, GroupInfo, Groups, IntoReadList, ListDelta, ReadList, RowSignal, StateList},
    text_buf::TextEdit as TextBufferEdit,
    text_stream::{TextStreamError, TextStreamOp, TextStreamSink},
    value::Memo,
};
pub use octara_runtime::media::{MediaCacheBackend, MediaResolver, VideoSurfaceProvider};
pub use octara_runtime::task::{Receiver, Sender, TaskId, channel};
pub use octara_runtime::{
    Binding, BuildCx, CleanupCx, CloseHandle, Field, HandlerEvent, HandlerOutcome, HoveredTooltip, Navigator, ReadCx, ReadSignal, Readable,
    RestartState, ScopeId, Signal, Store, TaskCx, TextBuffer, Trigger, Watch, WindowId, WindowOptions, Writable, WriteCx,
};
pub use octara_tree::{
    arbiter::{Drag as DragEvent, DragPhase, InteractionPayload, TouchScrollStage},
    focus::FocusGroup,
    overlay::{FocusTrapFeatures, LayerGroupId, LayerPolicy},
    tables::HandlerKind,
};

/// Paint a single line at its full width or ellipsize it at its available width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextOverflow {
    Visible,
    Ellipsis,
}

impl From<TextOverflow> for TextWrap {
    fn from(value: TextOverflow) -> Self {
        match value {
            TextOverflow::Visible => Self::None,
            TextOverflow::Ellipsis => Self::Truncate,
        }
    }
}
