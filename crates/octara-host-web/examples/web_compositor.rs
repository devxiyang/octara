//! Browser fixture for 2.5D transforms and external canvas composition.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use octara::prelude::*;
    use octara::runtime::engine::HostEngine;
    use octara::tree::tables::AuthoredStyle;
    use octara::tree::{NodeChange, NodeContent, NodeSpec, Retirement, RuntimeOp, Tree, TreeId};
    use octara_host_web::{DomEngine, WebApp, WebCanvas};
    use octara_model::geometry::{Size, WindowViewport};
    use octara_model::image::{ExternalTextureHandle, ImageState, MediaAsset};
    use octara_model::motion::Curve;
    use octara_model::style::LayoutLength;
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
    use web_sys::{HtmlCanvasElement, HtmlElement};

    #[wasm_bindgen]
    pub struct CompositorFixture {
        app: Option<WebApp>,
        registration: Option<WebCanvas>,
        visible: Signal<bool>,
        angle: Signal<Option<f32>>,
        popup: Signal<bool>,
    }

    #[wasm_bindgen]
    impl CompositorFixture {
        pub fn show_canvas(&self, visible: bool) {
            self.app.as_ref().unwrap().update(|write| write.set(self.visible, visible)).unwrap();
        }

        pub fn rotate_card(&self, angle: Option<f32>) {
            self.app.as_ref().unwrap().update(|write| write.set(self.angle, angle)).unwrap();
        }

        pub fn show_popup(&self, visible: bool) {
            self.app.as_ref().unwrap().update(|write| write.set(self.popup, visible)).unwrap();
        }

        pub fn unmount(&mut self) {
            self.app = None;
        }

        pub fn unregister(&mut self) {
            self.registration = None;
        }
    }

    #[wasm_bindgen]
    pub fn mount_compositor(surface: HtmlElement, source: HtmlCanvasElement) -> Result<CompositorFixture, JsValue> {
        let registration = WebCanvas::register(source)?;
        let handle = registration.handle();
        let signals = Rc::new(Cell::new(None));
        let app = WebApp::mount(surface, {
            let signals = signals.clone();
            move |build| {
                let visible = build.signal(true);
                let angle = build.signal(None);
                let popup = build.signal(false);
                signals.set(Some((visible, angle, popup)));
                let transform = Transform3D::default()
                    .perspective(420.0)
                    .translate(10.0, 6.0, 12.0)
                    .rotate_x(-18.0)
                    .origin(TransformOrigin { x: LayoutLength::Px(30.0), y: LayoutLength::Pct(0.25), z: 4.0 });
                let card = el()
                    .test_id("card")
                    .position(Position::Absolute)
                    .left(32)
                    .top(40)
                    .w(180)
                    .h(120)
                    .opacity(0.75)
                    .style_with(move |read, patch| patch.transform_3d(read.get(angle).map(|angle| transform.rotate_y(angle))))
                    .child(show(
                        move |read| read.get(visible),
                        move |_| {
                            image_source(ImageSource::External(handle.clone()))
                                .test_id("external")
                                .w(180)
                                .h(120)
                                .rounded(24)
                                .style(StyleProp::Shadow(ShadowToken::Md.into()))
                        },
                    ));
                let sibling = el()
                    .test_id("sibling")
                    .position(Position::Absolute)
                    .left(90)
                    .top(76)
                    .w(36)
                    .h(36)
                    .style(StyleProp::ZIndex(2))
                    .bg(rgb(0x00ff00));
                let local_canvas = canvas(|_, canvas| canvas.fill(&Path::rect(Rect::new(0.0, 0.0, 80.0, 40.0)), rgb(0x004444)))
                    .test_id("local-canvas")
                    .position(Position::Absolute)
                    .left(240)
                    .top(40)
                    .w(80)
                    .h(40)
                    .transform_3d(transform.rotate_y(42.0));
                el().position(Position::Relative).w_full().h_full().child((card, sibling, local_canvas)).portal_part(
                    move |read| read.get(popup),
                    |_| el().test_id("popup").position(Position::Absolute).left(108).top(92).w(42).h(40).bg(rgb(0xff00ff)),
                )
            }
        })?;
        let (visible, angle, popup) = signals.get().unwrap();
        Ok(CompositorFixture { app: Some(app), registration: Some(registration), visible, angle, popup })
    }

    #[wasm_bindgen]
    pub fn run_canvas_rollback(surface: HtmlElement, expect_motion: bool) -> Result<(), JsValue> {
        std::panic::set_hook(Box::new(|panic| {
            let document = web_sys::window().unwrap().document().unwrap();
            document.body().unwrap().set_attribute("data-panic", &panic.to_string()).unwrap();
        }));
        let document = surface.owner_document().unwrap();
        let canvas = || document.create_element("canvas")?.dyn_into::<HtmlCanvasElement>();
        let sources = [canvas()?, canvas()?];
        let registrations = [WebCanvas::register(sources[0].clone())?, WebCanvas::register(sources[1].clone())?];
        let handles = registrations.each_ref().map(WebCanvas::handle);
        let viewport = WindowViewport { size: Size::new(420.0, 290.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut engine = DomEngine::new(surface)?;
        let mut tree = Tree::new(TreeId::new([1, 2], 3), Theme::default());
        let mut tx = tree.transaction();
        let media = handles.each_ref().map(|handle| {
            tx.create(NodeSpec::new(NodeContent::ImageSource { source: ImageSource::External(handle.clone()), fit: ObjectFit::Fill }))
        });
        for (index, media) in media.into_iter().enumerate() {
            tx.place(tree.node_ref(tree.root()), index, media);
        }
        tx.change(
            media[1],
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [
                    StyleProp::Position(Position::Absolute),
                    StyleProp::Left(14.into()),
                    StyleProp::Top(22.into()),
                    StyleProp::Width(100.into()),
                    StyleProp::Height(60.into()),
                    StyleProp::Transform3D(Some(Transform3D::default().perspective(240.0).rotate_y(42.0))),
                    StyleProp::OpacityMotion(Curve::Fade(0.3)),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            })),
        );
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let nodes = media.map(|media| receipt.node(media).unwrap());
        let update = |node, handle: &ExternalTextureHandle| RuntimeOp::SetMedia {
            node,
            state: ImageState::Ready { natural_size: handle.size() },
            asset: Some(MediaAsset::External(handle.clone())),
        };
        let mut initial = tree.initial_delta();
        initial.merge(mounted);
        for (node, handle) in nodes.into_iter().zip(&handles) {
            initial.merge(tree.apply_runtime(update(node, handle)));
        }
        engine.project(&tree, &initial, viewport, 0.0).unwrap();
        assert!(sources.iter().all(|source| source.is_connected()));
        engine.rollback();
        assert!(sources.iter().all(|source| !source.is_connected()));
        engine.project(&tree, &initial, viewport, 0.0).unwrap();
        engine.commit();

        let containers = sources.each_ref().map(|source| source.parent_node().unwrap());
        let placements = containers.each_ref().map(|container| container.parent_node().unwrap());
        let assert_swapped = |swapped| {
            for (index, container) in containers.iter().enumerate() {
                let owner = if swapped { 1 - index } else { index };
                assert_eq!(container.parent_node().as_ref(), Some(&placements[owner]));
            }
        };
        let mut tx = tree.transaction();
        for (node, handle) in nodes.into_iter().zip(handles.iter().rev()) {
            tx.change(tree.node_ref(node), NodeChange::ImageSource { source: ImageSource::External(handle.clone()), fit: ObjectFit::Fill });
        }
        let (_, mut swapped) = tree.apply(tx.finish()).unwrap();
        for (node, handle) in nodes.into_iter().zip(handles.iter().rev()) {
            swapped.merge(tree.apply_runtime(update(node, handle)));
        }
        engine.project(&tree, &swapped, viewport, 0.0).unwrap();
        assert_swapped(true);
        engine.rollback();
        assert_swapped(false);
        engine.project(&tree, &swapped, viewport, 0.0).unwrap();
        engine.commit();
        assert_swapped(true);

        let transformed = placements[1].unchecked_ref::<HtmlElement>();
        let rect = transformed.get_bounding_client_rect();
        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(nodes[1]), Retirement::VisibleExit);
        let (_, exit) = tree.apply(tx.finish()).unwrap();
        let feedback = engine.project(&tree, &exit, viewport, 0.0).unwrap();
        if expect_motion {
            assert!(feedback.next_deadline.is_some());
            let exiting = transformed.get_bounding_client_rect();
            let before = [rect.x(), rect.y(), rect.width(), rect.height()];
            let during = [exiting.x(), exiting.y(), exiting.width(), exiting.height()];
            assert!(before.into_iter().zip(during).all(|(old, new)| (old - new).abs() < 0.02));
            assert!(sources[0].is_connected());
        }
        engine.rollback();
        assert_swapped(true);

        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(nodes[1]), Retirement::Destroy);
        let (_, retired) = tree.apply(tx.finish()).unwrap();
        engine.project(&tree, &retired, viewport, 0.0).unwrap();
        assert!(containers[0].parent_node().is_none());
        engine.rollback();
        assert_swapped(true);
        assert!(sources[0].is_connected());
        engine.project(&tree, &retired, viewport, 0.0).unwrap();
        engine.commit();
        assert!(containers[0].parent_node().is_none());
        assert!(!sources[0].is_connected());
        Ok(())
    }
}
