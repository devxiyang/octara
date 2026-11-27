#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use octara::prelude::*;
    use octara_host_web::WebApp;
    use wasm_bindgen::prelude::*;
    use web_sys::HtmlElement;

    #[wasm_bindgen]
    pub struct WebInputFixture {
        app: WebApp,
    }

    #[wasm_bindgen]
    impl WebInputFixture {
        #[wasm_bindgen(constructor)]
        pub fn new(mount: HtmlElement) -> Result<Self, JsValue> {
            let app = WebApp::mount(mount, |cx| {
                let value = cx.signal("a😀b".to_owned());
                let presses = cx.signal(0);
                let intercepts = cx.signal(0);
                let shortcuts = cx.signal(0);
                let dropped = cx.signal(String::new());
                cx.on_chord("f2".parse().unwrap(), move |w| w.update(shortcuts, |count| *count += 1));
                cx.on_chord("tab".parse().unwrap(), move |w| w.update(shortcuts, |count| *count += 1));
                let content = col()
                    .child(
                        el().role(Role::Button)
                            .focusable()
                            .test_id("press")
                            .on_press(move |cx| {
                                let count = cx.get(presses);
                                cx.set(presses, count + 1);
                            })
                            .child(text("Press")),
                    )
                    .child(text(bind(move |cx| cx.get(presses).to_string())).test_id("press-count"))
                    .child(
                        input()
                            .test_id("editor")
                            .value(bind(move |cx| cx.get(value)))
                            .on_input(move |cx, text| {
                                if !text.contains('!') {
                                    cx.set(value, text.to_owned());
                                }
                            })
                            .behavior(move |behavior| {
                                behavior.on_key(move |cx, key| {
                                    if key.key != Key::Down {
                                        return false;
                                    }
                                    let count = cx.get(intercepts);
                                    cx.set(intercepts, count + 1);
                                    true
                                })
                            }),
                    )
                    .child(text(bind(move |cx| cx.get(intercepts).to_string())).test_id("key-count"))
                    .child(text(bind(move |cx| cx.get(shortcuts).to_string())).test_id("shortcut-count"))
                    .child(
                        el().test_id("drop-zone")
                            .on_file_drop(move |cx, files| {
                                let names = files
                                    .iter()
                                    .filter_map(|file| match file {
                                        DroppedFile::Name(name) => Some(name.as_str()),
                                        DroppedFile::Path(_) => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                cx.set(dropped, names);
                            })
                            .child(text("Drop files").test_id("drop-label")),
                    )
                    .child(text(bind(move |cx| cx.get(dropped))).test_id("dropped-files"));
                #[cfg(feature = "webgpu")]
                let content = content.child(gpu_canvas());
                content
            })?;
            Ok(Self { app })
        }

        pub fn value(&self) -> String {
            self.editor(|editor| editor.value().to_owned())
        }

        pub fn selection(&self) -> Vec<u32> {
            self.editor(|editor| {
                let (anchor, focus) = editor.selection();
                vec![anchor as u32, focus as u32]
            })
        }

        pub fn marked(&self) -> bool {
            self.editor(|editor| editor.composition().is_some())
        }

        pub fn viewport_width(&self) -> f32 {
            self.app.with_app(|app| app.window(self.app.window_id()).unwrap().runtime().options().viewport.size.width).unwrap()
        }
    }

    impl WebInputFixture {
        fn editor<R>(&self, read: impl FnOnce(&octara::tree::editor::EditorSlot) -> R) -> R {
            self.app
                .with_app(|app| {
                    let tree = app.window(self.app.window_id()).unwrap().runtime().tree();
                    read(tree.editor(tree.test_ids("editor")[0]).unwrap())
                })
                .unwrap()
        }
    }

    #[cfg(feature = "webgpu")]
    fn gpu_canvas() -> El {
        use octara::model::paint::PaintInput;
        use octara_render::gpu::{CanvasFrame, retained_canvas};
        use octara_render::wgpu;

        let program = retained_canvas(|_| {
            |frame: &mut CanvasFrame<'_>| {
                let target = [Some(wgpu::RenderPassColorAttachment {
                    view: frame.target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::GREEN), store: wgpu::StoreOp::Store },
                })];
                frame.begin_render_pass("fixture canvas", &wgpu::RenderPassDescriptor { color_attachments: &target, ..Default::default() });
            }
        });
        el().w(96).h(48).test_id("gpu-canvas").paint(program, PaintInput::default())
    }
}

fn main() {}
