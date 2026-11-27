//! Inverses for mutations made during one or more speculative rounds.

use crate::{dom, image, rich};
use js_sys::{Function, Reflect};
use octara_model::geometry::Vec2;
use octara_model::identity::NodeId;
use octara_model::paint::Sample;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, FocusOptions, HtmlCanvasElement, HtmlElement, HtmlInputElement, HtmlTextAreaElement, Node};

#[derive(Default)]
pub(crate) struct DomUndoLog(Vec<Undo>);

enum Undo {
    Position { node: Node, parent: Option<Node>, next: Option<Node> },
    Attr { element: Element, name: String, value: Option<String> },
    Check { input: HtmlInputElement, checked: bool, mixed: bool },
    Input { element: HtmlElement, state: InputState },
    RichSelection { element: HtmlElement, endpoints: (usize, usize) },
    Element { dom: dom::DomHandle, node: NodeId, previous: HtmlElement },
    Scroll { element: HtmlElement, offset: Vec2 },
    Image { canvas: HtmlCanvasElement, previous: Option<Sample> },
}

impl DomUndoLog {
    pub fn place(&mut self, node: &Node, parent: Option<&Node>, next: Option<&Node>) -> Result<bool, JsValue> {
        let previous = node.parent_node();
        let previous_next = node.next_sibling();
        if previous.as_ref() == parent && previous_next.as_ref() == next {
            return Ok(false);
        }
        let undo = Undo::Position { node: node.clone(), parent: previous, next: previous_next };
        place(node, parent, next)?;
        self.0.push(undo);
        Ok(true)
    }

    pub fn attr(&mut self, element: &Element, name: &str, value: Option<&str>) -> Result<(), JsValue> {
        let previous = element.get_attribute(name);
        if previous.as_deref() != value {
            self.0.push(Undo::Attr { element: element.clone(), name: name.into(), value: previous });
            dom::attribute(element, name, value)?;
        }
        Ok(())
    }

    pub fn check(&mut self, input: &HtmlInputElement, checked: bool, mixed: bool) {
        let previous = (input.checked(), input.indeterminate());
        if previous != (checked, mixed) {
            self.0.push(Undo::Check { input: input.clone(), checked: previous.0, mixed: previous.1 });
            input.set_checked(checked);
            input.set_indeterminate(mixed);
        }
    }

    pub fn save_input(&mut self, element: &HtmlElement, state: InputState) {
        self.save_scroll(element);
        self.0.push(Undo::Input { element: element.clone(), state });
    }

    pub fn save_rich_selection(&mut self, element: &HtmlElement) {
        if let Some(endpoints) = rich::selection(element) {
            self.save_scroll(element);
            self.0.push(Undo::RichSelection { element: element.clone(), endpoints });
        }
    }

    pub fn replace_element(&mut self, dom: &dom::DomHandle, node: NodeId, element: HtmlElement) {
        if let Some(previous) = dom.replace(node, element) {
            self.0.push(Undo::Element { dom: dom.clone(), node, previous });
        }
    }

    pub fn save_scroll(&mut self, element: &HtmlElement) {
        self.0.push(Undo::Scroll { element: element.clone(), offset: scroll_offset(element) });
    }

    pub fn snapshot_scroll(&mut self, element: &HtmlElement, offset: Vec2) {
        self.0.push(Undo::Scroll { element: element.clone(), offset });
    }

    pub fn save_image(&mut self, canvas: &HtmlCanvasElement, previous: Option<Sample>) {
        self.0.push(Undo::Image { canvas: canvas.clone(), previous });
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn rollback(&mut self) {
        let mut scroll = Vec::new();
        for op in self.0.drain(..).rev() {
            let result = match op {
                Undo::Position { node, parent, next } => place(&node, parent.as_ref(), next.as_ref()),
                Undo::Attr { element, name, value } => dom::attribute(&element, &name, value.as_deref()),
                Undo::Check { input, checked, mixed } => {
                    input.set_checked(checked);
                    input.set_indeterminate(mixed);
                    Ok(())
                }
                Undo::Input { element, state } => state.restore(&element),
                Undo::RichSelection { element, endpoints } => rich::select(&element, endpoints),
                Undo::Element { dom, node, previous } => {
                    dom.replace(node, previous);
                    Ok(())
                }
                Undo::Scroll { element, offset } => {
                    // Restore offsets only after undoing the layout that can constrain them.
                    scroll.push((element, offset));
                    Ok(())
                }
                Undo::Image { canvas, previous } => image::upload(&canvas, previous.as_ref()),
            };
            if let Err(error) = result {
                web_sys::console::error_1(&error);
            }
        }
        for (element, offset) in scroll {
            scroll_to(&element, offset);
        }
    }
}

pub(crate) fn place(node: &Node, parent: Option<&Node>, next: Option<&Node>) -> Result<(), JsValue> {
    if let Some(parent) = parent {
        if node.is_connected()
            && parent.is_connected()
            && let Ok(method) = Reflect::get(parent, &JsValue::from_str("moveBefore"))
            && let Ok(method) = method.dyn_into::<Function>()
            && method.call2(parent, node, next.map_or(&JsValue::NULL, AsRef::as_ref)).is_ok()
        {
            return Ok(());
        }
        parent.insert_before(node, next)?;
    } else if let Some(parent) = node.parent_node() {
        parent.remove_child(node)?;
    }
    Ok(())
}

pub(crate) fn scroll_offset(element: &HtmlElement) -> Vec2 {
    let read =
        |property| Reflect::get(element, &JsValue::from_str(property)).ok().and_then(|value| value.as_f64()).unwrap_or_default() as f32;
    Vec2::new(read("scrollLeft"), read("scrollTop"))
}

pub(crate) fn scroll_to(element: &HtmlElement, offset: Vec2) {
    let options = web_sys::ScrollToOptions::new();
    options.set_left(offset.x.into());
    options.set_top(offset.y.into());
    options.set_behavior(web_sys::ScrollBehavior::Instant);
    element.scroll_to_with_scroll_to_options(&options);
}

pub(crate) fn focus(element: &HtmlElement) -> Result<(), JsValue> {
    let options = FocusOptions::new();
    options.set_prevent_scroll(true);
    element.focus_with_options(&options)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputState {
    pub value: String,
    pub selection: Option<(u32, u32, String)>,
}

impl InputState {
    pub fn read(element: &HtmlElement) -> Option<Self> {
        macro_rules! read {
            ($field:expr) => {{
                let field = $field;
                let selection =
                    (|| Some((field.selection_start().ok()??, field.selection_end().ok()??, field.selection_direction().ok()??)))();
                Some(Self { value: field.value(), selection })
            }};
        }
        if let Some(input) = element.dyn_ref::<HtmlInputElement>() {
            read!(input)
        } else {
            read!(element.dyn_ref::<HtmlTextAreaElement>()?)
        }
    }

    pub fn restore(&self, element: &HtmlElement) -> Result<(), JsValue> {
        let Some(current) = Self::read(element) else { return Ok(()) };
        macro_rules! restore {
            ($field:expr) => {{
                let field = $field;
                let value_changed = current.value != self.value;
                if value_changed {
                    field.set_value(&self.value);
                }
                if let Some((start, end, direction)) = &self.selection
                    && (value_changed || current.selection != self.selection)
                {
                    field.set_selection_range_with_direction(*start, *end, direction)?;
                }
            }};
        }
        if let Some(input) = element.dyn_ref::<HtmlInputElement>() {
            restore!(input);
        } else if let Some(textarea) = element.dyn_ref::<HtmlTextAreaElement>() {
            restore!(textarea);
        }
        Ok(())
    }
}
