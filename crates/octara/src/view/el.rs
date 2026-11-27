//! Element builders and their transaction writes.

use super::behavior::{Behavior, Handler};
use super::control::{OverlayBinding, Portal};
use super::prop::{IntoProp, Prop, ReadValue};
use super::style::{StylePatch, Styled, TwStyle};
use super::text::{IntoText, TextContent, mount_buffer, resolve_image_source};
use super::{Component, IntoView, View, mount_children, node_handle};
use crate::DragEvent;
use octara_model::{
    geometry::{LayoutBox, Placement, Point, Vec2},
    image::{ImageSource, ImageState, VideoSource},
    input::{
        ContextMenuEvent, CopyFormat, CursorStyle, DragPayload, DropAxis, DropEvent, DropHover, DroppedFile, KeyEvent, LinkEvent,
        PasteEvent,
    },
    motion::Curve,
    native::{NativeEvent, NativeSlot, TypedNativeProps},
    paint::{Canvas, Icon, Image, NodeIcon, ObjectFit, PaintInput, PaintMount, PaintProgram},
    semantics::{Checked, Live, NumericRange, Role},
    style::{FlexDirection, Position, State, StyleProp, When},
    theme::{ColorValue, Fill, Theme, ThemePatch},
};
use octara_runtime::media::{MediaResolver, VideoAdapter};
use octara_runtime::window::CopiedText;
use octara_runtime::{Binding, BuildCx, HandlerEvent, HandlerOutcome, MountCx, ReadCx, Signal, WriteCx};
use octara_tree::{
    NodeChange, NodeContent, NodeFlags, NodeRef, NodeSpec, RowKey,
    arbiter::InteractionPayload,
    editor::{EditorBackend, EditorRichText, EditorSlot, EditorState},
    focus::FocusGroup,
    overlay::{LayerGroupId, LayerPolicy},
    tables::{AuthoredStyle, CustomPaintBinding, DragDrop, DropTarget, HandlerKind, PaintSlot, SemanticsSlot, TransitionBinding},
};
use smallvec::SmallVec;
use std::{marker::PhantomData, mem, panic::Location, sync::Arc};

type Draw = Box<dyn FnMut(&mut ReadCx<'_>, &mut Canvas)>;
type ReadSemantics = Box<dyn FnMut(&mut ReadCx<'_>, &mut SemanticsSlot)>;
type Command = (Arc<str>, Box<dyn Fn(&mut WriteCx<'_>)>);
type CopySelection = Box<dyn Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String>>;
type StyleOverride = fn(StylePatch) -> StylePatch;

#[doc(hidden)]
pub enum ElementContent {}

enum Content {
    Node(NodeContent),
    Text(TextContent),
    Editor(EditorContent),
    Canvas(Draw),
    Media { source: Prop<ImageSource>, fit: ObjectFit },
    Video(VideoSource),
}

/// Marks an editor element constructed with [`input`] or [`textarea`].
pub enum Input {}

struct EditorContent {
    slot: Box<EditorSlot>,
    value: Option<Prop<String>>,
    rich: Option<Prop<EditorRichText>>,
    backend: Option<Arc<dyn EditorBackend>>,
    placeholder: Option<Prop<Arc<str>>>,
    secure: Option<Prop<bool>>,
    composing: Option<Signal<bool>>,
    editor_state: Option<Signal<Option<EditorState>>>,
}

/// A text field whose retained document can be controlled by a reactive string.
pub type TextInput = El<Input>;

enum StyleEntry {
    Static(StyleProp),
    Dynamic(ReadValue<StylePatch>),
}

#[derive(Default)]
struct Semantics {
    authored: SemanticsSlot,
    dynamic: SmallVec<[ReadSemantics; 2]>,
}

enum NativeProps {
    Json(Prop<serde_json::Value>),
    Typed(Prop<TypedNativeProps>),
}

struct ElementProps {
    content: Content,
    origin: &'static Location<'static>,
    component: Option<Component>,
    children: SmallVec<[View; 4]>,
    styles: SmallVec<[StyleEntry; 8]>,
    overrides: SmallVec<[(When, Vec<StyleProp>); 2]>,
    theme_patch: Option<Box<Prop<ThemePatch>>>,
    group: Option<&'static str>,
    flags: NodeFlags,
    disabled: Option<Prop<bool>>,
    geometry: SmallVec<[Signal<Option<LayoutBox>>; 1]>,
    scroll: Option<Signal<Vec2>>,
    text_reveal: Option<Prop<Option<octara_model::motion::TextReveal>>>,
    copy_selection: Option<CopySelection>,
    copy_formats: Vec<(CopyFormat, CopySelection)>,
    id: Option<Prop<Arc<str>>>,
    test_id: Option<Prop<Arc<str>>>,
    semantics: Option<Box<Semantics>>,
    behavior: Behavior,
    drag_drop: Option<Box<DragDrop>>,
    commands: SmallVec<[Command; 1]>,
    key_context: Option<Arc<str>>,
    layer: Option<LayerPolicy>,
    layer_group: Option<LayerGroupId>,
    portals: SmallVec<[Portal; 1]>,
    custom_paint: Option<(PaintProgram, Prop<PaintInput>)>,
    transition_paint: Option<(PaintProgram, Prop<u64>, Curve)>,
    native_props: Option<NativeProps>,
    image_state: Option<Signal<ImageState>>,
}

macro_rules! prop_setters {
    ($($name:ident($value:ty) => $field:ident),* $(,)?) => {$(
        #[inline(never)]
        fn $name(&mut self, value: $value) {
            self.$field = Some(value);
        }
    )*};
}

impl EditorContent {
    prop_setters! {
        set_value(Prop<String>) => value,
        set_rich(Prop<EditorRichText>) => rich,
        set_placeholder(Prop<Arc<str>>) => placeholder,
        set_secure(Prop<bool>) => secure,
    }
}

impl ElementProps {
    #[inline(never)]
    fn boxed(content: Content, origin: &'static Location<'static>) -> Box<Self> {
        Box::new(Self {
            content,
            origin,
            component: None,
            children: SmallVec::new(),
            styles: SmallVec::new(),
            overrides: SmallVec::new(),
            theme_patch: None,
            group: None,
            flags: NodeFlags::empty(),
            disabled: None,
            geometry: SmallVec::new(),
            scroll: None,
            text_reveal: None,
            copy_selection: None,
            copy_formats: Vec::new(),
            id: None,
            test_id: None,
            semantics: None,
            behavior: Behavior::default(),
            drag_drop: None,
            commands: SmallVec::new(),
            key_context: None,
            layer: None,
            layer_group: None,
            portals: SmallVec::new(),
            custom_paint: None,
            transition_paint: None,
            native_props: None,
            image_state: None,
        })
    }

    prop_setters! {
        set_test_id(Prop<Arc<str>>) => test_id,
        set_id(Prop<Arc<str>>) => id,
        set_group(&'static str) => group,
        set_paint((PaintProgram, Prop<PaintInput>)) => custom_paint,
        set_transition_paint((PaintProgram, Prop<u64>, Curve)) => transition_paint,
        set_scroll(Signal<Vec2>) => scroll,
        set_text_reveal(Prop<Option<octara_model::motion::TextReveal>>) => text_reveal,
        set_copy_selection(CopySelection) => copy_selection,
        set_disabled(Prop<bool>) => disabled,
        set_key_context(Arc<str>) => key_context,
        set_layer(LayerPolicy) => layer,
        set_layer_group(LayerGroupId) => layer_group,
        set_native_props(NativeProps) => native_props,
    }

    #[inline(never)]
    fn set_theme_patch(&mut self, patch: Prop<ThemePatch>) {
        self.theme_patch = Some(Box::new(patch));
    }

    fn push_geometry(&mut self, signal: Signal<Option<LayoutBox>>) {
        self.geometry.push(signal);
    }

    fn set_component(&mut self, component: Component) {
        self.component.get_or_insert(component);
    }

    fn editor(&mut self) -> &mut EditorContent {
        match &mut self.content {
            Content::Editor(editor) => editor,
            _ => unreachable!("text input builders contain editors"),
        }
    }

    #[inline(never)]
    fn push_style(&mut self, entry: StyleEntry) {
        self.styles.push(entry);
    }

    #[inline(never)]
    fn push_styles(&mut self, properties: &[StyleProp]) {
        self.styles.extend(properties.iter().copied().map(StyleEntry::Static));
    }

    #[inline(never)]
    fn push_override(&mut self, condition: When, patch: StylePatch) {
        self.overrides.push((condition, patch.0.into_vec()));
    }

    fn configure_override(&mut self, condition: When, configure: StyleOverride) {
        self.push_override(condition, configure(StylePatch::default()));
    }

    fn push_tw(&mut self, style: TwStyle) {
        self.push_styles(style.base);
        for &(condition, properties) in style.overrides {
            self.overrides.push((condition, properties.to_vec()));
        }
    }

    #[inline(never)]
    fn push_child(&mut self, child: View) {
        self.children.push(child);
    }

    #[inline(never)]
    fn push_children(&mut self, children: &mut dyn Iterator<Item = View>) {
        self.children.extend(children);
    }

    #[inline(never)]
    fn static_semantics(&mut self) -> &mut SemanticsSlot {
        &mut self.semantics.get_or_insert_with(Box::default).authored
    }

    #[inline(never)]
    fn push_semantic_binding(&mut self, read: ReadSemantics) {
        self.semantics.get_or_insert_with(Box::default).dynamic.push(read);
    }

    fn insert_flags(&mut self, flags: NodeFlags) {
        self.flags.insert(flags);
    }

    fn set_flag(&mut self, flag: NodeFlags, enabled: bool) {
        self.flags.set(flag, enabled);
    }

    fn set_selectable(&mut self, enabled: bool) {
        self.set_flag(NodeFlags::SELECT_REGION, enabled);
        self.set_flag(NodeFlags::SELECT_NONE, !enabled);
    }

    #[inline(never)]
    fn extend_disabled(&mut self, mut value: Prop<bool>) {
        self.disabled = Some(match self.disabled.take() {
            Some(Prop::Static(true)) => Prop::Static(true),
            Some(Prop::Dyn(mut existing)) => Prop::Dyn(Box::new(move |read| {
                existing(read)
                    || match &mut value {
                        Prop::Static(value) => *value,
                        Prop::Dyn(value) => value(read),
                    }
            })),
            _ => value,
        });
    }

    #[inline(never)]
    fn extend_behavior(&mut self, behavior: Behavior) {
        self.behavior.extend(behavior);
    }

    #[inline(never)]
    fn prepend_behavior(&mut self, mut behavior: Behavior) {
        behavior.extend(mem::take(&mut self.behavior));
        self.behavior = behavior;
    }

    fn set_drag_source(&mut self, payload: DragPayload) {
        self.drag_drop.get_or_insert_with(Box::default).source = Some(payload);
    }

    fn set_drop_target(&mut self, target: DropTarget) {
        self.drag_drop.get_or_insert_with(Box::default).target = Some(target);
    }

    fn enable_keyboard_drag(&mut self) {
        self.drag_drop.get_or_insert_with(Box::default).keyboard = true;
    }

    #[inline(never)]
    fn push_command(&mut self, command: Command) {
        self.commands.push(command);
    }

    #[inline(never)]
    fn push_portal(&mut self, portal: Portal) {
        self.portals.push(portal);
    }
}

/// A builder for one retained node. Static content, styles, and policies mount in one transaction.
///
/// Editor-only setters are available on [`TextInput`].
///
/// ```compile_fail
/// use octara::prelude::*;
/// let _ = el().value("text");
/// ```
#[must_use]
pub struct El<C = ElementContent> {
    props: Box<ElementProps>,
    marker: PhantomData<fn() -> C>,
}

#[track_caller]
fn element<C>(content: Content) -> El<C> {
    El { props: ElementProps::boxed(content, Location::caller()), marker: PhantomData }
}

#[track_caller]
pub fn el() -> El {
    element(Content::Node(NodeContent::Box))
}

#[track_caller]
pub fn row() -> El {
    el().direction(FlexDirection::Row)
}

#[track_caller]
pub fn col() -> El {
    el().direction(FlexDirection::Column)
}

#[track_caller]
pub fn spacer() -> El {
    el().grow(1.0)
}

#[track_caller]
pub fn text(value: impl IntoText) -> El {
    text_content(value.into_text())
}

#[inline(never)]
#[track_caller]
fn text_content(value: TextContent) -> El {
    element(Content::Text(value))
}

#[track_caller]
pub fn image(image: &Image) -> El {
    element(Content::Node(NodeContent::Image { image: image.clone(), fit: ObjectFit::default() }))
}

/// An image loaded when visible or in the viewport's prefetch band.
#[track_caller]
pub fn image_source(source: impl IntoProp<ImageSource>) -> El {
    element(Content::Media { source: source.into_prop(), fit: ObjectFit::default() })
}

/// A video played by the application's video module.
/// Without a video adapter, displays the poster when one is supplied.
#[track_caller]
pub fn video(source: VideoSource) -> El {
    element(Content::Video(source)).position(Position::Relative)
}

#[track_caller]
pub fn icon(icon: Icon) -> El {
    element(Content::Node(NodeContent::Icon(Arc::new(NodeIcon::new(&icon, None)))))
}

#[track_caller]
pub fn canvas(draw: impl FnMut(&mut ReadCx<'_>, &mut Canvas) + 'static) -> El {
    element(Content::Canvas(Box::new(draw)))
}

#[track_caller]
pub fn native_view(kind: &'static str) -> El {
    element(Content::Node(NodeContent::Native { kind, slot: NativeSlot::Overlay }))
}

/// A view backed by a factory registered by one of the application's modules.
#[track_caller]
pub fn typed_native_view(kind: &'static str, props: impl IntoProp<TypedNativeProps>) -> El {
    native_view(kind).typed_native_props(props)
}

#[track_caller]
pub fn input() -> TextInput {
    element(Content::Editor(EditorContent {
        slot: Box::default(),
        value: None,
        rich: None,
        backend: None,
        placeholder: None,
        secure: None,
        composing: None,
        editor_state: None,
    }))
    .focusable()
}

#[track_caller]
pub fn textarea() -> TextInput {
    let mut input = input();
    let editor = input.props.editor();
    editor.slot.multiline = true;
    editor.slot.rows = (2, 8);
    input
}

impl<C> Styled for El<C> {
    fn style(mut self, property: StyleProp) -> Self {
        self.props.push_style(StyleEntry::Static(property));
        self
    }

    fn push_styles(mut self, properties: &[StyleProp]) -> Self {
        self.props.push_styles(properties);
        self
    }
}

impl<C> IntoView for El<C> {
    fn into_view(self) -> View {
        View::element(El { props: self.props, marker: PhantomData })
    }

    fn with_component(mut self, component: Component) -> Self {
        self.props.set_component(component);
        self
    }
}

impl<C> From<El<C>> for View {
    fn from(el: El<C>) -> Self {
        el.into_view()
    }
}

macro_rules! string_properties {
    ($($name:ident),* $(,)?) => {$ (
        pub fn $name(self, value: impl IntoProp<Arc<str>>) -> Self {
            self.semantic_prop(value.into_prop(), |slot, value| slot.$name = Some(value))
        }
    )*};
}

macro_rules! bool_properties {
    ($($name:ident),* $(,)?) => {$ (
        pub fn $name(self, value: impl IntoProp<bool>) -> Self {
            self.semantic_prop(value.into_prop(), |slot, value| slot.$name = Some(value))
        }
    )*};
}

impl<C> El<C> {
    /// Appends a child or a tuple of up to twelve children in order.
    pub fn child(mut self, child: impl IntoView) -> Self {
        child.append_to(&mut self);
        self
    }

    #[inline(always)]
    pub(crate) fn push_child(&mut self, child: View) {
        self.props.push_child(child);
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoView>) -> Self {
        self.props.push_children(&mut children.into_iter().map(IntoView::into_view));
        self
    }

    pub fn test_id(mut self, id: impl IntoProp<Arc<str>>) -> Self {
        self.props.set_test_id(id.into_prop());
        self
    }

    /// Names this element for semantic relationships, focus and virtual reveal.
    pub fn id(mut self, id: impl IntoProp<Arc<str>>) -> Self {
        self.props.set_id(id.into_prop());
        self
    }

    pub fn when(mut self, condition: impl Into<When>, style: impl FnOnce(StylePatch) -> StylePatch) -> Self {
        self.props.push_override(condition.into(), style(StylePatch::default()));
        self
    }

    pub fn hover(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::State(State::HOVER), style);
        self
    }

    pub fn pressed(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::State(State::PRESSED), style);
        self
    }

    pub fn focus_visible(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::State(State::FOCUS_VISIBLE), style);
        self
    }

    pub fn focus_within(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::State(State::FOCUS_WITHIN), style);
        self
    }

    pub fn disabled_style(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::State(State::DISABLED), style);
        self
    }

    pub fn group_hover(mut self, name: &'static str, style: StyleOverride) -> Self {
        self.props.configure_override(When::GroupHover(name), style);
        self
    }

    pub fn dark(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::Dark, style);
        self
    }

    pub fn light(mut self, style: StyleOverride) -> Self {
        self.props.configure_override(When::Light, style);
        self
    }

    /// Applies the compile-time declarations from [`tw!`](crate::tw).
    pub fn tw(mut self, style: TwStyle) -> Self {
        self.props.push_tw(style);
        self
    }

    /// Applies reusable style properties in declaration order.
    pub fn style_patch(self, patch: &StylePatch) -> Self {
        self.push_styles(&patch.0)
    }

    /// Overrides selected theme values for this element and its descendants.
    /// A reactive patch updates the mounted subtree in place.
    pub fn theme_patch(mut self, patch: impl IntoProp<ThemePatch>) -> Self {
        self.props.set_theme_patch(patch.into_prop());
        self
    }

    /// Sets the base theme for this element and its descendants.
    pub fn theme_scope(self, theme: Theme) -> Self {
        self.theme_patch(ThemePatch::scope(theme))
    }

    pub fn style_with(mut self, mut style: impl FnMut(&mut ReadCx<'_>, StylePatch) -> StylePatch + 'static) -> Self {
        self.props.push_style(StyleEntry::Dynamic(Box::new(move |r| style(r, StylePatch::default()))));
        self
    }

    pub fn group(mut self, name: &'static str) -> Self {
        self.props.set_group(name);
        self
    }

    pub fn bg(self, value: impl IntoProp<Fill>) -> Self {
        self.style_prop(style_entry(value.into_prop(), |fill| StyleProp::Background(Some(fill))))
    }

    pub fn text_color(self, value: impl IntoProp<ColorValue>) -> Self {
        self.style_prop(style_entry(value.into_prop(), StyleProp::TextColor))
    }

    pub fn border_color(self, value: impl IntoProp<ColorValue>) -> Self {
        self.style_prop(style_entry(value.into_prop(), StyleProp::BorderColor))
    }

    pub fn opacity(self, value: impl IntoProp<f32>) -> Self {
        self.style_prop(style_entry(value.into_prop(), StyleProp::Opacity))
    }

    fn style_prop(mut self, entry: StyleEntry) -> Self {
        self.props.push_style(entry);
        self
    }

    pub fn paint(mut self, program: PaintProgram, input: impl IntoProp<PaintInput>) -> Self {
        self.props.set_paint((program, input.into_prop()));
        self
    }

    pub fn paint_transition(mut self, program: PaintProgram, key: impl IntoProp<u64>, curve: impl Into<Curve>) -> Self {
        self.props.set_transition_paint((program, key.into_prop(), curve.into()));
        self
    }

    pub fn role(self, role: Role) -> Self {
        self.semantic_prop(Prop::Static(role), |slot, role| slot.role = Some(role))
    }

    string_properties!(label, description, tooltip, a11y_value);
    bool_properties!(selected, required, invalid, expanded);

    pub fn expanded_state(self, expanded: impl IntoProp<Option<bool>>) -> Self {
        self.semantic_prop(expanded.into_prop(), |slot, expanded| slot.expanded = expanded)
    }

    pub fn multi_selectable(self, value: impl IntoProp<bool>) -> Self {
        self.semantic_prop(value.into_prop(), |slot, value| slot.multiselectable = Some(value))
    }

    pub fn checked(self, checked: impl IntoProp<Checked>) -> Self {
        self.semantic_prop(checked.into_prop(), |slot, checked| slot.checked = Some(checked))
    }

    /// Adds a validation condition to those already authored on the control.
    pub fn invalid_when(self, condition: impl FnMut(&mut ReadCx<'_>) -> bool + 'static) -> Self {
        self.semantic_prop(condition.into_prop(), |slot, invalid| {
            slot.invalid = Some(slot.invalid.unwrap_or(false) || invalid);
        })
    }

    pub fn described_by(self, id: impl IntoProp<Option<String>>) -> Self {
        self.semantic_prop(id.into_prop(), |slot, id| slot.relations.described_by = id.map(Into::into))
    }

    /// Supplies a description when no description has been authored.
    pub fn described_by_fallback(self, id: impl FnMut(&mut ReadCx<'_>) -> Option<String> + 'static) -> Self {
        self.semantic_prop(super::bind(id), |slot, id| {
            if slot.relations.described_by.is_none() {
                slot.relations.described_by = id.map(Into::into);
            }
        })
    }

    pub fn labelled_by(self, id: impl IntoProp<Option<String>>) -> Self {
        self.semantic_prop(id.into_prop(), |slot, id| slot.relations.labelled_by = id.map(Into::into))
    }

    pub fn controls(self, id: impl IntoProp<Option<String>>) -> Self {
        self.semantic_prop(id.into_prop(), |slot, id| slot.relations.controls = id.map(Into::into))
    }

    pub fn active_descendant(self, id: impl IntoProp<Option<String>>) -> Self {
        self.semantic_prop(id.into_prop(), |slot, id| slot.relations.active_descendant = id.map(Into::into))
    }

    pub fn numeric_range(self, range: impl IntoProp<NumericRange>) -> Self {
        self.semantic_prop(range.into_prop(), |slot, range| slot.numeric_range = Some(range))
    }

    pub fn cursor(self, cursor: CursorStyle) -> Self {
        self.semantic_prop(Prop::Static(cursor), |slot, cursor| slot.cursor = Some(cursor))
    }

    pub fn live(self, live: Live) -> Self {
        self.semantic_prop(Prop::Static(live), |slot, live| slot.live = Some(live))
    }

    pub fn level(self, level: impl IntoProp<u8>) -> Self {
        self.semantic_prop(level.into_prop(), |slot, level| slot.level = Some(level))
    }

    pub fn tab_index(self, index: i32) -> Self {
        self.semantic_prop(Prop::Static(index), |slot, index| slot.tab_index = index)
    }

    pub fn focus_group(self, group: FocusGroup) -> Self {
        self.semantic_prop(Prop::Static(group), |slot, group| slot.focus_group = Some(group))
    }

    /// Prefers this element as the Tab entry when it belongs to a focus group.
    pub fn focus_preferred(self, value: impl IntoProp<bool>) -> Self {
        self.semantic_prop(value.into_prop(), |slot, preferred| slot.focus_preferred = preferred)
    }

    /// Publishes the latest computed geometry to a signal owned by the current build scope.
    /// Call this again to publish the same geometry to additional signals.
    pub fn observe_geometry(mut self, signal: Signal<Option<LayoutBox>>) -> Self {
        self.props.push_geometry(signal);
        self
    }

    /// Publishes this element's current scroll offset to a signal.
    pub fn observe_scroll(mut self, signal: Signal<Vec2>) -> Self {
        self.props.set_scroll(signal);
        self
    }

    fn semantic_prop<T: 'static>(mut self, value: Prop<T>, apply: fn(&mut SemanticsSlot, T)) -> Self {
        set_semantic_prop(&mut self.props, value, apply);
        self
    }

    pub(crate) fn flags(mut self, flags: NodeFlags) -> Self {
        self.props.insert_flags(flags);
        self
    }

    pub fn focusable(self) -> Self {
        self.flags(NodeFlags::FOCUSABLE)
    }

    /// Lets unclaimed primary presses on this element move the host window.
    pub fn window_drag_region(mut self, enabled: bool) -> Self {
        self.props.set_flag(NodeFlags::WINDOW_DRAG_REGION, enabled);
        self
    }

    pub fn autofocus(self) -> Self {
        self.flags(NodeFlags::AUTOFOCUS)
    }

    /// Start a selection region, or exclude this element's subtree from text selection.
    pub fn selectable(mut self, enabled: bool) -> Self {
        self.props.set_selectable(enabled);
        self
    }

    pub fn select_none(self) -> Self {
        self.selectable(false)
    }

    /// Controls whether this element itself can be targeted by a pointer.
    pub fn pointer_events(self, enabled: bool) -> Self {
        self.style(StyleProp::PointerEvents(enabled))
    }

    /// Lets pointer input reach elements behind this one.
    pub fn pointer_events_none(self) -> Self {
        self.pointer_events(false)
    }

    /// Sets the paint-time reveal frontier on a text element.
    pub fn text_reveal(mut self, value: impl IntoProp<Option<octara_model::motion::TextReveal>>) -> Self {
        self.props.set_text_reveal(value.into_prop());
        self
    }

    /// Formats a selection inside this selectable element. Return `None` for its plain text.
    pub fn copy_selection_with(mut self, format: impl Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String> + 'static) -> Self {
        self.props.set_copy_selection(Box::new(format));
        self
    }

    pub fn a11y_hidden(self) -> Self {
        self.flags(NodeFlags::HIDDEN)
    }

    pub fn follow_tail(self) -> Self {
        self.flags(NodeFlags::FOLLOW_TAIL)
    }

    /// Keeps the first visible child in place when content above it changes.
    pub fn maintain_scroll_anchor(mut self, enabled: bool) -> Self {
        self.props.set_flag(NodeFlags::MAINTAIN_SCROLL_ANCHOR, enabled);
        self
    }

    /// Sets the visibility of this scrollport's overlay scrollbars.
    pub fn scrollbar(self, visibility: octara_model::style::ScrollbarVisibility) -> Self {
        self.style(StyleProp::Scrollbar(visibility))
    }

    pub fn preserve_focus_on_press(self) -> Self {
        self.flags(NodeFlags::PRESERVE_FOCUS)
    }

    pub fn disabled(mut self, value: impl IntoProp<bool>) -> Self {
        self.props.set_disabled(value.into_prop());
        self
    }

    /// Adds a disabling condition while preserving any condition already authored.
    pub fn disabled_when(mut self, value: impl IntoProp<bool>) -> Self {
        self.props.extend_disabled(value.into_prop());
        self
    }

    /// Gives this element pointer interaction states even without an activation callback.
    pub fn track_interaction(self) -> Self {
        self.flags(NodeFlags::INTERACTION_STATE)
    }

    pub fn behavior(mut self, configure: impl FnOnce(Behavior) -> Behavior) -> Self {
        self.props.extend_behavior(configure(Behavior::default()));
        self
    }

    /// Installs component behavior ahead of handlers already authored by the caller.
    pub fn behavior_first(mut self, configure: impl FnOnce(Behavior) -> Behavior) -> Self {
        self.props.prepend_behavior(configure(Behavior::default()));
        self
    }

    pub fn on(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) + 'static) -> Self {
        self.handler(Handler::Event(kind, Box::new(f)))
    }

    pub fn handle(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome + 'static) -> Self {
        self.handler(Handler::Raw(kind, Box::new(f)))
    }

    pub fn on_press(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::Press, Box::new(f)))
    }

    fn handler(mut self, handler: Handler) -> Self {
        self.props.behavior.push(handler);
        self
    }

    /// Intercepts rich-text link events. A consumed press suppresses default navigation.
    pub fn on_link(self, f: impl Fn(&mut WriteCx<'_>, &LinkEvent) -> bool + 'static) -> Self {
        self.handle(HandlerKind::Link, move |write, event| {
            let consumed = matches!(event, HandlerEvent::Link(link) if f(write, link));
            HandlerOutcome { consumed, ..Default::default() }
        })
    }

    /// Receives the viewport geometry and available actions for a context-menu request.
    pub fn on_context_menu(self, f: impl Fn(&mut WriteCx<'_>, &ContextMenuEvent) -> bool + 'static) -> Self {
        self.handle(HandlerKind::ContextMenu, move |write, event| {
            let consumed = matches!(event, HandlerEvent::ContextMenu(menu) if f(write, menu));
            HandlerOutcome { consumed, ..Default::default() }
        })
    }

    /// Registers a clipboard representation offered by the selection context menu.
    pub fn copy_format(mut self, kind: CopyFormat, format: impl Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String> + 'static) -> Self {
        self.props.copy_formats.push((kind, Box::new(format)));
        self
    }

    pub fn on_double_press(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::DoublePress, Box::new(f)))
    }

    pub fn on_secondary_press(self, f: impl Fn(&mut WriteCx<'_>, Point) + 'static) -> Self {
        self.handle(HandlerKind::SecondaryPress, move |write, event| {
            if let HandlerEvent::Interaction { payload: InteractionPayload::Press { pos, .. }, .. } = *event {
                f(write, pos);
                HandlerOutcome { consumed: true, ..Default::default() }
            } else {
                HandlerOutcome::default()
            }
        })
    }

    pub fn on_drag(self, f: impl Fn(&mut WriteCx<'_>, DragEvent) + 'static) -> Self {
        self.on(HandlerKind::Drag, move |write, event| {
            if let HandlerEvent::Interaction { payload: InteractionPayload::Drag(drag), .. } = *event {
                f(write, drag);
            }
        })
    }

    /// Starts a guarded element drag when the pointer exceeds its movement threshold.
    pub fn draggable(mut self, payload: DragPayload) -> Self {
        self.props.set_drag_source(payload);
        self
    }

    /// Accepts an element drag with one of the given kinds. The receiver's axis
    /// divides its bounds into before, inside, and after regions.
    pub fn drop_target(
        mut self,
        kinds: impl IntoIterator<Item = impl Into<Arc<str>>>,
        axis: DropAxis,
        on_drop: impl Fn(&mut WriteCx<'_>, &DropEvent) + 'static,
    ) -> Self {
        self.props.set_drop_target(DropTarget { kinds: kinds.into_iter().map(Into::into).collect(), axis });
        self.on(HandlerKind::Drop, move |write, event| {
            if let HandlerEvent::Interaction { payload: InteractionPayload::Drop { event, .. }, .. } = event {
                on_drop(write, event);
            }
        })
    }

    /// Reports entry, position changes, and departure during an element drag.
    pub fn on_drop_hover(self, on_hover: impl Fn(&mut WriteCx<'_>, &DropHover) + 'static) -> Self {
        self.on(HandlerKind::DropHover, move |write, event| {
            if let HandlerEvent::Interaction { payload: InteractionPayload::DropHover { event, .. }, .. } = event {
                on_hover(write, event);
            }
        })
    }

    /// Makes a draggable row focusable. Space or Enter grabs and drops it,
    /// arrows select a sibling receiver, and Escape cancels the operation.
    pub fn reorder_keys(mut self) -> Self {
        self.props.enable_keyboard_drag();
        self.focusable()
    }

    /// Accepts a file drop when this is the closest interactive receiver to the hit node.
    /// Native hosts supply local paths. Browsers supply display names.
    pub fn on_file_drop(self, f: impl Fn(&mut WriteCx<'_>, &[DroppedFile]) + 'static) -> Self {
        self.handler(Handler::FileDrop(Box::new(f)))
    }

    pub fn on_touch_scroll_pre(self, f: impl Fn(&mut WriteCx<'_>, Vec2) -> HandlerOutcome + 'static) -> Self {
        self.on_touch_scroll(HandlerKind::TouchScrollPre, f)
    }

    pub fn on_touch_scroll_post(self, f: impl Fn(&mut WriteCx<'_>, Vec2) -> HandlerOutcome + 'static) -> Self {
        self.on_touch_scroll(HandlerKind::TouchScrollPost, f)
    }

    fn on_touch_scroll(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, Vec2) -> HandlerOutcome + 'static) -> Self {
        self.handle(kind, move |write, event| {
            if let HandlerEvent::Interaction { payload: InteractionPayload::TouchScroll { available }, .. } = *event {
                f(write, available)
            } else {
                HandlerOutcome::default()
            }
        })
    }

    pub fn on_toggle(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::Toggle, Box::new(f)))
    }

    pub fn on_key(self, f: impl Fn(&mut WriteCx<'_>, &KeyEvent) -> bool + 'static) -> Self {
        self.handler(Handler::Key(Box::new(f)))
    }

    /// Receives paste gestures targeted at this element or a logical descendant.
    pub fn on_paste(self, f: impl Fn(&mut WriteCx<'_>, &PasteEvent) -> bool + 'static) -> Self {
        self.handler(Handler::Paste(Box::new(f)))
    }

    pub fn on_focus(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::Focus, Box::new(f)))
    }

    pub fn on_blur(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::Blur, Box::new(f)))
    }

    pub fn on_hover_change(self, f: impl Fn(&mut WriteCx<'_>, bool) + 'static) -> Self {
        self.handler(Handler::Hover(Box::new(f)))
    }

    pub fn on_command(mut self, name: impl Into<Arc<str>>, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.props.push_command((name.into(), Box::new(f)));
        self
    }

    pub fn key_context(mut self, name: impl Into<Arc<str>>) -> Self {
        self.props.set_key_context(name.into());
        self
    }

    pub fn layer(mut self, policy: LayerPolicy, dismiss: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.props.set_layer(policy);
        self.handler(Handler::Action(HandlerKind::Dismiss, Box::new(dismiss)))
    }

    /// Include this element and its owned content in a shared logical layer scope.
    pub fn layer_group(mut self, group: LayerGroupId) -> Self {
        self.props.set_layer_group(group);
        self
    }

    /// Opens content on the window's overlay layer, logically owned by this node.
    pub fn overlay<V: IntoView + 'static>(
        mut self,
        placement: Placement,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
    ) -> Self {
        self.props.push_portal(Portal::new(Some(placement), open, content));
        self
    }

    /// Mounts a caller-authored element as the anchored overlay root.
    pub fn overlay_part(
        mut self,
        placement: Placement,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> Self {
        self.props.push_portal(Portal::part(Some(placement), None, open, content));
        self
    }

    /// Positions the overlay relative to the target in window coordinates.
    /// An absent target uses the owning node.
    pub fn overlay_target(
        mut self,
        placement: Placement,
        target: impl IntoProp<Option<octara_model::overlay::AnchorTarget>>,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> Self {
        self.props.push_portal(Portal::part(Some(placement), Some(target.into_prop()), open, content));
        self
    }

    /// Mounts the actual root element in a portal, with its authored layout.
    pub fn portal_part(
        mut self,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> Self {
        self.props.push_portal(Portal::part(None, None, open, content));
        self
    }

    /// Opens viewport-sized content on the window's overlay layer.
    pub fn portal<V: IntoView + 'static>(
        mut self,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
    ) -> Self {
        self.props.push_portal(Portal::new(None, open, content));
        self
    }
}

impl El {
    pub fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        match &mut self.props.content {
            Content::Node(NodeContent::Image { fit, .. }) | Content::Media { fit, .. } => *fit = object_fit,
            _ => panic!("object_fit requires an image element"),
        }
        self
    }

    /// Publishes image load and header-probe state to a reactive signal.
    pub fn observe_image_state(mut self, signal: Signal<ImageState>) -> Self {
        self.props.image_state = Some(signal);
        self
    }

    pub fn native_slot(mut self, native_slot: NativeSlot) -> Self {
        let Content::Node(NodeContent::Native { slot, .. }) = &mut self.props.content else {
            panic!("native_slot requires a native view");
        };
        *slot = native_slot;
        self
    }

    pub fn native_props(mut self, props: impl IntoProp<serde_json::Value>) -> Self {
        self.props.set_native_props(NativeProps::Json(props.into_prop()));
        self
    }

    /// Supplies an in-process payload to a registered native view factory.
    pub fn typed_native_props(mut self, props: impl IntoProp<TypedNativeProps>) -> Self {
        self.props.set_native_props(NativeProps::Typed(props.into_prop()));
        self
    }

    pub fn on_native(self, f: impl Fn(&mut WriteCx<'_>, &NativeEvent) + 'static) -> Self {
        self.on(HandlerKind::Native, move |w, event| {
            if let HandlerEvent::Native(event) = event {
                f(w, event);
            }
        })
    }

    /// Handles events whose payload has the requested Rust type.
    pub fn on_typed_native<E: 'static>(self, f: impl Fn(&mut WriteCx<'_>, &'static str, &E) + 'static) -> Self {
        self.handle(HandlerKind::Native, move |w, event| {
            if let HandlerEvent::TypedNative(event) = event
                && let Some(payload) = event.downcast_ref::<E>()
            {
                f(w, event.name, payload);
                return HandlerOutcome { consumed: true, ..Default::default() };
            }
            HandlerOutcome::default()
        })
    }
}

impl TextInput {
    /// Publishes whether this editor currently has marked input-method text.
    pub fn observe_composition(mut self, signal: Signal<bool>) -> Self {
        self.props.editor().composing = Some(signal);
        self
    }

    /// Publishes the current focused, nonsecure editor's selection and revision.
    pub fn observe_editor(mut self, signal: Signal<Option<EditorState>>) -> Self {
        self.props.editor().editor_state = Some(signal);
        self
    }

    pub fn value(mut self, value: impl IntoProp<String>) -> Self {
        let value = value.into_prop();
        self.props.editor().set_value(value);
        self
    }

    /// Applies rich styling, inline objects and atomic token ranges to the editor value.
    pub fn rich_text(mut self, rich: impl IntoProp<EditorRichText>) -> Self {
        let rich = rich.into_prop();
        self.props.editor().set_rich(rich);
        self
    }

    /// Sets the fallback presentation backend for this input.
    /// A scoped `Arc<dyn EditorBackend>` can supply its own presentation.
    pub fn editor_backend(mut self, backend: Arc<dyn EditorBackend>) -> Self {
        self.props.editor().backend = Some(backend);
        self
    }

    /// Use the same model to publish editor text and accept edits.
    pub fn bind_value(self, binding: impl Into<Binding<String>>) -> Self {
        let binding = binding.into();
        self.value(binding).on_input(move |write, text| write.set(binding, text.to_owned()))
    }

    pub fn placeholder(mut self, value: impl IntoProp<Arc<str>>) -> Self {
        let value = value.into_prop();
        self.props.editor().set_placeholder(value);
        self
    }

    pub fn secure(mut self, secure: impl IntoProp<bool>) -> Self {
        let secure = secure.into_prop();
        self.props.editor().set_secure(secure);
        self
    }

    pub fn rows(mut self, min: u16, max: u16) -> Self {
        self.props.editor().slot.rows = (min.max(1), max.max(min).max(1));
        self
    }

    pub fn submit_on_enter(mut self) -> Self {
        self.props.editor().slot.submit_on_enter = true;
        self
    }

    pub fn on_input(self, f: impl Fn(&mut WriteCx<'_>, &str) + 'static) -> Self {
        self.handler(Handler::Input(Box::new(f)))
    }

    pub fn on_submit(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.handler(Handler::Action(HandlerKind::Submit, Box::new(f)))
    }
}

#[inline(never)]
fn style_entry<T: 'static>(prop: Prop<T>, make_style: fn(T) -> StyleProp) -> StyleEntry {
    match prop {
        Prop::Static(value) => StyleEntry::Static(make_style(value)),
        Prop::Dyn(mut read) => StyleEntry::Dynamic(Box::new(move |r| StylePatch::default().style(make_style(read(r))))),
    }
}

#[inline(never)]
fn set_semantic_prop<T: 'static>(props: &mut ElementProps, prop: Prop<T>, apply: fn(&mut SemanticsSlot, T)) {
    match prop {
        Prop::Static(value) => apply(props.static_semantics(), value),
        Prop::Dyn(mut read) => props.push_semantic_binding(Box::new(move |r, slot| apply(slot, read(r)))),
    }
}

fn mount_prop<T: 'static>(cx: &mut MountCx<'_>, node: NodeRef, value: Option<Prop<T>>, change: impl Fn(T) -> NodeChange + 'static) {
    match value {
        Some(Prop::Static(value)) => cx.change(node, change(value)),
        Some(Prop::Dyn(mut read)) => cx.bind(node, move |r| change(read(r))),
        None => {}
    }
}

fn mount_style(
    cx: &mut MountCx<'_>,
    node: NodeRef,
    styles: SmallVec<[StyleEntry; 8]>,
    overrides: SmallVec<[(When, Vec<StyleProp>); 2]>,
    group: Option<&'static str>,
) {
    let static_base = styles
        .iter()
        .filter_map(|entry| match entry {
            StyleEntry::Static(property) => Some(*property),
            StyleEntry::Dynamic(_) => None,
        })
        .collect::<SmallVec<_>>();
    let is_dynamic = styles.len() != static_base.len();
    if !static_base.is_empty() || !overrides.is_empty() || group.is_some() {
        let authored = AuthoredStyle { base: static_base, overrides: overrides.clone(), group };
        cx.change(node, NodeChange::Style(Box::new(authored)));
    }
    if is_dynamic {
        let mut styles = styles;
        cx.bind(node, move |r| {
            let mut base = SmallVec::new();
            for entry in &mut styles {
                match entry {
                    StyleEntry::Static(property) => base.push(*property),
                    StyleEntry::Dynamic(read) => base.extend(read(r).0),
                }
            }
            NodeChange::Style(Box::new(AuthoredStyle { base, overrides: overrides.clone(), group }))
        });
    }
}

fn mount_semantics(cx: &mut MountCx<'_>, node: NodeRef, semantics: Semantics) {
    let Semantics { authored, mut dynamic } = semantics;
    if dynamic.is_empty() {
        cx.change(node, NodeChange::Semantics(Box::new(authored)));
        return;
    }
    cx.change(node, NodeChange::Semantics(Box::new(authored.clone())));
    cx.bind(node, move |r| {
        let mut value = authored.clone();
        for apply in &mut dynamic {
            apply(r, &mut value);
        }
        NodeChange::Semantics(Box::new(value))
    });
}

type Paint<U> = Option<(PaintProgram, Prop<U>)>;

fn mount_paint(cx: &mut MountCx<'_>, node: NodeRef, custom: Paint<PaintInput>, transition: Option<(PaintProgram, Prop<u64>, Curve)>) {
    fn value<T: Clone>(value: &mut Prop<T>, read: Option<&mut ReadCx<'_>>) -> Option<T> {
        match value {
            Prop::Static(value) => Some(value.clone()),
            Prop::Dyn(f) => read.map(f),
        }
    }
    if custom.is_none() && transition.is_none() {
        return;
    }
    let mut custom = custom;
    let mut transition = transition;
    let dynamic = matches!(custom, Some((_, Prop::Dyn(_)))) || matches!(transition, Some((_, Prop::Dyn(_), _)));
    let (custom_mount, transition_mount) = (PaintMount::default(), PaintMount::default());
    let mut change = move |mut read: Option<&mut ReadCx<'_>>| {
        let custom = custom.as_mut().and_then(|(program, input)| {
            value(input, read.as_deref_mut()).map(|input| CustomPaintBinding {
                program: program.clone(),
                input,
                mount: custom_mount.clone(),
            })
        });
        let transition = transition.as_mut().and_then(|(program, key, curve)| {
            value(key, read).map(|key| TransitionBinding { program: program.clone(), curve: *curve, key, mount: transition_mount.clone() })
        });
        NodeChange::Paint(Box::new(PaintSlot { custom, transition }))
    };
    cx.change(node, change(None));
    if dynamic {
        cx.bind(node, move |r| change(Some(r)));
    }
}

pub(crate) fn build_el(
    cx: &mut MountCx<'_>,
    element: El,
    component: Option<Component>,
    row_key: Option<RowKey>,
    overlay: Option<OverlayBinding>,
) -> NodeRef {
    let El { mut props, .. } = element;
    if matches!(&props.content, Content::Node(NodeContent::Box))
        && props.children.is_empty()
        && props.custom_paint.is_none()
        && props.transition_paint.is_none()
        && props.semantics.as_ref().and_then(|value| value.authored.role) == Some(Role::CheckBox)
    {
        props.flags.insert(NodeFlags::NATIVE_CHECKBOX);
    }
    let component = props.component.or(component);
    let allocate = |cx: &mut MountCx<'_>, content| NodeRef::from(cx.create(NodeSpec { content, origin: props.origin, component, row_key }));
    let node = match props.content {
        Content::Node(content) => allocate(cx, content),
        Content::Media { source, fit } => {
            let resolver = cx.build(|build| build.context::<Arc<dyn MediaResolver>>());
            let resolve = move |source| resolve_image_source(source, resolver.as_deref());
            match source {
                Prop::Static(source) => allocate(cx, NodeContent::ImageSource { source: resolve(source), fit }),
                dynamic => {
                    let node = allocate(cx, NodeContent::ImageSource { source: ImageSource::Uri(Arc::from("")), fit });
                    mount_prop(cx, node, Some(dynamic), move |source| NodeChange::ImageSource { source: resolve(source), fit });
                    node
                }
            }
        }
        Content::Video(source) => {
            let adapter = cx.build(|build| build.context::<VideoAdapter>());
            if let Some(adapter) = adapter {
                props.native_props = Some(NativeProps::Typed(Prop::Static((adapter.props)(source))));
                allocate(cx, NodeContent::Native { kind: adapter.kind, slot: NativeSlot::Overlay })
            } else if let Some(poster) = source.poster {
                let resolver = cx.build(|build| build.context::<Arc<dyn MediaResolver>>());
                let source = resolve_image_source(ImageSource::Uri(poster.into()), resolver.as_deref());
                allocate(cx, NodeContent::ImageSource { source, fit: ObjectFit::Contain })
            } else {
                allocate(cx, NodeContent::Box)
            }
        }
        Content::Text(text) => {
            let node = allocate(cx, NodeContent::Text(Arc::from("")));
            match text {
                TextContent::Rich(value) => {
                    let resolver = cx.build(|build| build.context::<Arc<dyn MediaResolver>>());
                    mount_prop(cx, node, Some(value), move |text| text.change(resolver.as_deref()));
                }
                TextContent::Buffer(buffer) => mount_buffer(cx, node, buffer),
            }
            node
        }
        Content::Editor(EditorContent { slot, value, rich, backend, placeholder, mut secure, composing, editor_state }) => {
            let scoped = cx.build(|build| build.context::<Arc<dyn EditorBackend>>());
            let node = allocate(cx, NodeContent::TextInput(slot));
            match (value, rich) {
                (Some(mut value), mut rich) if scoped.is_some() || backend.is_some() => {
                    let mut secure = secure.take();
                    cx.bind_changes(node, move |read| {
                        let text = match &mut value {
                            Prop::Static(value) => value.clone(),
                            Prop::Dyn(value) => value(read),
                        };
                        let secure = secure.as_mut().map(|secure| match secure {
                            Prop::Static(secure) => *secure,
                            Prop::Dyn(secure) => secure(read),
                        });
                        let generated = if secure == Some(true) {
                            None
                        } else {
                            scoped
                                .as_ref()
                                .and_then(|backend| backend.rich_text(&text))
                                .or_else(|| backend.as_ref().and_then(|backend| backend.rich_text(&text)))
                        };
                        let presentation = generated
                            .or_else(|| match &mut rich {
                                Some(Prop::Static(rich)) => Some(rich.clone()),
                                Some(Prop::Dyn(rich)) => Some(rich(read)),
                                None => None,
                            })
                            .unwrap_or_default();
                        secure
                            .into_iter()
                            .map(NodeChange::EditorSecure)
                            .chain([NodeChange::EditorValue(text), NodeChange::EditorRichText(Box::new(presentation))])
                            .collect()
                    });
                }
                (Some(Prop::Dyn(mut value)), Some(Prop::Static(rich))) => {
                    let mut initial_rich = Some(rich);
                    cx.bind_changes(node, move |read| {
                        let mut changes = vec![NodeChange::EditorValue(value(read))];
                        if let Some(rich) = initial_rich.take() {
                            changes.push(NodeChange::EditorRichText(Box::new(rich)));
                        }
                        changes
                    });
                }
                (Some(Prop::Dyn(mut value)), Some(Prop::Dyn(mut rich))) => {
                    cx.bind_changes(node, move |read| {
                        vec![NodeChange::EditorValue(value(read)), NodeChange::EditorRichText(Box::new(rich(read)))]
                    });
                }
                (value, rich) => {
                    mount_prop(cx, node, value, NodeChange::EditorValue);
                    mount_prop(cx, node, rich, |rich| NodeChange::EditorRichText(Box::new(rich)));
                }
            }
            mount_prop(cx, node, placeholder, NodeChange::EditorPlaceholder);
            mount_prop(cx, node, secure, NodeChange::EditorSecure);
            if let Some(composing) = composing {
                cx.observe_composition(node, composing);
            }
            if let Some(editor_state) = editor_state {
                cx.observe_editor(node, editor_state);
            }
            node
        }
        Content::Canvas(mut draw) => {
            let node = allocate(cx, NodeContent::Canvas(Arc::from([])));
            let geometry = cx.build(|cx| cx.signal(None::<LayoutBox>));
            cx.observe_geometry(node, geometry);
            cx.bind(node, move |r| {
                let bounds = r.get(geometry).unwrap_or_default();
                let width =
                    (bounds.rect.w - bounds.padding.left - bounds.padding.right - bounds.border.left - bounds.border.right).max(0.0);
                let height =
                    (bounds.rect.h - bounds.padding.top - bounds.padding.bottom - bounds.border.top - bounds.border.bottom).max(0.0);
                let mut canvas = Canvas::new(width, height);
                draw(r, &mut canvas);
                NodeChange::Canvas(canvas.into_ops().into())
            });
            node
        }
    };

    mount_style(cx, node, props.styles, props.overrides, props.group);
    if let Some(patch) = props.theme_patch {
        mount_prop(cx, node, Some(*patch), |patch| NodeChange::ThemePatch(Some(Box::new(patch))));
    }
    for geometry in props.geometry {
        cx.observe_geometry(node, geometry);
    }
    if let Some(scroll) = props.scroll {
        cx.observe_scroll(node, scroll);
    }
    if let Some(format) = props.copy_selection {
        cx.on_copy_selection(node, format);
    }
    mount_prop(cx, node, props.id, |id| NodeChange::Id(Some(id)));
    mount_prop(cx, node, props.test_id, |id| NodeChange::TestId(Some(id)));
    mount_prop(cx, node, props.disabled, NodeChange::Disabled);
    match props.native_props {
        Some(NativeProps::Json(props)) => mount_prop(cx, node, Some(props), NodeChange::NativeProps),
        Some(NativeProps::Typed(props)) => mount_prop(cx, node, Some(props), NodeChange::TypedNativeProps),
        None => {}
    }
    if let Some(signal) = props.image_state {
        cx.observe_image_state(node, signal);
    }
    mount_paint(cx, node, props.custom_paint, props.transition_paint);
    if !props.flags.is_empty() {
        cx.change(node, NodeChange::Flags(props.flags));
    }
    if let Some(semantics) = props.semantics {
        mount_semantics(cx, node, *semantics);
    }
    if props.drag_drop.is_some() {
        cx.change(node, NodeChange::DragDrop(props.drag_drop));
    }
    for (kind, format) in props.copy_formats {
        cx.on_copy_format(node, Some(kind), format);
    }
    mount_prop(cx, node, props.text_reveal, NodeChange::TextReveal);
    if overlay.is_some() || props.layer.is_some() || props.layer_group.is_some() {
        let (mut spec, target) = overlay.map_or_else(|| (Default::default(), None), |binding| (binding.spec, binding.target));
        spec.layer = props.layer.or(spec.layer);
        spec.group = props.layer_group.or(spec.group);
        cx.change(node, NodeChange::Overlay(spec));
        if let Some(mut target) = target {
            cx.bind(node, move |read| target(read, spec));
        }
    }
    if let Some(context) = props.key_context {
        cx.key_context(node, context);
    }
    for (name, callback) in props.commands {
        cx.on_command(node, name, callback);
    }
    props.behavior.mount(cx, node);
    mount_children(cx, node, 0, props.children, component);
    if !props.portals.is_empty() {
        let owner = node_handle(cx, node);
        for portal in props.portals {
            portal.mount(cx, owner.clone(), component);
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_tuples_use_inline_child_storage() {
        let parent = el().child(((), Some((input(), "first")), (None::<El>, el(), ("second",))));
        assert_eq!(parent.props.children.len(), 4);
        assert!(!parent.props.children.spilled());
        assert!(parent.props.children.iter().all(|child| matches!(&child.kind, super::super::Kind::Element(_))));
    }
}
