//! The platform contract's conformance checks ([platform](../../../docs/design/platform.md)): one
//! set of scenarios every backend passes, the headless one in tests and each windowing backend
//! from its own loop (the shell: `OCTARA_CONFORMANCE=1`). They observe only what the app sees,
//! so they run unchanged on any backend. What needs a person or has effects outside the app
//! (opening a URL, a file dialog) is not among them.

use crate::app::{App, InputEvent};
use crate::platform::{ClipboardItem, CursorStyle, WindowOptions};
use crate::prelude::*;

/// One check's outcome.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Check {
    pub name: &'static str,
    /// Why it failed (`None`: passed).
    pub failure: Option<String>,
}

/// A scenario: it passes, or says why not.
type Scenario = fn(&App) -> Result<(), String>;

/// Runs every check on `app`, an app on the backend under test that can open windows now (a
/// windowing backend calls this from inside its loop). The clipboard is restored afterwards.
pub fn run(app: &App) -> Vec<Check> {
    let checks: [(&'static str, Scenario); 6] = [
        ("clipboard_text", clipboard_text),
        ("clipboard_image", clipboard_image),
        ("windows_open_and_close", windows_open_and_close),
        ("operations_present_frames", operations_present_frames),
        ("activation", activation),
        ("pointer_look", pointer_look),
    ];
    let saved = app.run(read_from_clipboard);
    let out = checks.iter().map(|(name, check)| Check { name, failure: check(app).err() }).collect();
    app.run(|| match saved {
        Some(item) => write_to_clipboard(item),
        None => write_to_clipboard(""),
    });
    out
}

fn ensure(ok: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why()) }
}

fn clipboard_text(app: &App) -> Result<(), String> {
    let text = "Octara conformance: 文字 ✓";
    app.run(|| write_to_clipboard(text));
    let back = app.run(read_from_clipboard);
    ensure(back.as_ref().and_then(ClipboardItem::text) == Some(text), || format!("wrote {text:?}, read {back:?}"))
}

fn clipboard_image(app: &App) -> Result<(), String> {
    let pixels = vec![255, 0, 0, 255, 0, 0, 255, 255];
    let image = crate::image::Image::rgba(2, 1, pixels.clone());
    app.run(|| write_to_clipboard(image));
    let back = app.run(read_from_clipboard).and_then(|i| i.image().cloned());
    let back = back.ok_or("no image read back")?;
    let f = back.frame();
    ensure(back.size() == (2, 1) && *f.rgba == *pixels, || format!("read a {:?} image, {:?}", back.size(), &f.rgba[..f.rgba.len().min(8)]))
}

fn windows_open_and_close(app: &App) -> Result<(), String> {
    let before = app.windows();
    let a = app.open_window(WindowOptions::new("Conformance A").size(240.0, 160.0), || text("A").into())?;
    let b = app.open_window(WindowOptions::new("Conformance B").size(240.0, 160.0), || text("B").into())?;
    let open = app.windows();
    ensure(open.contains(&a.id()) && open.contains(&b.id()) && open.len() == before.len() + 2, || format!("open: {open:?}"))?;
    ensure(b.options().title == "Conformance B", || format!("titled {:?}", b.options().title))?;
    app.close_window(a.id());
    ensure(!app.windows().contains(&a.id()) && app.windows().contains(&b.id()), || format!("after closing A: {:?}", app.windows()))?;
    app.close_window(b.id());
    ensure(app.windows() == before, || format!("after closing both: {:?}", app.windows()))
}

fn operations_present_frames(app: &App) -> Result<(), String> {
    let w = app.open_window(WindowOptions::new("Conformance frames").size(240.0, 160.0), || {
        let n = state("n", || 0);
        el().id("count").size_full().on_press(move || n.update(|n| *n += 1)).child(text(move || format!("{}", n.get()))).into()
    })?;
    let result = (|| {
        let mut w = w.clone();
        let start = w.frames_presented();
        ensure(start >= 1, || "opening presented no frame".into())?;
        w.tap("count")?;
        let after = w.frames_presented();
        ensure(after > start, || "a press that changed the text presented no frame".into())?;
        app.advance(std::time::Duration::from_millis(50));
        ensure(w.frames_presented() == after, || "an idle clock step presented a frame".into())
    })();
    app.close_window(w.id());
    result
}

fn activation(app: &App) -> Result<(), String> {
    let mut w = app.open_window(WindowOptions::new("Conformance activation").size(240.0, 160.0), || text_input().id("f").into())?;
    let result = (|| {
        w.tap("f")?;
        w.event(InputEvent::Active(false));
        ensure(!w.is_active(), || "still active after Active(false)".into())?;
        w.event(InputEvent::Active(true));
        ensure(w.is_active(), || "not active after Active(true)".into())
    })();
    app.close_window(w.id());
    result
}

fn pointer_look(app: &App) -> Result<(), String> {
    let mut w = app.open_window(WindowOptions::new("Conformance pointer").size(240.0, 160.0), || {
        el().id("hand").size_full().cursor_pointer().into()
    })?;
    let result = (|| {
        let (x, y, ww, h) = w.kernel().shown_rect(w.kernel().find("hand").ok_or("no element")?);
        w.pointer_move(x + ww / 2.0, y + h / 2.0);
        ensure(w.cursor() == CursorStyle::PointingHand, || format!("the pointer looks {:?}", w.cursor()))
    })();
    app.close_window(w.id());
    result
}
