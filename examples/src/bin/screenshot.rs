//! Reference capture with the native projection and CPU renderer.

use octara::{Picture, prelude::*};
use octara_examples::{apps, showcase, window_options};
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_render::CpuRenderer;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (flags, args): (Vec<String>, Vec<String>) = std::env::args().skip(1).partition(|arg| arg.starts_with("--"));
    let bundled = flags.iter().any(|flag| flag == "--bundled");
    let size = flags
        .iter()
        .find_map(|flag| flag.strip_prefix("--size="))
        .map(|size| {
            let (width, height) = size.split_once('x').ok_or("--size expects WIDTHxHEIGHT")?;
            Ok::<_, Box<dyn std::error::Error>>((width.parse::<f32>()?, height.parse::<f32>()?))
        })
        .transpose()?
        .unwrap_or((800.0, 600.0));
    let dir = args.first().map_or(Path::new("screenshots"), |arg| Path::new(arg));
    let scale = args.get(1).map(|arg| arg.parse::<f32>()).transpose()?.unwrap_or(2.0);
    if !scale.is_finite() || scale <= 0.0 {
        return Err("capture scale must be finite and positive".into());
    }
    std::fs::create_dir_all(dir)?;
    type Case = (&'static str, fn(&mut BuildCx<'_>) -> El);
    let cases: [Case; 8] = [
        ("showcase", showcase::showcase),
        ("workspace", |b| showcase::page(b, "workspace").expect("workspace card")),
        ("counter", apps::counter),
        ("todo", |b| apps::todo(b, 8)),
        ("chat", |b| apps::chat(b, 12)),
        ("settings", apps::settings),
        ("contacts", |b| apps::contacts(b, 20)),
        ("chat_virtual", |b| apps::chat_virtual(b, 1000)),
    ];
    let mut renderer = CpuRenderer::new();
    for (name, view) in cases {
        let mut app = App::new(move |_, _| {
            let mut engine = NativeEngine::new(NativeEngineConfig { system_fonts: !bundled, ..Default::default() });
            if bundled {
                for data in [octara_fonts::cjk::REGULAR, octara_fonts::cjk::SEMIBOLD] {
                    engine.register_font_data(data.into(), &octara_fonts::cjk::SCRIPTS);
                }
            }
            Box::new(engine)
        });
        let id = app.open_window(window_options(name, size.0, size.1), view)?;
        let frame = app
            .runtime()
            .window(id)
            .and_then(|window| window.last_commit())
            .and_then(|frame| frame.draw.as_deref())
            .ok_or("missing published drawing")?;
        let path = dir.join(format!("{name}.png"));
        write_png(&path, &renderer.render(frame, scale))?;
        println!("{}", path.display());
    }
    Ok(())
}

fn write_png(path: &Path, picture: &Picture) -> Result<(), Box<dyn std::error::Error>> {
    let mut png = png::Encoder::new(std::fs::File::create(path)?, picture.width(), picture.height());
    png.set_color(png::ColorType::Rgba);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()?.write_image_data(picture.rgba())?;
    Ok(())
}
