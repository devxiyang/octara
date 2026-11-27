//! The retained browser nodes and the pieces of their authored projection.

use crate::css;
use crate::undo::{DomUndoLog, InputState};
use crate::{measure::MeasureMode, rich};
use js_sys::{BigInt, WeakMap};
use octara::tree::{NodeFlags, NodeId, NodeKind, Tree};
use octara_model::identity::{NativeId, TreeId};
use octara_model::input::ContactTarget;
use octara_model::semantics::{Checked, Live, Role};
use octara_model::style::State;
use octara_model::text::RichSpan;
use rustc_hash::{FxHashMap, FxHashSet};
use slotmap::KeyData;
use std::cell::{Cell, OnceCell, RefCell};
use std::fmt::Write;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, HtmlElement, ResizeObserver};

#[derive(Clone)]
pub(crate) struct DomHandle(Rc<DomStore>);

struct DomStore {
    entries: RefCell<FxHashMap<NodeId, Entry>>,
    ids: WeakMap,
    observer: OnceCell<ResizeObserver>,
    resized: RefCell<FxHashSet<NodeId>>,
    composing: Cell<Option<NodeId>>,
    projecting: Cell<bool>,
    mode: Cell<MeasureMode>,
    full_measure: Cell<bool>,
}

struct Entry {
    element: HtmlElement,
    action: Option<NodeId>,
}

impl Default for DomHandle {
    fn default() -> Self {
        Self(Rc::new(DomStore {
            entries: RefCell::default(),
            ids: WeakMap::new(),
            observer: OnceCell::new(),
            resized: RefCell::default(),
            composing: Cell::default(),
            projecting: Cell::default(),
            mode: Cell::default(),
            full_measure: Cell::default(),
        }))
    }
}

impl DomHandle {
    pub fn element(&self, node: NodeId) -> Option<HtmlElement> {
        self.0.entries.borrow().get(&node).map(|entry| entry.element.clone())
    }

    pub fn node(&self, element: &JsValue) -> Option<NodeId> {
        let key = element.dyn_ref::<Element>()?;
        let value = self.0.ids.get(key.as_ref());
        let id = NodeId::from(KeyData::from_ffi(u64::try_from(value).ok()?));
        self.0.entries.borrow().contains_key(&id).then_some(id)
    }

    pub fn target(&self, tree: &Tree, element: &JsValue) -> Option<ContactTarget> {
        let node = self.node(element).filter(|&node| tree.is_live(node))?;
        let action = self.0.entries.borrow().get(&node)?.action.filter(|&action| tree.is_live(action));
        Some(ContactTarget { hit: tree.guard(node), action: action.map(|action| tree.guard(action)) })
    }

    pub fn install_observer(&self, observer: ResizeObserver) {
        for entry in self.0.entries.borrow().values() {
            observer.observe(&entry.element);
        }
        assert!(self.0.observer.set(observer).is_ok(), "the DOM observer is installed once");
    }

    pub fn resize_nodes(&self, nodes: &[NodeId]) {
        self.0.resized.borrow_mut().extend(nodes.iter().copied());
    }

    pub fn take_resized(&self) -> FxHashSet<NodeId> {
        std::mem::take(&mut *self.0.resized.borrow_mut())
    }

    pub fn request_full_measure(&self) {
        self.0.full_measure.set(true);
    }

    pub fn take_full_measure(&self) -> bool {
        self.0.full_measure.take()
    }

    pub fn set_measure_mode(&self, mode: MeasureMode) {
        self.0.mode.set(mode);
    }

    pub fn measure_mode(&self) -> MeasureMode {
        self.0.mode.get()
    }

    pub fn set_composing(&self, node: Option<NodeId>) {
        self.0.composing.set(node);
    }

    pub fn composing(&self) -> Option<NodeId> {
        self.0.composing.get()
    }

    pub fn is_projecting(&self) -> bool {
        self.0.projecting.get()
    }

    pub fn project_guard(&self) -> impl Drop + use<> {
        self.0.projecting.set(true);
        struct Guard(DomHandle);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.0.projecting.set(false);
            }
        }
        Guard(self.clone())
    }

    pub fn register(&self, node: NodeId, element: HtmlElement) {
        self.map_node(node, &element);
        self.0.entries.borrow_mut().insert(node, Entry { element, action: None });
    }

    pub fn replace(&self, node: NodeId, element: HtmlElement) -> Option<HtmlElement> {
        let mut entries = self.0.entries.borrow_mut();
        let entry = entries.get_mut(&node)?;
        self.map_node(node, &element);
        let previous = std::mem::replace(&mut entry.element, element);
        self.0.ids.delete(previous.as_ref());
        if let Some(observer) = self.0.observer.get() {
            observer.unobserve(&previous);
            observer.observe(&entry.element);
        }
        Some(previous)
    }

    pub fn map_node(&self, node: NodeId, element: &Element) {
        self.0.ids.set(element.as_ref(), BigInt::from(NativeId::from(node).0).as_ref());
    }

    pub fn remove(&self, node: NodeId) {
        if let Some(entry) = self.0.entries.borrow_mut().remove(&node) {
            self.0.ids.delete(entry.element.as_ref());
            if let Some(observer) = self.0.observer.get() {
                observer.unobserve(&entry.element);
            }
        }
    }

    pub fn observe(&self, node: NodeId) {
        if let (Some(element), Some(observer)) = (self.element(node), self.0.observer.get()) {
            observer.observe(&element);
        }
    }

    pub fn set_action(&self, node: NodeId, action: Option<NodeId>) -> Option<Option<NodeId>> {
        let mut entries = self.0.entries.borrow_mut();
        let entry = entries.get_mut(&node)?;
        (entry.action != action).then(|| std::mem::replace(&mut entry.action, action))
    }
}

pub(crate) fn attribute(element: &Element, name: &str, value: Option<&str>) -> Result<(), JsValue> {
    match value {
        Some(value) => element.set_attribute(name, value),
        None => element.remove_attribute(name),
    }
}

pub(crate) fn create_element(doc: &Document, tree: &Tree, node: NodeId) -> Result<HtmlElement, JsValue> {
    let core = tree.node(node).expect("create a retained node");
    let checkbox = core.kind == NodeKind::Box && core.flags.contains(NodeFlags::NATIVE_CHECKBOX);
    let tag = match core.kind {
        NodeKind::TextInput => editor_tag(tree.editor(node).expect("mounted editor")),
        NodeKind::Image => "canvas",
        NodeKind::Native if tree.native(node).is_some_and(|slot| slot.kind == "video") => "video",
        NodeKind::Link => "a",
        NodeKind::Box if checkbox => "input",
        NodeKind::Box if tree.semantics(node).and_then(|slot| slot.role) == Some(Role::Button) || tree.has_activation_handler(node) => {
            "button"
        }
        _ => "div",
    };
    let element = html(doc, tag)?;
    element.set_attribute("id", &address(tree.id(), node))?;
    if core.kind == NodeKind::TextInput {
        element.set_attribute("data-octara-editor", "")?;
    }
    if checkbox {
        element.set_attribute("type", "checkbox")?;
    } else if tag == "button" {
        element.set_attribute("type", "button")?;
    }
    Ok(element)
}

pub(crate) fn html(doc: &Document, tag: &str) -> Result<HtmlElement, JsValue> {
    Ok(doc.create_element(tag)?.dyn_into()?)
}

pub(crate) fn editor_tag(editor: &octara::tree::editor::EditorSlot) -> &'static str {
    if editor.is_rich() {
        "div"
    } else if editor.multiline {
        "textarea"
    } else {
        "input"
    }
}

fn address(tree: TreeId, node: NodeId) -> String {
    let [high, low] = tree.namespace;
    format!("octara-{high:x}-{low:x}-{:x}-{:x}-{:x}", tree.window, tree.generation, NativeId::from(node).0)
}

pub(crate) fn action(tree: &Tree, node: NodeId, modal: Option<NodeId>) -> Option<NodeId> {
    std::iter::successors(Some(node), |&node| tree.interaction_parent(node)).find(|&node| {
        tree.node(node).is_some_and(|core| {
            matches!(core.kind, NodeKind::Link | NodeKind::TextInput)
                || core.flags.contains(NodeFlags::FOCUSABLE)
                || tree.has_activation_handler(node)
        }) && modal.is_none_or(|modal| tree.is_in_modal_scope(node, modal))
    })
}

pub(crate) fn update_semantics(
    undo: &mut DomUndoLog,
    element: &HtmlElement,
    tree: &Tree,
    node: NodeId,
    modal: Option<NodeId>,
    modal_ancestors: &FxHashSet<NodeId>,
) -> Result<(), JsValue> {
    let core = tree.node(node).expect("semantics for a retained node");
    let semantics = tree.semantics(node);
    let in_modal = modal.is_none_or(|modal| tree.is_in_modal_scope(node, modal));
    let inert = core.flags.contains(NodeFlags::INERT) || (!in_modal && !modal_ancestors.contains(&node));
    let expose = in_modal && !core.flags.intersects(NodeFlags::INERT | NodeFlags::HIDDEN);
    let disabled = core.state.contains(State::DISABLED);
    let role = semantics.and_then(|slot| slot.role).or(match core.kind {
        NodeKind::Image | NodeKind::Media => Some(Role::Image),
        NodeKind::Link => Some(Role::Link),
        _ if tree.has_activation_handler(node) => Some(Role::Button),
        _ => None,
    });
    macro_rules! set {
        ($name:expr, $value:expr) => {
            undo.attr(element, $name, $value)
        };
    }
    set!(
        "role",
        if expose {
            role.and_then(aria_role)
        } else if in_modal || inert {
            None
        } else {
            Some("presentation")
        }
    )?;
    set!("aria-label", semantics.filter(|_| expose).and_then(|slot| slot.label.as_deref()))?;
    set!("aria-description", semantics.filter(|_| expose).and_then(|slot| slot.description.as_deref()))?;
    set!("title", semantics.and_then(|slot| slot.tooltip.as_deref()))?;
    set!("aria-valuetext", semantics.filter(|_| expose).and_then(|slot| slot.a11y_value.as_deref()))?;
    set!("data-octara-id", tree.provenance(node).and_then(|source| source.test_id.as_deref()))?;
    set!("data-octara-action", tree.has_activation_handler(node).then_some(""))?;
    set!("data-octara-preserve-focus", core.flags.contains(NodeFlags::PRESERVE_FOCUS).then_some(""))?;
    set!("inert", inert.then_some(""))?;
    set!("aria-hidden", (inert || core.flags.contains(NodeFlags::HIDDEN)).then_some("true"))?;
    set!("aria-disabled", (expose && disabled).then_some("true"))?;
    set!("aria-modal", (modal == Some(node)).then_some("true"))?;
    if matches!(element.local_name().as_str(), "button" | "input" | "textarea") {
        set!("disabled", disabled.then_some(""))?;
    } else if core.kind == NodeKind::TextInput {
        set!("contenteditable", Some(if disabled || inert { "false" } else { "true" }))?;
        set!("aria-readonly", (disabled || inert).then_some("true"))?;
    }
    let index = if expose && !disabled && core.flags.contains(NodeFlags::FOCUSABLE) {
        let index = semantics.map_or(0, |slot| slot.tab_index);
        let roving = tree
            .focus_group_of(node)
            .and_then(|group| tree.semantics(group))
            .and_then(|slot| slot.group_current)
            .is_some_and(|current| current != node);
        Some(if roving { -1 } else { index })
    } else if (expose && !disabled && tree.is_layer_focus_container(node))
        || matches!(element.local_name().as_str(), "button" | "input" | "textarea" | "a")
    {
        Some(-1)
    } else {
        None
    };
    set!("tabindex", index.map(|index| index.to_string()).as_deref())?;
    let visible_semantics = semantics.filter(|_| expose);
    let orientation = visible_semantics
        .filter(|_| role == Some(Role::TabList))
        .and_then(|slot| slot.focus_group)
        .map(|group| if group.is_vertical() { "vertical" } else { "horizontal" });
    set!("aria-orientation", orientation)?;
    let level = match role {
        Some(Role::Heading) if expose => Some(visible_semantics.and_then(|slot| slot.level).unwrap_or(2)),
        Some(Role::TreeItem) => visible_semantics.and_then(|slot| slot.level),
        _ => None,
    }
    .map(|level| level.max(1).to_string());
    set!("aria-level", level.as_deref())?;
    let live = semantics.filter(|_| expose).and_then(|slot| slot.live).map(|live| match live {
        Live::Off => "off",
        Live::Polite => "polite",
        Live::Assertive => "assertive",
    });
    set!("aria-live", live)?;
    let selected = match role {
        Some(Role::Tab | Role::ListBoxOption) if expose => Some(core.state.contains(State::SELECTED)),
        Some(Role::TreeItem | Role::Row) => visible_semantics.and_then(|slot| slot.selected),
        _ => None,
    };
    let checked = visible_semantics.and_then(|slot| slot.checked);
    let checked_string = checked.map(|checked| match checked {
        Checked::Unchecked => "false",
        Checked::Checked => "true",
        Checked::Mixed => "mixed",
    });
    set!("aria-checked", checked_string.filter(|_| role != Some(Role::Button)))?;
    set!("aria-pressed", checked_string.filter(|_| role == Some(Role::Button)))?;
    if let Some(input) = element.dyn_ref::<web_sys::HtmlInputElement>().filter(|input| input.type_() == "checkbox") {
        undo.check(input, checked == Some(Checked::Checked), checked == Some(Checked::Mixed));
    }
    for (name, state) in [
        ("aria-expanded", visible_semantics.and_then(|slot| slot.expanded)),
        ("aria-multiselectable", visible_semantics.and_then(|slot| slot.multiselectable)),
        ("aria-required", visible_semantics.and_then(|slot| slot.required)),
        ("aria-invalid", visible_semantics.and_then(|slot| slot.invalid)),
        ("aria-selected", selected),
    ] {
        set!(name, state.map(|value| if value { "true" } else { "false" }))?;
    }
    let range = visible_semantics.and_then(|slot| slot.numeric_range);
    for (name, value) in [
        ("aria-valuemin", range.map(|range| range.min)),
        ("aria-valuemax", range.map(|range| range.max)),
        ("aria-valuenow", range.and_then(|range| range.value)),
    ] {
        set!(name, value.map(|value| value.to_string()).as_deref())?;
    }
    if let Some(link) = tree.link(node) {
        set!("href", link.navigation_href())?;
    }
    update_relations(undo, element, tree, node, modal)
}

pub(crate) fn update_relations(
    undo: &mut DomUndoLog,
    element: &HtmlElement,
    tree: &Tree,
    node: NodeId,
    modal: Option<NodeId>,
) -> Result<(), JsValue> {
    let exposed = |node| {
        tree.is_shown(node)
            && modal.is_none_or(|modal| tree.is_in_modal_scope(node, modal))
            && std::iter::successors(Some(node), |&node| tree.parent(node))
                .all(|node| tree.node(node).is_some_and(|core| !core.flags.intersects(NodeFlags::INERT | NodeFlags::HIDDEN)))
    };
    let relations = tree.semantics(node).filter(|_| exposed(node)).map(|slot| &slot.relations);
    for (name, target) in [
        ("aria-labelledby", relations.and_then(|r| r.labelled_by.as_deref())),
        ("aria-describedby", relations.and_then(|r| r.described_by.as_deref())),
        ("aria-controls", relations.and_then(|r| r.controls.as_deref())),
        ("aria-activedescendant", relations.and_then(|r| r.active_descendant.as_deref())),
    ] {
        let target = target.and_then(|name| {
            let [id] = tree.named_nodes(name) else { return None };
            exposed(*id).then_some(*id)
        });
        undo.attr(element, name, target.map(|id| address(tree.id(), id)).as_deref())?;
    }
    Ok(())
}

fn aria_role(role: Role) -> Option<&'static str> {
    Some(match role {
        Role::Window => "application",
        Role::GenericContainer | Role::Label | Role::PasswordInput => return None,
        Role::Heading => "heading",
        Role::Paragraph => "paragraph",
        Role::Blockquote => "blockquote",
        Role::Code => "code",
        Role::Button => "button",
        Role::CheckBox => "checkbox",
        Role::RadioButton => "radio",
        Role::RadioGroup => "radiogroup",
        Role::Switch => "switch",
        Role::TextInput | Role::MultilineTextInput => "textbox",
        Role::ComboBox | Role::EditableComboBox => "combobox",
        Role::List => "list",
        Role::ListItem => "listitem",
        Role::ListBox => "listbox",
        Role::ListBoxOption => "option",
        Role::Table => "table",
        Role::Row => "row",
        Role::Cell => "cell",
        Role::ColumnHeader => "columnheader",
        Role::Tree => "tree",
        Role::TreeItem => "treeitem",
        Role::Navigation => "navigation",
        Role::Separator => "separator",
        Role::Tooltip => "tooltip",
        Role::MenuItemCheckbox => "menuitemcheckbox",
        Role::MenuItemRadio => "menuitemradio",
        Role::Toolbar => "toolbar",
        Role::Menu => "menu",
        Role::MenuItem => "menuitem",
        Role::Tab => "tab",
        Role::TabList => "tablist",
        Role::TabPanel => "tabpanel",
        Role::Dialog => "dialog",
        Role::AlertDialog => "alertdialog",
        Role::Status => "status",
        Role::Alert => "alert",
        Role::Main => "main",
        Role::Group | Role::ScrollView => "group",
        Role::Document | Role::WebView => "document",
        Role::Link => "link",
        Role::Image => "img",
        Role::Slider => "slider",
        Role::SpinButton => "spinbutton",
        Role::ProgressIndicator => "progressbar",
        Role::Splitter => "separator",
    })
}

pub(crate) fn update_editor(
    undo: &mut DomUndoLog,
    element: &HtmlElement,
    tree: &Tree,
    node: NodeId,
    composing: bool,
) -> Result<(), JsValue> {
    let Some(editor) = tree.editor(node) else { return Ok(()) };
    let Some(before) = InputState::read(element) else { return Ok(()) };
    let masked = editor.is_secure() && !editor.revealed;
    let field_type = (!editor.multiline).then_some(if masked { "password" } else { "text" });
    let type_change = element.get_attribute("type").as_deref() != field_type;
    if type_change {
        undo.save_input(element, before.clone());
        undo.attr(element, "type", field_type)?;
    }
    undo.attr(element, "placeholder", Some(&editor.placeholder))?;
    undo.attr(element, "aria-multiline", editor.multiline.then_some("true"))?;
    let rows = editor.multiline.then(|| editor.rows.0.max(1).to_string());
    undo.attr(element, "rows", rows.as_deref())?;
    if composing && !type_change {
        return Ok(());
    }
    let (value, selection) = editor
        .composition()
        .map_or_else(|| (editor.value(), Some(editor.selection())), |composition| (composition.text(), composition.selection()));
    let selection = selection.map(|(anchor, caret)| {
        let offset = |byte| editor.displayed_rope().byte_to_utf16(byte).unwrap_or_default() as u32;
        (offset(anchor.min(caret)), offset(anchor.max(caret)), if anchor > caret { "backward" } else { "forward" }.to_owned())
    });
    let desired = InputState { value: value.to_owned(), selection };
    if desired.value != before.value
        || desired.selection.as_ref().is_some_and(|(start, end, _)| {
            before.selection.as_ref().is_none_or(|(a, b, direction)| {
                (start, end) != (a, b) || (start != end && desired.selection.as_ref().is_some_and(|(_, _, next)| direction != next))
            })
        })
    {
        if !type_change {
            undo.save_input(element, before);
        }
        desired.restore(element)?;
    } else if type_change {
        desired.restore(element)?;
    }
    Ok(())
}

pub(crate) fn update_rich_editor(
    undo: &mut DomUndoLog,
    element: &HtmlElement,
    tree: &Tree,
    node: NodeId,
    rebuild: bool,
) -> Result<(), JsValue> {
    let editor = tree.editor(node).expect("mounted editor");
    undo.attr(element, "aria-placeholder", (!editor.placeholder.is_empty()).then_some(&editor.placeholder))?;
    undo.attr(element, "aria-multiline", editor.multiline.then_some("true"))?;
    let (value, selection) = editor
        .composition()
        .map_or_else(|| (editor.value(), Some(editor.selection())), |composition| (composition.text(), composition.selection()));
    let active = element.owner_document().and_then(|doc| doc.active_element()).as_ref() == Some(element.as_ref());
    if rebuild || rich::logical_text(element) != value {
        if active {
            undo.save_rich_selection(element);
        }
        while let Some(child) = element.first_child() {
            undo.place(&child, None, None)?;
        }
        let doc = element.owner_document().expect("editor document");
        let theme = tree.theme_at(node);
        let visual = tree.visual(node);
        let decorations = editor.decorations();
        let atoms = editor.atoms();
        let attachments = editor.attachments();
        let ranges = decorations.iter().map(|(range, _)| range).chain(atoms.iter().map(|atom| &atom.range));
        let mut atom_element = None;
        for (range, attachment) in rich::pieces(value, ranges, attachments) {
            let atom = atoms.iter().position(|atom| atom.range.contains(&range.start));
            if atom != atom_element.as_ref().map(|(index, _)| *index) {
                atom_element = match atom {
                    Some(index) => {
                        let atom = html(&doc, "span")?;
                        atom.set_attribute("contenteditable", "false")?;
                        undo.place(&atom, Some(element), None)?;
                        Some((index, atom))
                    }
                    None => None,
                };
            }
            let parent = atom_element.as_ref().map_or(element, |(_, atom)| atom);
            let mut declarations = String::new();
            for (_, span) in decorations.iter().filter(|(style, _)| style.contains(&range.start)) {
                rich_style(&mut declarations, *span, theme);
            }
            let value = &value[range];
            let child: web_sys::Node = if let Some((index, attachment)) = attachment {
                rich::attachment(&doc, index, attachment, value, visual, theme)?.into()
            } else {
                doc.create_text_node(value).into()
            };
            if declarations.is_empty() {
                undo.place(&child, Some(parent), None)?;
            } else {
                let span = html(&doc, "span")?;
                span.set_attribute("style", &declarations)?;
                span.append_child(&child)?;
                undo.place(&span, Some(parent), None)?;
            }
        }
        if value.is_empty() && attachments.is_empty() && !editor.placeholder.is_empty() {
            let placeholder = rich::placeholder(&doc, &editor.placeholder, theme)?;
            undo.place(&placeholder, Some(element), None)?;
        }
    }
    if active
        && let Some(selection) = selection
        && rich::selection(element) != Some(selection)
    {
        undo.save_rich_selection(element);
        rich::select(element, selection)?;
    }
    Ok(())
}

pub(crate) fn update_text(
    dom: &DomHandle,
    undo: &mut DomUndoLog,
    doc: &Document,
    element: &HtmlElement,
    tree: &Tree,
    node: NodeId,
) -> Result<(), JsValue> {
    let text = tree.text(node).expect("text node");
    let mut child = element.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        if !current.dyn_ref::<Element>().is_some_and(|element| element.has_attribute("data-octara-gpu-layer")) {
            undo.place(&current, None, None)?;
        }
    }
    for &link in &text.links {
        if let Some(element) = dom.element(link) {
            while let Some(child) = element.first_child() {
                undo.place(&child, None, None)?;
            }
        }
    }
    let ranges = text.runs.iter().map(|(range, _)| range).chain(text.links.iter().filter_map(|&id| tree.link(id).map(|link| &link.range)));
    let theme = tree.theme_at(node);
    let visual = tree.visual(node);
    for (range, attachment) in rich::pieces(&text.text, ranges, &text.attachments) {
        let start = range.start;
        let value = &text.text[range.clone()];
        let link = text.links.iter().copied().find(|&link| tree.link(link).is_some_and(|link| link.range.contains(&start)));
        let link_element = link.and_then(|link| dom.element(link));
        let parent = link_element.as_ref().unwrap_or(element);
        if let Some(link) = &link_element
            && link.parent_element().as_ref() != Some(element.as_ref())
        {
            undo.place(link, Some(element), None)?;
        }
        let mut declarations = String::new();
        for (_, span) in text.runs.iter().filter(|(style_range, _)| style_range.contains(&start)) {
            rich_style(&mut declarations, *span, theme);
        }
        let text_node: web_sys::Node = if let Some((index, attachment)) = attachment {
            rich::attachment(doc, index, attachment, value, visual, theme)?.into()
        } else {
            doc.create_text_node(value).into()
        };
        if declarations.is_empty() {
            undo.place(&text_node, Some(parent), None)?;
        } else {
            let span = html(doc, "span")?;
            span.set_attribute("style", &declarations)?;
            dom.map_node(link.unwrap_or(node), &span);
            span.append_child(&text_node)?;
            undo.place(&span, Some(parent), None)?;
        }
    }
    Ok(())
}

fn rich_style(style: &mut String, span: RichSpan, theme: &octara_model::theme::Theme) {
    if let Some(weight) = span.weight {
        let _ = write!(style, "font-weight:{weight};");
    }
    if span.italic {
        style.push_str("font-style:italic;");
    }
    if span.mono {
        let _ = write!(style, "font-family:{};", css::font_family(theme.font_mono));
    }
    if let Some(size) = span.size {
        let _ = write!(style, "font-size:{size}px;");
    }
    if let Some(color) = span.color {
        let _ = write!(style, "color:{};", css::color(color.resolve(theme).0));
    }
    if let Some(color) = span.background {
        let _ = write!(style, "background-color:{};", css::color(color.resolve(theme).0));
    }
    if let Some(radius) = span.radius.filter(|radius| radius.is_finite() && *radius >= 0.0) {
        let _ = write!(style, "border-radius:{radius}px;");
    }
    if let Some(padding) = span.padding_x.filter(|padding| padding.is_finite() && *padding >= 0.0) {
        let _ = write!(style, "padding-inline:{padding}px;");
    }
    if let Some(padding) = span.padding_y.filter(|padding| padding.is_finite() && *padding >= 0.0) {
        let _ = write!(style, "padding-block:{padding}px;");
    }
    if let Some(border) = span.border_color {
        let _ = write!(style, "border:1px solid {};", css::color(border.resolve(theme).0));
    }
    if span.background.is_some() || span.border_color.is_some() || span.padding_x.is_some() || span.padding_y.is_some() {
        style.push_str("-webkit-box-decoration-break:clone;box-decoration-break:clone;");
    }
    css::text_decoration(style, span.underline, span.strike);
}

pub(crate) fn icon(doc: &Document, icon: &octara_model::paint::NodeIcon) -> Result<Element, JsValue> {
    use octara_model::paint::{FillRule, PathStyle};

    const SVG: &str = "http://www.w3.org/2000/svg";
    let svg = doc.create_element_ns(Some(SVG), "svg")?;
    let view = icon.view_box;
    svg.set_attribute("viewBox", &format!("{} {} {} {}", view.x, view.y, view.w, view.h))?;
    svg.set_attribute("width", &view.w.to_string())?;
    svg.set_attribute("height", &view.h.to_string())?;
    svg.set_attribute("aria-hidden", "true")?;
    svg.set_attribute("focusable", "false")?;
    svg.set_attribute("style", "display:block;width:100%;height:100%;pointer-events:none;")?;
    for shape in &icon.shapes {
        let element = doc.create_element_ns(Some(SVG), "path")?;
        element.set_attribute("d", &svg_path(&shape.path))?;
        match &shape.style {
            PathStyle::Fill(rule) => {
                element.set_attribute("fill", "currentColor")?;
                if *rule == FillRule::EvenOdd {
                    element.set_attribute("fill-rule", "evenodd")?;
                }
            }
            PathStyle::Stroke(stroke) => {
                svg_stroke(&element, stroke, "currentColor")?;
            }
        }
        svg.append_child(&element)?;
    }
    Ok(svg)
}

pub(crate) fn svg_stroke(element: &Element, stroke: &octara_model::paint::Stroke, color: &str) -> Result<(), JsValue> {
    use octara_model::paint::{LineCap, LineJoin};

    element.set_attribute("fill", "none")?;
    element.set_attribute("stroke", color)?;
    element.set_attribute("stroke-width", &stroke.width.to_string())?;
    element.set_attribute("stroke-miterlimit", &stroke.miter_limit.to_string())?;
    element.set_attribute(
        "stroke-linecap",
        match stroke.cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        },
    )?;
    element.set_attribute(
        "stroke-linejoin",
        match stroke.join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        },
    )?;
    if !stroke.dash.is_empty() {
        let dash = stroke.dash.iter().map(f32::to_string).collect::<Vec<_>>().join(" ");
        element.set_attribute("stroke-dasharray", &dash)?;
        element.set_attribute("stroke-dashoffset", &stroke.dash_offset.to_string())?;
    }
    Ok(())
}

pub(crate) fn svg_path(path: &octara_model::paint::Path) -> String {
    use octara_model::paint::PathCmd;

    let mut data = String::new();
    for command in path.commands() {
        let _ = match command {
            PathCmd::MoveTo(point) => write!(data, "M{} {}", point.x, point.y),
            PathCmd::LineTo(point) => write!(data, "L{} {}", point.x, point.y),
            PathCmd::QuadTo(a, b) => write!(data, "Q{} {} {} {}", a.x, a.y, b.x, b.y),
            PathCmd::CurveTo(a, b, c) => write!(data, "C{} {} {} {} {} {}", a.x, a.y, b.x, b.y, c.x, c.y),
            PathCmd::Close => write!(data, "Z"),
        };
    }
    data
}
