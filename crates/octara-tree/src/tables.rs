//! Cold payloads are stored independently from the compact node header.

pub use crate::editor::EditorSlot;
use crate::focus::FocusGroup;
use crate::overlay::{LayerGroupId, LayerPolicy, OverlayMount};
use crate::{NodeId, RowKey};
use octara_model::geometry::{Placed, Placement, Vec2};
use octara_model::image::{ImageSource, ImageState, MediaAsset};
use octara_model::input::{CursorStyle, DragPayload, DropAxis};
use octara_model::motion::{Curve, TextReveal};
use octara_model::native::{NativeSlot as NativeLayer, TypedNativeProps};
use octara_model::overlay::AnchorTarget;
use octara_model::paint::{CanvasOp, Image, NodeIcon, ObjectFit, PaintInput, PaintMount, PaintProgram};
use octara_model::semantics::{Checked, Live, NumericRange, Relations, Role};
use octara_model::style::{Inherited, StyleProp, When};
use octara_model::text::{InlineAttachment, RichSpan};
use octara_model::theme::{Theme, ThemePatch};
use slotmap::SparseSecondaryMap;
use smallvec::SmallVec;
use std::collections::BTreeMap;
use std::ops::Range;
use std::panic::Location;
use std::sync::Arc;

// Define the storage and its presence bits together so a new table cannot
// omit a bit or be missed when a node is reclaimed.
macro_rules! cold_tables {
    ($($field:ident: $value:ty => $bit:ident = $position:literal),+ $(,)?) => {
        bitflags::bitflags! {
            #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
            pub struct TableMask: u32 {
                $(const $bit = 1 << $position;)+
            }
        }

        #[derive(Default)]
        pub(crate) struct Tables {
            $(pub $field: SparseSecondaryMap<NodeId, $value>,)+
        }

        impl Tables {
            pub fn remove(&mut self, node: NodeId, present: TableMask) -> Option<PaintSlot> {
                // Paint assets are handed to the host for deferred retirement.
                let paint = present.contains(TableMask::PAINTS).then(|| self.paints.remove(node)).flatten();
                $(
                    if TableMask::$bit != TableMask::PAINTS && present.contains(TableMask::$bit) {
                        self.$field.remove(node);
                    }
                )+
                paint
            }

            pub fn presence(&self, node: NodeId) -> TableMask {
                let mut present = TableMask::empty();
                $(present.set(TableMask::$bit, self.$field.contains_key(node));)+
                present
            }
        }
    };
}

cold_tables! {
    children: Vec<NodeId> => CHILDREN = 0,
    styles: StyleSlot => STYLES = 1,
    theme_patches: ThemePatch => THEME_PATCHES = 2,
    theme_scopes: ThemeScope => THEME_SCOPES = 3,
    texts: TextSlot => TEXTS = 4,
    links: LinkSlot => LINKS = 5,
    editors: EditorSlot => EDITORS = 6,
    images: ImageSlot => IMAGES = 7,
    icons: Arc<NodeIcon> => ICONS = 8,
    canvases: Arc<[CanvasOp]> => CANVASES = 9,
    natives: NativeSlot => NATIVES = 10,
    paints: PaintSlot => PAINTS = 11,
    semantics: SemanticsSlot => SEMANTICS = 12,
    handlers: HandlerSlot => HANDLERS = 13,
    commands: CommandSlot => COMMANDS = 14,
    overlays: OverlaySlot => OVERLAYS = 15,
    scrolls: ScrollSlot => SCROLLS = 16,
    provenance: Provenance => PROVENANCE = 17,
    text_reveals: TextReveal => TEXT_REVEALS = 18,
    media_sources: MediaSourceSlot => MEDIA_SOURCES = 19,
    inline_media: BTreeMap<usize, MediaSourceSlot> => INLINE_MEDIA = 20,
}

pub(crate) struct ThemeScope {
    pub base_revision: Option<u64>,
    pub revision: u64,
    pub theme: Theme,
}

#[derive(Debug, Default)]
pub struct AuthoredStyle {
    pub base: SmallVec<[StyleProp; 8]>,
    pub overrides: SmallVec<[(When, Vec<StyleProp>); 2]>,
    pub group: Option<&'static str>,
}

#[derive(Debug)]
pub struct StyleSlot {
    pub(crate) theme_revision: u64,
    pub(crate) sensitivity: u64,
    pub authored: AuthoredStyle,
    pub(crate) resolved: Arc<crate::style::ComputedStyle>,
    pub inherited: Inherited,
    pub inherited_from: Inherited,
    pub(crate) opacity_motion_authored: bool,
}

impl std::ops::Deref for StyleSlot {
    type Target = crate::style::ComputedStyle;

    fn deref(&self) -> &Self::Target {
        &self.resolved
    }
}

#[derive(Debug, Default)]
pub struct TextSlot {
    pub text: Arc<str>,
    pub runs: Vec<(Range<usize>, RichSpan)>,
    pub attachments: Vec<InlineAttachment>,
    pub links: Vec<NodeId>,
    pub text_rev: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSlot {
    pub range: Range<usize>,
    pub href: Arc<str>,
}

impl LinkSlot {
    /// The URL for default host navigation, if its scheme is supported.
    pub fn navigation_href(&self) -> Option<&str> {
        let href = self.href.trim();
        let scheme = href.split_once(':').map(|(scheme, _)| scheme);
        let allowed = scheme.is_none_or(|scheme| ["https", "http", "mailto", "tel"].iter().any(|url| scheme.eq_ignore_ascii_case(url)));
        (allowed && !href.is_empty()).then_some(href)
    }
}

#[derive(Clone, Debug)]
pub struct MediaSourceSlot {
    pub source: ImageSource,
    pub fit: ObjectFit,
    pub state: ImageState,
    pub asset: Option<MediaAsset>,
    pub(crate) size_hint: Option<(u32, u32)>,
}

impl MediaSourceSlot {
    pub(crate) fn new(source: ImageSource, fit: ObjectFit) -> Self {
        Self { source, fit, state: ImageState::Idle, asset: None, size_hint: None }
    }

    /// The current dimensions, or the last known dimensions of this source while loading is paused.
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        self.state.natural_size().or(self.size_hint)
    }

    pub(crate) fn set(&mut self, state: ImageState, asset: Option<MediaAsset>) -> bool {
        if self.state == state && self.asset == asset {
            return false;
        }
        if let Some(size) = state.natural_size().filter(|&(width, height)| width > 0 && height > 0) {
            self.size_hint = Some(size);
        }
        self.state = state;
        self.asset = asset;
        true
    }
}

#[derive(Clone)]
pub struct ImageSlot {
    pub image: Image,
    pub fit: ObjectFit,
}

pub struct NativeSlot {
    pub kind: &'static str,
    pub slot: NativeLayer,
    pub props: serde_json::Value,
    pub typed_props: Option<TypedNativeProps>,
    pub props_rev: u64,
    pub live: bool,
}

#[derive(Debug)]
pub struct CustomPaintBinding {
    pub program: PaintProgram,
    pub input: PaintInput,
    pub mount: PaintMount,
}

#[derive(Debug)]
pub struct TransitionBinding {
    pub program: PaintProgram,
    pub curve: Curve,
    pub key: u64,
    pub mount: PaintMount,
}

#[derive(Debug, Default)]
pub struct PaintSlot {
    pub custom: Option<CustomPaintBinding>,
    pub transition: Option<TransitionBinding>,
}

#[derive(Clone, Debug, Default)]
pub struct SemanticsSlot {
    pub role: Option<Role>,
    pub label: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub tooltip: Option<Arc<str>>,
    pub a11y_value: Option<Arc<str>>,
    pub relations: Relations<Arc<str>>,
    pub numeric_range: Option<NumericRange>,
    pub checked: Option<Checked>,
    pub selected: Option<bool>,
    pub multiselectable: Option<bool>,
    pub required: Option<bool>,
    pub invalid: Option<bool>,
    pub expanded: Option<bool>,
    pub level: Option<u8>,
    pub live: Option<Live>,
    pub cursor: Option<CursorStyle>,
    pub tab_index: i32,
    pub focus_preferred: bool,
    pub focus_group: Option<FocusGroup>,
    pub group_current: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandlerKind {
    Press,
    SecondaryPress,
    ContextMenu,
    DoublePress,
    Toggle,
    Drag,
    Drop,
    DropHover,
    FileDrop,
    Paste,
    TouchScrollPre,
    TouchScrollPost,
    Dismiss,
    Key,
    Focus,
    Blur,
    Hover,
    Input,
    Submit,
    Scroll,
    Command,
    Link,
    Native,
    Accessibility,
}

#[derive(Debug, Default)]
pub(crate) struct HandlerSlot {
    pub callbacks: SmallVec<[(HandlerKind, HandlerId); 2]>,
    pub drag_drop: Option<Box<DragDrop>>,
}

#[derive(Debug, Default)]
pub struct DragDrop {
    pub source: Option<DragPayload>,
    pub keyboard: bool,
    pub target: Option<DropTarget>,
}

#[derive(Debug)]
pub struct DropTarget {
    pub kinds: Arc<[Arc<str>]>,
    pub axis: DropAxis,
}

#[derive(Debug, Default)]
pub struct CommandSlot {
    pub key_context: Option<Arc<str>>,
    pub commands: SmallVec<[(Arc<str>, HandlerId); 2]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlaySlot {
    pub target: Option<(AnchorTarget, Placement)>,
    pub placed: Option<Placed>,
    pub layer: Option<LayerPolicy>,
    pub group: Option<LayerGroupId>,
    pub mount: Option<OverlayMount>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollSlot {
    pub offset: Vec2,
    pub requested: Option<(u64, Vec2)>,
    pub clamped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    pub name: &'static str,
    pub module: &'static str,
    pub at: &'static Location<'static>,
}

#[derive(Debug)]
pub struct Provenance {
    pub id: Option<Arc<str>>,
    pub test_id: Option<Arc<str>>,
    pub row_key: Option<RowKey>,
    pub component: Option<Component>,
    pub origin: &'static Location<'static>,
}

impl Default for Provenance {
    #[track_caller]
    fn default() -> Self {
        Self { id: None, test_id: None, row_key: None, component: None, origin: Location::caller() }
    }
}
