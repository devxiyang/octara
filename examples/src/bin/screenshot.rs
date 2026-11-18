//! Renders every example app with the reference renderer:
//! `screenshot <dir> [scale] [--bundled] [--size=WxH]` (default 800×600).
//!
//! Text uses the machine's fonts (as a window would), so CJK, emoji and other scripts show.
//! `--bundled` uses only the fonts that ship with Octara plus the CJK test subset: the same
//! pixels on every machine.

use octara::prelude::*;
use octara_examples::apps;

fn main() {
    let (flags, mut args): (Vec<String>, Vec<String>) = std::env::args().skip(1).partition(|a| a.starts_with("--"));
    let bundled = flags.iter().any(|f| f == "--bundled");
    let size = flags.iter().find_map(|f| f.strip_prefix("--size=")).map(|s| {
        let (w, h) = s.split_once('x').expect("--size=WxH");
        (w.parse::<f32>().expect("width"), h.parse::<f32>().expect("height"))
    });
    args.reverse();
    let dir = args.pop().unwrap_or_else(|| "screenshots".into());
    let scale: f32 = args.pop().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    std::fs::create_dir_all(&dir).expect("output directory");
    type Case = (&'static str, fn() -> View);
    let cases: [Case; 7] = [
        ("showcase", octara_examples::showcase::showcase),
        ("counter", || apps::counter().into()),
        ("todo", || apps::todo(8)),
        ("chat", || apps::chat(12)),
        ("settings", apps::settings),
        ("contacts", || apps::contacts(20)),
        ("chat_virtual", || apps::chat_virtual(1000)),
    ];
    for (name, view) in cases {
        let mut config = if bundled { AppConfig::default() } else { AppConfig::default().with_system_fonts() };
        if bundled {
            for data in [octara_fonts::cjk::REGULAR, octara_fonts::cjk::SEMIBOLD] {
                config.text_engine.register_fallback_font(data, &octara_fonts::cjk::SCRIPTS);
            }
        }
        let runtime = App::with_config(octara::platform::HeadlessPlatform::default(), config);
        let (w, h) = size.unwrap_or((800.0, 600.0));
        let app = runtime.open_window(octara::platform::WindowOptions::new(name).size(w, h), view).expect("headless window");
        let path = format!("{dir}/{name}.png");
        octara_render::CpuRenderer::new().render(&app.display_list(), scale).save_png(&path).expect("write png");
        println!("{path}");
    }
}
