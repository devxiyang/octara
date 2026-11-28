use crate::Driver;
use js_sys::{Array, Uint8Array};
use octara::{DragItem, Input, InteractionEvent};
use std::cell::Cell;
use wasm_bindgen::JsCast;
use web_sys::{DataTransfer, DragEvent, Event, File, FilePropertyBag};

pub(super) struct DragTransfer {
    data: DataTransfer,
    written: Cell<bool>,
}

impl Driver {
    pub(super) fn drag_out(&self, event: &Event) {
        let Some(data) = event.dyn_ref::<DragEvent>().and_then(DragEvent::data_transfer) else { return };
        if event.default_prevented() {
            return;
        }
        let transfer = DragTransfer { data, written: Cell::new(false) };
        let contact = self.pointer.get();
        let authored = self.cycle_with_drag(Some(&transfer), |app, window| {
            let Some(target) = self.contact(app, window, event) else { return Ok(false) };
            let tree = app.runtime().window(window).expect("browser window").tree();
            let Some(hit) = target.hit.node() else { return Ok(false) };
            if tree.drag_out_source(hit).is_some() {
                app.dispatch(window, Input::Interaction(InteractionEvent::DragStart { target: target.hit, contact }))?;
                Ok(true)
            } else {
                // In-app transfers already own this pointer.
                Ok(std::iter::successors(Some(hit), |&node| tree.logical_parent(node))
                    .any(|node| tree.drag_drop(node).is_some_and(|slot| slot.source.is_some())))
            }
        });
        match authored {
            Ok(true) if transfer.written.get() => {
                if let Some(id) = self.pointer.take() {
                    self.send(Input::Interaction(InteractionEvent::PointerCancel { id }));
                }
            }
            Ok(true) => event.prevent_default(),
            Err(error) => {
                event.prevent_default();
                self.report_error(error);
            }
            Ok(false) => {}
        }
    }
}

impl DragTransfer {
    pub(super) fn offer(&self, items: Vec<DragItem>) {
        let mut text = Vec::new();
        let mut written = false;
        for item in items {
            match item {
                DragItem::Text(value) => text.push(value),
                DragItem::Image { media_type, bytes } => written |= self.add_image(&media_type, &bytes),
                // A page cannot read an application-provided local path or publish it as a file.
                DragItem::File(_) => {}
            }
        }
        if !text.is_empty() {
            written |= self.data.set_data("text/plain", &text.join("\n")).is_ok();
        }
        if written {
            self.written.set(true);
        }
    }

    fn add_image(&self, media_type: &str, bytes: &[u8]) -> bool {
        if !media_type.starts_with("image/") || bytes.is_empty() {
            return false;
        }
        let name = match media_type {
            "image/png" => "image.png",
            "image/jpeg" => "image.jpg",
            "image/gif" => "image.gif",
            "image/webp" => "image.webp",
            _ => "image",
        };
        let parts = Array::new();
        parts.push(&Uint8Array::from(bytes));
        let options = FilePropertyBag::new();
        options.set_type(media_type);
        File::new_with_u8_array_sequence_and_options(&parts, name, &options)
            .and_then(|file| self.data.items().add_with_file(&file))
            .is_ok_and(|item| item.is_some())
    }
}
