//! A stock AppKit text view over the same directory row, for the IME placement comparison.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAccessibility, NSApplication, NSFont, NSResponder, NSTextInputClient, NSTextInputContext, NSTextView, NSView};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize, NSString};
use octara::prelude::*;
use octara::serde_json::{Value, json};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const WINDOW: &str = "Octara Rename Validation";
const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";
const MOVE: f64 = 44.0;

pub(super) fn install() {
    let Some(socket) = std::env::var_os("OCTARA_INSPECT") else { return };
    let report = PathBuf::from(socket).with_extension("native-control.json");
    let started = Cell::new(false);
    let move_requested = Rc::new(Cell::new(false));
    let movement = move_requested.clone();
    on_command("rename.move-native-control", move || movement.set(true));
    on_command("rename.native-control", move || {
        let id = current_window();
        let Some(window) = App::current().window(id) else { return };
        let snapshot = window.inspect();
        let Some(editor) = snapshot.find("rename-editor") else {
            write(&report, &json!({"error": "Start the Octara row editor before switching to AppKit."}));
            return;
        };
        if started.replace(true) {
            return;
        }
        let bounds = editor.bounds;
        focus("rename-outside");
        spawn(run(id, bounds, report.clone(), move_requested.clone()));
    });
}

async fn run(id: WindowId, bounds: [f32; 4], report: PathBuf, move_requested: Rc<Cell<bool>>) {
    let mut retired = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(16)).await;
        let Some(window) = App::current().window(id) else { return };
        let snapshot = window.inspect();
        if snapshot.focus.as_deref() == Some("rename-outside") && snapshot.text_input.is_none() {
            retired = true;
            break;
        }
    }
    if !retired {
        write(&report, &json!({"error": "The Octara editor did not release focus."}));
        return;
    }
    let Some(control) = Control::attach(bounds) else {
        write(&report, &json!({"error": "Could not focus the standard NSTextView in the fixture window."}));
        return;
    };
    let mut previous = Value::Null;
    let mut movement = Value::Null;
    loop {
        let Some(window) = control.view.window() else { return };
        let view = &*control.view;
        let parent = &*control.parent;
        let context = view.inputContext();
        let current = NSTextInputContext::currentInputContext(view.mtm());
        let active = window.isKeyWindow()
            && window.firstResponder().is_some_and(|responder| std::ptr::eq(&*responder, std::ptr::from_ref(view).cast::<NSResponder>()));
        if move_requested.replace(false) && active && movement.is_null() {
            movement = control.move_if_marked();
        }
        let position = parent.convertRect_toView(view.frame(), None);
        let marked = view.markedRange();
        let source = context.as_ref().and_then(|context| context.selectedKeyboardInputSource()).map(|id| id.to_string());
        let value = view.string();
        let preedit = (view.hasMarkedText() && marked.location <= value.length() && marked.length <= value.length() - marked.location)
            .then(|| value.substringWithRange(marked).to_string());
        let next = json!({
            "pid": std::process::id(), "window": window.windowNumber(), "native_focus": active,
            "input_context_current": context.as_ref().zip(current.as_ref()).is_some_and(|(a, b)| std::ptr::eq(&**a, &**b)),
            "source": source, "has_marked": view.hasMarkedText(), "marked_text": preedit,
            "marked_range_utf16": preedit.as_ref().map(|_| [marked.location, marked.length]),
            "selection_utf16": range(NSTextInputClient::selectedRange(view)), "text": value.to_string(),
            "view_screen_rect": rect(window.convertRectToScreen(position)), "movement": movement,
        });
        if next != previous {
            write(&report, &next);
            previous = next;
        }
        sleep(Duration::from_millis(80)).await;
    }
}

struct Control {
    view: Retained<NSTextView>,
    parent: Retained<NSView>,
}

impl Control {
    fn attach(bounds: [f32; 4]) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let window = NSApplication::sharedApplication(mtm).windows().iter().find(|window| window.title().to_string() == WINDOW)?;
        let parent = window.contentView()?;
        let [x, y, w, h] = bounds.map(f64::from);
        let origin = parent.bounds().origin;
        let y = if parent.isFlipped() { origin.y + y } else { origin.y + parent.bounds().size.height - y - h };
        let frame = NSRect::new(NSPoint::new(origin.x + x, y), NSSize::new(w, h));
        let view = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
        view.setString(&NSString::from_str("A😀Z"));
        view.setFont(Some(&NSFont::systemFontOfSize(16.0)));
        view.setTextContainerInset(NSSize::new(8.0, 2.0));
        view.setAccessibilityLabel(Some(&NSString::from_str("Native IME control")));
        view.setSelectedRange(NSRange::new(1, 2));
        parent.addSubview(&view);
        let control = Self { view, parent };
        window.makeFirstResponder(Some(&control.view)).then_some(control)
    }

    fn move_if_marked(&self) -> Value {
        let view = &*self.view;
        let Some(context) = view.inputContext() else { return Value::Null };
        if !view.hasMarkedText() || !context.selectedKeyboardInputSource().is_some_and(|source| source.to_string() == PINYIN) {
            return Value::Null;
        }
        let marked = view.markedRange();
        let selected = NSTextInputClient::selectedRange(view);
        // A null out pointer asks AppKit for the rect only.
        let before = unsafe { view.firstRectForCharacterRange_actualRange(selected, std::ptr::null_mut()) };
        let mut origin = view.frame().origin;
        origin.y += if self.parent.isFlipped() { -MOVE } else { MOVE };
        view.setFrameOrigin(origin);
        context.invalidateCharacterCoordinates();
        let after = unsafe { view.firstRectForCharacterRange_actualRange(selected, std::ptr::null_mut()) };
        json!({"up_points": MOVE, "first_rect_before": rect(before), "first_rect_after": rect(after),
               "marked_before": range(marked), "selection_before": range(selected)})
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

fn range(range: NSRange) -> [usize; 2] {
    [range.location, range.length]
}

fn rect(rect: NSRect) -> [f64; 4] {
    [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height]
}

fn write(path: &Path, value: &Value) {
    let tmp = path.with_extension("tmp");
    if let Err(error) = std::fs::write(&tmp, value.to_string()).and_then(|()| std::fs::rename(&tmp, path)) {
        eprintln!("native IME control observation: {error}");
    }
}
