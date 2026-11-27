//! Browser regression: build for wasm32 and call run_dom_transactions() after wasm-bindgen initialization.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use js_sys::{Array, Function, Promise, Reflect};
    use octara::runtime::engine::HostEngine;
    use octara::tree::editor::EditorSlot;
    use octara::tree::tables::{AuthoredStyle, SemanticsSlot};
    use octara::tree::transaction::OverlaySpec;
    use octara::tree::{NodeChange, NodeContent, NodeFlags, NodeId, NodeSpec, Retirement, RuntimeOp, Tree, TreeDelta, TreeId};
    use octara_host_web::{DomEngine, MeasureMode};
    use octara_model::geometry::{Placement, Point, Rect, Size, Vec2, WindowViewport};
    use octara_model::input::HostReply;
    use octara_model::motion::Curve;
    use octara_model::paint::{Canvas, CanvasOp, Icon, Image, NodeIcon, ObjectFit, Path};
    use octara_model::semantics::Relations;
    use octara_model::style::{Display, FlexDirection, Length, Overflow, Position, ScrollbarVisibility, StyleProp};
    use octara_model::theme::{Theme, rgb};
    use std::sync::Arc;
    use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlElement, HtmlInputElement};

    const VIEWPORT: WindowViewport = WindowViewport { size: Size::new(400.0, 300.0), scale: 1.0, safe_area: None };

    fn style(props: impl IntoIterator<Item = StyleProp>) -> NodeChange {
        NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() }))
    }

    fn drawing(color: u32) -> Arc<[CanvasOp]> {
        let mut canvas = Canvas::new(40.0, 30.0);
        canvas.clip(Rect::new(0.0, 0.0, 40.0, 30.0), 2.0, |canvas| {
            canvas.fill(&Path::rect(Rect::new(0.0, 0.0, 40.0, 30.0)), rgb(color));
        });
        canvas.into_ops().into()
    }

    fn settle(engine: &mut DomEngine, tree: &mut Tree, mut delta: TreeDelta) {
        for _ in 0..32 {
            let feedback = engine.project(tree, &delta, VIEWPORT, 0.0).unwrap();
            if feedback.updates.is_empty() {
                engine.commit();
                return;
            }
            delta = TreeDelta::default();
            for update in feedback.updates {
                delta.merge(tree.apply_runtime(update));
            }
        }
        panic!("DOM feedback did not converge");
    }

    fn element(surface: &HtmlElement, name: &str) -> HtmlElement {
        surface.query_selector(&format!("[data-octara-id={name}]")).unwrap().unwrap().dyn_into().unwrap()
    }

    fn pixel_at(element: &HtmlElement, x: f64) -> Vec<u8> {
        element
            .unchecked_ref::<HtmlCanvasElement>()
            .get_context("2d")
            .unwrap()
            .unwrap()
            .dyn_into::<CanvasRenderingContext2d>()
            .unwrap()
            .get_image_data(x, 0.0, 1.0, 1.0)
            .unwrap()
            .data()
            .0
    }

    #[wasm_bindgen]
    pub fn run_dom_scrolling() -> Result<(), JsValue> {
        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([6, 1], 1), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let root = tree.node_ref(tree.root());
        let port_style = |visibility| {
            style([
                StyleProp::Width(Length::Px(160.0)),
                StyleProp::Height(Length::Px(60.0)),
                StyleProp::Shrink(0.0),
                StyleProp::Direction(FlexDirection::Column),
                StyleProp::OverflowY(Overflow::Scroll),
                StyleProp::Scrollbar(visibility),
            ])
        };
        let fixed_height = |height| style([StyleProp::Height(Length::Px(height)), StyleProp::Shrink(0.0)]);
        let anchored = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(anchored, NodeChange::TestId(Some("anchored-port".into())));
        tx.change(anchored, port_style(ScrollbarVisibility::Hidden));
        tx.change(anchored, NodeChange::Flags(NodeFlags::MAINTAIN_SCROLL_ANCHOR));
        tx.place(root, 0, anchored);
        let mut clips = Vec::new();
        for (index, (name, height)) in
            [("leading-row", 100.0), ("anchor-row", 60.0), ("end-row", 30.0), ("tail-row", 100.0)].into_iter().enumerate()
        {
            let row = tx.create(NodeSpec::new(NodeContent::Box));
            tx.change(row, NodeChange::TestId(Some(name.into())));
            tx.change(row, fixed_height(height));
            if name == "end-row" {
                tx.change(row, NodeChange::Flags(NodeFlags::FOCUSABLE));
            }
            tx.place(anchored, index, row);
            clips.push(row);
        }
        let follow = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(follow, NodeChange::TestId(Some("follow-port".into())));
        tx.change(follow, port_style(ScrollbarVisibility::Always));
        tx.change(follow, NodeChange::Flags(NodeFlags::FOLLOW_TAIL));
        tx.place(root, 1, follow);
        let stream = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(stream, fixed_height(160.0));
        tx.place(follow, 0, stream);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        let anchored = receipt.node(anchored).unwrap();
        let focus = receipt.node(clips[2]).unwrap();
        let follow = receipt.node(follow).unwrap();
        let stream = receipt.node(stream).unwrap();
        delta.merge(tree.apply_runtime(RuntimeOp::RequestScroll { node: anchored, target: Vec2::new(0.0, 105.0) }));
        settle(&mut engine, &mut tree, delta);
        let port = element(&surface, "anchored-port");
        let anchor = element(&surface, "anchor-row");
        let follow_port = element(&surface, "follow-port");
        assert_eq!(browser.get_computed_style(&port)?.unwrap().get_property_value("scrollbar-width")?, "none");
        assert_eq!(browser.get_computed_style(&port)?.unwrap().get_property_value("overflow-anchor")?, "auto");
        assert_eq!(browser.get_computed_style(&follow_port)?.unwrap().get_property_value("scrollbar-width")?, "thin");
        assert_eq!(browser.get_computed_style(&follow_port)?.unwrap().get_property_value("overflow-anchor")?, "none");
        assert_eq!(follow_port.scroll_top(), 100);

        let previous = anchor.get_bounding_client_rect().y();
        let mut tx = tree.transaction();
        let inserted = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(inserted, fixed_height(25.0));
        tx.place(tree.node_ref(anchored), 0, inserted);
        tx.change(tree.node_ref(stream), fixed_height(260.0));
        let (_, mut delta) = tree.apply(tx.finish()).unwrap();
        delta.merge(tree.apply_runtime(RuntimeOp::RequestScroll { node: follow, target: Vec2::new(0.0, 100.0) }));
        settle(&mut engine, &mut tree, delta);
        assert_eq!(anchor.get_bounding_client_rect().y(), previous);
        assert_eq!(port.scroll_top(), 130);
        assert_eq!(follow_port.scroll_top(), 100);
        assert_eq!(browser.get_computed_style(&follow_port)?.unwrap().get_property_value("overflow-anchor")?, "auto");

        let delta = tree.apply_runtime(RuntimeOp::SetFocus { node: Some(focus), visible: true });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(port.scroll_top(), 155);
        assert_eq!(document.active_element().as_ref(), Some(element(&surface, "end-row").as_ref()));
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_rich_pills() -> Result<(), JsValue> {
        use octara::prelude::*;
        use octara_host_web::WebApp;

        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let app = WebApp::mount(surface.clone(), |_| {
            text(
                rich()
                    .text("Before ")
                    .link_span(
                        "pill",
                        "#pill",
                        Span {
                            background: Some(rgb(0xf0f0f0).into()),
                            border_color: Some(rgb(0x284572).into()),
                            radius: Some(4.0),
                            padding_x: Some(5.0),
                            ..Default::default()
                        }
                        .padding_y(2.0),
                    )
                    .text(" after"),
            )
            .tw(tw!("items-center italic underline line-through tracking-[1.5px] tabular-nums"))
            .min_h(64.0)
            .test_id("rich-pill")
        })?;
        let label = element(&surface, "rich-pill");
        assert_eq!(label.text_content().as_deref(), Some("Before pill after"));
        let label_style = browser.get_computed_style(&label)?.unwrap();
        assert_eq!(label_style.get_property_value("font-style")?, "italic");
        assert_eq!(label_style.get_property_value("text-decoration-line")?, "underline line-through");
        assert_eq!(label_style.get_property_value("letter-spacing")?, "1.5px");
        assert_eq!(label_style.get_property_value("font-variant-numeric")?, "tabular-nums");
        let span = label.query_selector("a > span")?.expect("an inline span inside the link");
        let style = browser.get_computed_style(&span)?.unwrap();
        assert_eq!(style.get_property_value("display")?, "inline");
        assert_eq!(style.get_property_value("border-top-left-radius")?, "4px");
        assert_eq!(style.get_property_value("padding-inline-start")?, "5px");
        assert_eq!(style.get_property_value("padding-inline-end")?, "5px");
        assert_eq!(style.get_property_value("padding-block-start")?, "2px");
        assert_eq!(style.get_property_value("padding-block-end")?, "2px");
        assert_eq!(style.get_property_value("box-decoration-break")?, "clone");
        assert_eq!(style.get_property_value("border-top-width")?, "1px");
        assert_eq!(style.get_property_value("border-top-style")?, "solid");
        assert_eq!(style.get_property_value("border-top-color")?, "rgb(40, 69, 114)");
        assert_eq!(style.get_property_value("background-color")?, "rgb(240, 240, 240)");
        let range = document.create_range()?;
        range.select_node_contents(&span)?;
        assert!((span.get_bounding_client_rect().width() - range.get_bounding_client_rect().width() - 12.0).abs() < 0.1);
        let label = label.get_bounding_client_rect();
        let glyphs = range.get_bounding_client_rect();
        assert!((glyphs.y() + glyphs.height() / 2.0 - label.y() - label.height() / 2.0).abs() < 2.0);
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_text_selection() -> Result<(), JsValue> {
        use octara::prelude::*;
        use octara_host_web::WebApp;
        use web_sys::{ClipboardEvent, Event};

        let document = web_sys::window().unwrap().document().unwrap();
        let body = document.body().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        body.append_child(&surface)?;
        let app = WebApp::mount(surface.clone(), |_| {
            col().child(text("Outside")).child(
                col()
                    .selectable(true)
                    .test_id("selection-region")
                    .copy_selection_with(|_, ranges| {
                        (ranges.len() == 2 && ranges.iter().all(|text| text.whole)).then(|| "# First café\n\nSecond **日本**".into())
                    })
                    .child(text("First café").test_id("selection-start"))
                    .child(
                        text(rich().text("Second ").link("日本", "#japan").attachment(InlineAttachment {
                            range: 7..13,
                            size: Size::default(),
                            align: InlineAlign::Baseline,
                            baseline: None,
                            content: InlineContent::Spacer { width: 16.0 },
                        }))
                        .test_id("selection-end"),
                    ),
            )
        })?;
        let last = element(&surface, "selection-end");
        let selection = document.get_selection()?.unwrap();
        selection.set_base_and_extent(&last, 0, &last, 0)?;

        let pointer = Function::new_with_args(
            "name,target",
            "const r = target.getBoundingClientRect(); return new PointerEvent(name,
            {bubbles:true,cancelable:true,clientX:r.x+4,clientY:r.y+4,pointerId:1,pointerType:'mouse',isPrimary:true});",
        );
        for name in ["pointerdown", "pointerup"] {
            let event = pointer.call2(&JsValue::UNDEFINED, &name.into(), last.as_ref())?.dyn_into::<Event>()?;
            assert!(last.dispatch_event(&event)?);
        }
        assert!(selection.is_collapsed());
        let key = Function::new_no_args("return new KeyboardEvent('keydown', {key:'a',ctrlKey:true,bubbles:true,cancelable:true});");
        let event = key.call0(&JsValue::UNDEFINED)?.dyn_into::<Event>()?;
        assert!(!body.dispatch_event(&event)?);
        let contents = selection.get_range_at(0)?.clone_contents()?;
        assert_eq!(contents.text_content().as_deref(), Some("First caféSecond "));
        let object = last.query_selector("a > [data-octara-inline-source]")?.expect("inline object inside link");
        assert_eq!(object.get_attribute("aria-label").as_deref(), Some("日本"));
        document.dispatch_event(&Event::new("selectionchange")?)?;

        let copy =
            Function::new_no_args("return new ClipboardEvent('copy', {clipboardData:new DataTransfer(),bubbles:true,cancelable:true});");
        let event = copy.call0(&JsValue::UNDEFINED)?.dyn_into::<ClipboardEvent>()?;
        assert!(!body.dispatch_event(&event)?);
        assert_eq!(event.clipboard_data().unwrap().get_data("text/plain")?, "# First café\n\nSecond **日本**");
        app.with_app(|inner| {
            let tree = inner.runtime().window(app.window_id()).unwrap().tree();
            let region = tree.test_ids("selection-region")[0];
            assert_eq!(tree.selected_text(region).as_deref(), Some("First café\nSecond 日本"));
        })
        .unwrap();
        selection.remove_all_ranges()?;
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_inline_editor() -> Result<(), JsValue> {
        use octara::prelude::*;
        use octara_host_web::WebApp;
        use web_sys::Event;

        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        document.body().unwrap().append_child(&surface)?;
        let app = WebApp::mount(surface.clone(), |build| {
            let value = build.signal("A @β Z".to_owned());
            input().bind_value(value).test_id("inline-editor").rich_text(EditorRichText {
                attachments: Arc::from([InlineAttachment {
                    range: 2..5,
                    size: Size::new(24.0, 14.0),
                    align: InlineAlign::CapHeightCenter,
                    baseline: None,
                    content: InlineContent::Vector { ops: drawing(0x397bd6) },
                }]),
                atoms: Arc::from([AtomicRange { range: 2..5, kind: "mention".into() }]),
                ..Default::default()
            })
        })?;
        let editor = element(&surface, "inline-editor");
        assert_eq!(editor.get_attribute("contenteditable").as_deref(), Some("true"));
        let object = editor.query_selector("[data-octara-inline-source]")?.expect("inline object");
        assert_eq!(object.get_attribute("aria-label").as_deref(), Some("@β"));
        editor.focus()?;
        let selection = document.get_selection()?.unwrap();
        let end = editor.child_nodes().length();
        selection.set_base_and_extent(&editor, end, &editor, end)?;
        let paste = Function::new_no_args(
            "return new InputEvent('beforeinput', {inputType:'insertFromPaste',data:'!',bubbles:true,cancelable:true});",
        );
        let event = paste.call0(&JsValue::UNDEFINED)?.dyn_into::<Event>()?;
        assert!(!editor.dispatch_event(&event)?);

        selection.set_base_and_extent(&editor, 2, &editor, 2)?;
        let delete = Function::new_no_args(
            "return new InputEvent('beforeinput', {inputType:'deleteContentBackward',bubbles:true,cancelable:true});",
        );
        let event = delete.call0(&JsValue::UNDEFINED)?.dyn_into::<Event>()?;
        assert!(!editor.dispatch_event(&event)?);
        let plain = element(&surface, "inline-editor").dyn_into::<HtmlInputElement>()?;
        assert_eq!(plain.value(), "A  Z!");
        assert_eq!(document.active_element().as_ref(), Some(plain.as_ref()));

        let undo = Function::new_no_args("return new KeyboardEvent('keydown',{key:'z',ctrlKey:true,bubbles:true,cancelable:true});");
        let event = undo.call0(&JsValue::UNDEFINED)?.dyn_into::<Event>()?;
        assert!(!plain.dispatch_event(&event)?);
        let restored = element(&surface, "inline-editor");
        assert_eq!(restored.get_attribute("contenteditable").as_deref(), Some("true"));
        assert!(restored.query_selector("[data-octara-inline-source]")?.is_some());
        assert_eq!(restored.text_content().as_deref(), Some("A  Z!"));
        assert_eq!(document.active_element().as_ref(), Some(restored.as_ref()));
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_text_anchors() -> Result<(), JsValue> {
        use octara::tree::editor::{EditorAction, EditorRichText, EditorSession};
        use octara_model::input::EditorEvent;
        use octara_model::overlay::AnchorTarget;
        use octara_model::text::{InlineAlign, InlineAttachment, InlineContent};

        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([6, 2], 1), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("a😀z")))));
        let label = tx.create(NodeSpec::new(NodeContent::Text("selected text".into())));
        tx.change(editor, NodeChange::Flags(NodeFlags::FOCUSABLE));
        tx.change(editor, style([StyleProp::Width(180.into()), StyleProp::Height(32.into())]));
        tx.change(label, NodeChange::Flags(NodeFlags::SELECT_REGION));
        tx.change(label, NodeChange::TestId(Some("anchor-label".into())));
        tx.place(tree.node_ref(tree.root()), 0, editor);
        tx.place(tree.node_ref(tree.root()), 1, label);
        let overlays = [
            AnchorTarget::EditorCaret(editor.into()),
            AnchorTarget::TextSelection(editor.into()),
            AnchorTarget::TextSelection(label.into()),
        ]
        .map(|target| {
            let overlay = tx.create(NodeSpec::new(NodeContent::Box));
            tx.change(overlay, style([StyleProp::Width(20.into()), StyleProp::Height(12.into())]));
            tx.change(overlay, NodeChange::Overlay(OverlaySpec { target: Some((target, Placement::BELOW)), ..Default::default() }));
            tx.place(tree.node_ref(tree.overlay_root()), 0, overlay);
            overlay
        });
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        let editor = receipt.node(editor).unwrap();
        let label = receipt.node(label).unwrap();
        let [caret, selected, document_selected] = overlays.map(|overlay| receipt.node(overlay).unwrap());
        delta.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(editor), visible: false }));

        let select = |tree: &mut Tree, selection| {
            let session = EditorSession::active(tree).unwrap();
            let event = EditorEvent::ProposeSelection { session: session.id(), expected_rev: session.editor().document_rev(), selection };
            let EditorAction::Apply(update) = session.route(event).unwrap() else { panic!("editor selection update") };
            tree.apply_runtime(RuntimeOp::EditorApply { node: editor, update })
        };
        let placed = |tree: &Tree, node| tree.overlay(node).unwrap().placed;

        delta.merge(select(&mut tree, (0, 0)));
        settle(&mut engine, &mut tree, delta);
        let left = placed(&tree, caret).unwrap().x;
        assert!(placed(&tree, selected).is_none());
        let delta = select(&mut tree, (5, 5));
        settle(&mut engine, &mut tree, delta);
        assert!(placed(&tree, caret).unwrap().x > left + 8.0);

        let mut tx = tree.transaction();
        tx.change(
            tree.node_ref(editor),
            NodeChange::EditorRichText(Box::new(EditorRichText {
                attachments: Arc::from([InlineAttachment {
                    range: 1..5,
                    size: Size::default(),
                    align: InlineAlign::Baseline,
                    baseline: None,
                    content: InlineContent::Spacer { width: 36.0 },
                }]),
                ..Default::default()
            })),
        );
        let (_, mut delta) = tree.apply(tx.finish()).unwrap();
        delta.merge(select(&mut tree, (1, 5)));
        settle(&mut engine, &mut tree, delta);
        let caret_placement = placed(&tree, caret).unwrap();
        let selection_placement = placed(&tree, selected).unwrap();
        assert!(caret_placement.x > selection_placement.x + 30.0);

        let text = element(&surface, "anchor-label").first_child().unwrap();
        document.get_selection()?.unwrap().set_base_and_extent(&text, 0, &text, 8)?;
        let delta = tree.apply_runtime(RuntimeOp::SetDocSelection { region: label, selection: Some([(label, 0), (label, 8)]) });
        settle(&mut engine, &mut tree, delta);
        assert!(placed(&tree, document_selected).is_some());
        document.get_selection()?.unwrap().remove_all_ranges()?;
        let delta = tree.apply_runtime(RuntimeOp::SetDocSelection { region: label, selection: None });
        settle(&mut engine, &mut tree, delta);
        assert!(placed(&tree, document_selected).is_none());
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_text_reveal() -> Result<(), JsValue> {
        use octara::tree::transaction::RichText;
        use octara_model::motion::{TextReveal, TextRevealKind};
        use octara_model::text::{InlineAlign, InlineAttachment, InlineContent};

        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([6, 2], 2), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
        tx.change(label, NodeChange::TestId(Some("reveal-label".into())));
        tx.change(
            label,
            NodeChange::RichText(Box::new(RichText {
                text: "A😀hidden".into(),
                attachments: vec![InlineAttachment {
                    range: 1..5,
                    size: Size::default(),
                    align: InlineAlign::Baseline,
                    baseline: None,
                    content: InlineContent::Spacer { width: 12.0 },
                }],
                ..Default::default()
            })),
        );
        tx.place(tree.node_ref(tree.root()), 0, label);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        settle(&mut engine, &mut tree, delta);
        let label = receipt.node(label).unwrap();
        let element = element(&surface, "reveal-label");
        let first = element.first_child().unwrap();
        let width = element.get_bounding_client_rect().width();
        let registry = Function::new_no_args(
            "return CSS.highlights ? Array.from(CSS.highlights).filter(([name]) => name.startsWith('octara-reveal-')).flatMap(([, value]) => Array.from(value, range => range.toString())).join('') : null;",
        );
        let highlighted = || registry.call0(&JsValue::UNDEFINED).unwrap().as_string();
        let Some(previous) = highlighted() else {
            surface.remove();
            return Ok(());
        };
        assert!(previous.is_empty());
        let reveal = TextReveal { kind: TextRevealKind::Fade { duration_secs: 0.0 }, revealed_bytes: 6, finished: false };
        let set = |tree: &mut Tree, reveal| {
            let mut tx = tree.transaction();
            tx.change(tree.node_ref(label), NodeChange::TextReveal(Some(reveal)));
            tree.apply(tx.finish()).unwrap().1
        };
        let delta = set(&mut tree, reveal);
        settle(&mut engine, &mut tree, delta);
        assert_eq!(highlighted().as_deref(), Some("idden"));
        assert_eq!(element.text_content().as_deref(), Some("Ahidden"));
        assert!(element.first_child().unwrap().is_same_node(Some(&first)));
        assert_eq!(element.get_bounding_client_rect().width(), width);
        engine.set_reduced_motion(true);
        settle(&mut engine, &mut tree, TreeDelta::default());
        assert_eq!(highlighted().as_deref(), Some(""));
        engine.set_reduced_motion(false);
        settle(&mut engine, &mut tree, TreeDelta::default());
        assert_eq!(highlighted().as_deref(), Some("idden"));
        let delta = set(&mut tree, TextReveal { finished: true, ..reveal });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(highlighted().as_deref(), Some(""));
        assert!(element.first_child().unwrap().is_same_node(Some(&first)));
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_links() -> Result<(), JsValue> {
        use octara::prelude::*;
        use octara_host_web::WebApp;

        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let original_url = browser.location().href()?;
        let original_hash = browser.location().hash()?;
        let app = WebApp::mount(surface.clone(), |b| {
            let activations = b.signal(0);
            col()
                .on_link(move |w, link| {
                    if link.kind == LinkEventKind::Press && &*link.href == "#internal-link" {
                        w.update(activations, |n| *n += 1);
                        true
                    } else {
                        false
                    }
                })
                .child(
                    text(rich().link_span("internal", "#internal-link", Span::default()).text(" ").link("external", "#external-link"))
                        .test_id("links"),
                )
                .child(text(move |r: &mut ReadCx<'_>| r.get(activations).to_string()).test_id("activations"))
        })?;
        let links = element(&surface, "links");
        let anchor = |selector| -> Result<HtmlElement, JsValue> { links.query_selector(selector)?.unwrap().dyn_into().map_err(Into::into) };
        let internal = anchor("a[href='#internal-link']")?;
        let external = anchor("a[href='#external-link']")?;
        let decoration = |link: &HtmlElement| browser.get_computed_style(link)?.unwrap().get_property_value("text-decoration-line");
        assert_eq!(decoration(&internal)?, "none");
        assert_eq!(decoration(&external)?, "underline");

        let activations = element(&surface, "activations");
        internal.click();
        assert_eq!(activations.text_content().as_deref(), Some("1"));
        assert_eq!(browser.location().hash()?, original_hash);
        let pointer = Function::new_with_args(
            "name,target",
            "const r = target.getBoundingClientRect(); return new PointerEvent(name, {bubbles:true,cancelable:true,
            clientX:r.x+r.width/2,clientY:r.y+r.height/2,pointerId:1,pointerType:'mouse',isPrimary:true});",
        );
        for name in ["pointerdown", "pointerup"] {
            let event = pointer.call2(&JsValue::UNDEFINED, &name.into(), internal.as_ref())?.dyn_into::<web_sys::Event>()?;
            internal.dispatch_event(&event)?;
        }
        let click = Function::new_no_args("return new MouseEvent('click', {bubbles:true,cancelable:true,detail:1});");
        let event = click.call0(&JsValue::UNDEFINED)?.dyn_into::<web_sys::Event>()?;
        assert!(!internal.dispatch_event(&event)?);
        assert_eq!(activations.text_content().as_deref(), Some("2"));
        assert_eq!(browser.location().hash()?, original_hash);

        external.click();
        assert_eq!(browser.location().hash()?, "#external-link");
        browser.history()?.replace_state_with_url(&JsValue::NULL, "", Some(&original_url))?;
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_layers() -> Result<(), JsValue> {
        use octara::prelude::*;
        use octara_host_web::WebApp;

        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let app = WebApp::mount(surface.clone(), |b| {
            let open = b.signal(false);
            let button = |name| el().test_id(name).role(Role::Button).focusable().child(text(name));
            col().child(button("outside")).child(button("trigger").on_press(move |w| w.set(open, true)).overlay(
                Placement::BELOW,
                move |r| r.get(open),
                move |_| {
                    col()
                        .layer(LayerPolicy { focus_trap: FocusTrapFeatures::ALL, ..LayerPolicy::non_modal() }, move |w| w.set(open, false))
                        .child(button("panel-first"))
                        .child(button("panel-last"))
                },
            ))
        })?;
        let trigger = element(&surface, "trigger");
        let outside = element(&surface, "outside");
        let focused = |name| {
            assert_eq!(document.active_element().and_then(|element| element.get_attribute("data-octara-id")).as_deref(), Some(name));
        };
        trigger.focus()?;
        trigger.click();
        focused("panel-first");
        outside.focus()?;
        focused("panel-first");

        let last = element(&surface, "panel-last");
        last.focus()?;
        focused("panel-last");
        let key = Function::new_with_args("key", "return new KeyboardEvent('keydown', {key, bubbles: true, cancelable: true});");
        let tab = key.call1(&JsValue::UNDEFINED, &"Tab".into())?.dyn_into::<web_sys::Event>()?;
        assert!(!last.dispatch_event(&tab)?);
        focused("trigger");
        let escape = key.call1(&JsValue::UNDEFINED, &"Escape".into())?.dyn_into::<web_sys::Event>()?;
        assert!(!trigger.dispatch_event(&escape)?);
        assert!(surface.query_selector("[data-octara-id=panel-first]")?.is_none());
        focused("trigger");
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_pointer_styles() -> Result<(), JsValue> {
        use octara::tree::NodeFlags;
        use octara_model::input::CursorStyle;
        use octara_model::semantics::Role;
        use octara_model::style::{State, UserSelect, When};

        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 7], 21), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let back = tx.create(NodeSpec::new(NodeContent::Box));
        let front = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        let bounds = [
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(Length::Px(0.0)),
            StyleProp::Top(Length::Px(0.0)),
            StyleProp::Width(Length::Px(80.0)),
            StyleProp::Height(Length::Px(80.0)),
        ];
        tx.change(back, style(bounds));
        tx.change(front, NodeChange::TestId(Some("pointer-front".into())));
        tx.change(child, NodeChange::TestId(Some("pointer-child".into())));
        tx.change(front, NodeChange::Flags(NodeFlags::FOCUSABLE));
        tx.change(
            front,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: bounds
                    .into_iter()
                    .chain([
                        StyleProp::PointerEvents(false),
                        StyleProp::UserSelect(UserSelect::None),
                        StyleProp::Cursor(CursorStyle::ZoomIn),
                    ])
                    .collect(),
                overrides: [(
                    When::State(State::SELECTED),
                    vec![
                        StyleProp::PointerEvents(true),
                        StyleProp::UserSelect(UserSelect::Text),
                        StyleProp::Cursor(CursorStyle::ContextMenu),
                    ],
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })),
        );
        tx.change(child, style([StyleProp::Width(Length::Px(20.0)), StyleProp::Height(Length::Px(20.0))]));
        tx.place(tree.node_ref(tree.root()), 0, back);
        tx.place(tree.node_ref(tree.overlay_root()), 0, front);
        tx.place(front, 0, child);
        let (receipt, added) = tree.apply(tx.finish()).unwrap();
        let [back, front, child] = [back, front, child].map(|node| receipt.node(node).unwrap());
        delta.merge(added);
        delta.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(front), visible: true }));
        settle(&mut engine, &mut tree, delta);
        let front_element = element(&surface, "pointer-front");
        let child_element = element(&surface, "pointer-child");
        let child_cursor = || browser.get_computed_style(&child_element)?.unwrap().get_property_value("cursor");
        let hit = |engine: &DomEngine, tree: &Tree, point| engine.hit_test(tree, point).and_then(|target| target.hit.node());
        assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(back));
        assert_eq!(hit(&engine, &tree, Point::new(5.0, 5.0)), Some(child));
        assert_eq!(front_element.style().get_property_value("pointer-events")?, "none");
        assert_eq!(front_element.style().get_property_value("user-select")?, "none");
        assert_eq!(front_element.style().get_property_value("cursor")?, "zoom-in");
        assert_eq!(child_element.style().get_property_value("pointer-events")?, "auto");
        assert_eq!(child_cursor()?, "zoom-in");
        assert_eq!(document.active_element().as_ref(), Some(front_element.as_ref()));

        let delta = tree.apply_runtime(RuntimeOp::SetState { node: front, state: State::SELECTED, on: true });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(front));
        assert_eq!(front_element.style().get_property_value("pointer-events")?, "auto");
        assert_eq!(front_element.style().get_property_value("user-select")?, "text");
        assert_eq!(front_element.style().get_property_value("cursor")?, "context-menu");
        assert_eq!(child_element.style().get_property_value("pointer-events")?, "");
        assert_eq!(child_cursor()?, "context-menu");
        assert_eq!(document.active_element().as_ref(), Some(front_element.as_ref()));

        let delta = tree.apply_runtime(RuntimeOp::SetState { node: front, state: State::SELECTED, on: false });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(back));
        assert_eq!(child_element.style().get_property_value("pointer-events")?, "auto");

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(front), style(bounds));
        tx.change(tree.node_ref(front), NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::Button), ..Default::default() })));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        settle(&mut engine, &mut tree, delta);
        assert_eq!(child_cursor()?, "pointer");

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(front), NodeChange::Disabled(true));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        settle(&mut engine, &mut tree, delta);
        assert_eq!(child_cursor()?, "not-allowed");

        let mut tx = tree.transaction();
        tx.place(tree.node_ref(back), 0, tree.node_ref(child));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        settle(&mut engine, &mut tree, delta);
        assert_eq!(child_cursor()?, "default");
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_local_stacking() -> Result<(), JsValue> {
        use octara_model::style::Radius;

        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 7], 28), Theme::default());
        let mut tx = tree.transaction();
        let root = tree.node_ref(tree.root());
        let back = tx.create(NodeSpec::new(NodeContent::Box));
        let nested = tx.create(NodeSpec::new(NodeContent::Box));
        let front = tx.create(NodeSpec::new(NodeContent::Box));
        let bounds = [
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(0.into()),
            StyleProp::Top(0.into()),
            StyleProp::Width(80.into()),
            StyleProp::Height(80.into()),
        ];
        for (node, index) in [(back, -5), (nested, 50), (front, 10)] {
            tx.change(node, style(bounds.into_iter().chain([StyleProp::ZIndex(index), StyleProp::RadiusTopLeft(Radius::Px(20.0))])));
        }
        tx.change(front, NodeChange::TestId(Some("stack-front".into())));
        tx.place(root, 0, back);
        tx.place(root, 1, front);
        tx.place(back, 0, nested);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let mut delta = tree.initial_delta();
        delta.merge(mounted);
        settle(&mut engine, &mut tree, delta);
        let front_element = element(&surface, "stack-front");
        assert_eq!(front_element.style().get_property_value("z-index")?, "10");
        assert_eq!(front_element.style().get_property_value("border-top-left-radius")?, "20px");
        assert_eq!(front_element.style().get_property_value("border-top-right-radius")?, "0px");
        assert_eq!(engine.hit_test(&tree, Point::new(40.0, 40.0)).unwrap().hit.node(), receipt.node(front));
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_semantics() -> Result<(), JsValue> {
        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 7], 9), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let check = tx.create(NodeSpec::new(NodeContent::Box));
        let description = tx.create(NodeSpec::new(NodeContent::Text("Partly checked".into())));
        let root = tree.node_ref(tree.root());
        tx.place(root, 0, check);
        tx.place(root, 1, description);
        tx.change(check, NodeChange::TestId(Some("mixed-check".into())));
        tx.change(check, NodeChange::Flags(octara::tree::NodeFlags::NATIVE_CHECKBOX));
        tx.change(description, NodeChange::TestId(Some("description".into())));
        let semantics = |checked| {
            NodeChange::Semantics(Box::new(SemanticsSlot {
                role: Some(Role::CheckBox),
                checked: Some(checked),
                relations: Relations { described_by: Some("description".into()), ..Default::default() },
                ..Default::default()
            }))
        };
        tx.change(check, semantics(Checked::Mixed));
        let (receipt, added) = tree.apply(tx.finish()).unwrap();
        delta.merge(added);
        settle(&mut engine, &mut tree, delta);
        let input = element(&surface, "mixed-check").dyn_into::<HtmlInputElement>()?;
        let label = element(&surface, "description");
        assert_eq!(input.get_attribute("aria-describedby"), Some(label.id()));
        assert_eq!(input.get_attribute("aria-checked").as_deref(), Some("mixed"));
        assert!(input.indeterminate());
        assert!(!input.checked());

        let check = receipt.node(check).unwrap();
        let mut tx = tree.transaction();
        tx.change(tree.node_ref(check), semantics(Checked::Checked));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        assert!(input.checked());
        assert!(!input.indeterminate());
        engine.rollback();
        assert!(input.indeterminate());
        assert_eq!(input.get_attribute("aria-checked").as_deref(), Some("mixed"));
        settle(&mut engine, &mut tree, delta);
        assert_eq!(input.get_attribute("aria-checked").as_deref(), Some("true"));

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(check), semantics(Checked::Unchecked));
        tx.change(tree.node_ref(receipt.node(description).unwrap()), NodeChange::TestId(Some("renamed".into())));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        settle(&mut engine, &mut tree, delta);
        assert!(!input.checked() && !input.indeterminate());
        assert_eq!(input.get_attribute("aria-checked").as_deref(), Some("false"));
        assert!(input.get_attribute("aria-describedby").is_none());
        surface.remove();

        // A conditional child may start absent. The authored control still owns
        // a child slot and must retain an element that can render the child later.
        use octara::prelude::*;
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        document.body().unwrap().append_child(&surface)?;
        let toggles = std::rc::Rc::new(std::cell::Cell::new(None));
        let app = octara_host_web::WebApp::mount(surface.clone(), {
            let toggles = toggles.clone();
            move |b| {
                let show_child = b.signal(false);
                toggles.set(Some(show_child));
                el().test_id("composite")
                    .role(Role::CheckBox)
                    .checked(Checked::Mixed)
                    .child(show(move |r| r.get(show_child), |_| text("Now visible").test_id("child")))
            }
        })?;
        let composite = element(&surface, "composite");
        assert_ne!(composite.local_name(), "input");
        let visible = toggles.get().expect("mounted show signal");
        app.update(|w| w.set(visible, true)).unwrap();
        assert!(composite.contains(Some(&element(&surface, "child"))));
        assert_eq!(composite.get_attribute("aria-checked").as_deref(), Some("mixed"));
        drop(app);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_transactions(force_fallback: bool) -> Result<(), JsValue> {
        std::panic::set_hook(Box::new(|panic| {
            let document = web_sys::window().unwrap().document().unwrap();
            document.body().unwrap().set_attribute("data-panic", &panic.to_string()).unwrap();
        }));
        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;margin:20px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let root_proto = js_sys::Object::get_prototype_of(&surface);
        let move_before = Reflect::get(&root_proto, &"moveBefore".into())?;
        if force_fallback {
            Reflect::set(&root_proto, &"moveBefore".into(), &JsValue::UNDEFINED)?;
        }

        let mut tree = Tree::new(TreeId::new([4, 7], 1), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let left = tx.create(NodeSpec::new(NodeContent::Box));
        let right = tx.create(NodeSpec::new(NodeContent::Box));
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("a界😀z")))));
        let text = tx.create(NodeSpec::new(NodeContent::Text("initial".into())));
        let scroll = tx.create(NodeSpec::new(NodeContent::Box));
        let tall = tx.create(NodeSpec::new(NodeContent::Box));
        let overlay = tx.create(NodeSpec::new(NodeContent::Box));
        let image = Image::rgba(1, 1, [100, 30, 20, 255]);
        let picture = tx.create(NodeSpec::new(NodeContent::Image { image: image.clone(), fit: ObjectFit::Contain }));
        let drawing_node = tx.create(NodeSpec::new(NodeContent::Canvas(drawing(0x336699))));
        let icon = NodeIcon::new(&Icon::path(Path::rect(Rect::new(0.0, 0.0, 20.0, 20.0))), None);
        let icon_node = tx.create(NodeSpec::new(NodeContent::Icon(Arc::new(icon))));
        let id = |value: &'static str| NodeChange::TestId(Some(value.into()));
        for (index, parent, child) in [(0, left, editor), (1, left, text), (2, left, scroll), (0, right, picture), (0, scroll, tall)] {
            tx.place(parent, index, child);
        }
        tx.place(right, 1, drawing_node);
        tx.place(right, 2, icon_node);
        tx.place(tree.node_ref(tree.root()), 0, left);
        tx.place(tree.node_ref(tree.root()), 1, right);
        tx.place(tree.node_ref(tree.overlay_root()), 0, overlay);
        tx.change(left, id("left"));
        tx.change(right, id("right"));
        tx.change(editor, id("editor"));
        tx.change(
            editor,
            NodeChange::Semantics(Box::new(SemanticsSlot {
                relations: Relations { labelled_by: Some("text".into()), ..Default::default() },
                ..Default::default()
            })),
        );
        tx.change(text, id("text"));
        tx.change(scroll, id("scroll"));
        tx.change(picture, id("image"));
        tx.change(drawing_node, id("drawing"));
        tx.change(drawing_node, style([StyleProp::Width(Length::Px(40.0)), StyleProp::Height(Length::Px(30.0))]));
        tx.change(icon_node, id("icon"));
        tx.change(left, style([StyleProp::Width(Length::Px(170.0)), StyleProp::Direction(FlexDirection::Column)]));
        tx.change(right, style([StyleProp::Direction(FlexDirection::Column)]));
        tx.change(
            editor,
            style([
                StyleProp::Width(Length::Px(120.0)),
                StyleProp::Height(Length::Px(30.0)),
                StyleProp::PaddingLeft(Length::Px(3.0)),
                StyleProp::BorderTop(Length::Px(1.0)),
                StyleProp::BorderRight(Length::Px(2.0)),
                StyleProp::BorderBottom(Length::Px(3.0)),
                StyleProp::BorderLeft(Length::Px(4.0)),
                StyleProp::RingWidth(Length::Px(2.0)),
                StyleProp::RingOffset(Length::Px(3.0)),
            ]),
        );
        tx.change(
            scroll,
            style([
                StyleProp::Width(Length::Px(100.0)),
                StyleProp::Height(Length::Px(40.0)),
                StyleProp::Direction(FlexDirection::Column),
                StyleProp::OverflowY(Overflow::Scroll),
            ]),
        );
        tx.change(tall, style([StyleProp::Height(Length::Px(240.0)), StyleProp::Shrink(0.0)]));
        tx.change(overlay, style([StyleProp::Width(Length::Px(40.0)), StyleProp::Height(Length::Px(40.0))]));
        tx.change(
            overlay,
            NodeChange::Overlay(OverlaySpec {
                target: Some((octara_model::overlay::AnchorTarget::Node(editor.into()), Placement::BELOW)),
                ..Default::default()
            }),
        );
        let (receipt, authored) = tree.apply(tx.finish()).unwrap();
        delta.merge(authored);
        let right = receipt.node(right).unwrap();
        let editor = receipt.node(editor).unwrap();
        let overlay = receipt.node(overlay).unwrap();
        let text = receipt.node(text).unwrap();
        let scroll = receipt.node(scroll).unwrap();
        let field = |node: NodeId| tree.node_ref(node);
        let (editor_ref, right_ref, text_ref, drawing_ref) =
            (field(editor), field(right), field(text), field(receipt.node(drawing_node).unwrap()));
        delta.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(editor), visible: true }));
        settle(&mut engine, &mut tree, delta);

        let input = element(&surface, "editor").dyn_into::<HtmlInputElement>()?;
        let label = element(&surface, "text");
        let canvas = element(&surface, "image");
        let drawing_wrapper = element(&surface, "drawing");
        let bitmap = || drawing_wrapper.query_selector("canvas").unwrap().unwrap().dyn_into::<HtmlElement>().unwrap();
        let original_bitmap = bitmap();
        assert!(element(&surface, "icon").query_selector("svg path")?.is_some());
        let left_el = element(&surface, "left");
        let right_el = element(&surface, "right");
        let scroll_el = element(&surface, "scroll");
        let mounted_rect = engine.geometry(editor).unwrap();
        assert_eq!(mounted_rect.rect.w, 120.0);
        assert_eq!(mounted_rect.padding.left, 3.0);
        assert_eq!(mounted_rect.border, [1.0, 2.0, 3.0, 4.0].into());
        assert_eq!(input.style().get_property_value("outline-width")?, "2px");
        assert_eq!(input.style().get_property_value("outline-offset")?, "3px");
        assert_eq!(engine.scroll_range(scroll).unwrap().max.y, 200.0);
        assert_eq!(tree.overlay(overlay).unwrap().placed.unwrap().y, 34.0);
        assert_eq!(engine.hit_test(&tree, Point::new(10.0, 10.0)).unwrap().hit.node(), Some(editor));
        assert_eq!(pixel_at(&canvas, 0.0), vec![100, 30, 20, 255]);
        assert_eq!(pixel_at(&original_bitmap, 3.0), vec![51, 102, 153, 255]);
        assert_eq!(input.get_attribute("aria-labelledby"), Some(label.id()));
        let delta = tree.apply_runtime(RuntimeOp::RequestScroll { node: scroll, target: Vec2::new(0.0, 75.0) });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(scroll_el.scroll_top(), 75);

        input.set_selection_range_with_direction(1, 4, "backward")?;
        let selection = || (input.selection_start().unwrap(), input.selection_end().unwrap(), input.selection_direction().unwrap());
        let previous = selection();
        let mut tx = tree.transaction();
        tx.place(right_ref, 0, editor_ref);
        tx.change(text_ref, NodeChange::Text("tentative".into()));
        tx.change(drawing_ref, NodeChange::Canvas(drawing(0x993366)));
        let (_, mut delta) = tree.apply(tx.finish()).unwrap();
        delta.merge(tree.apply_runtime(RuntimeOp::RequestScroll { node: scroll, target: Vec2::new(0.0, 135.0) }));
        image.set(1, 1, [40, 50, 60, 255]);
        engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        assert_eq!(input.parent_element().as_ref(), Some(right_el.as_ref()));
        assert_eq!(label.text_content().as_deref(), Some("tentative"));
        assert_eq!(scroll_el.scroll_top(), 135);
        assert_eq!(pixel_at(&canvas, 0.0), vec![40, 50, 60, 255]);
        assert_eq!(pixel_at(&bitmap(), 3.0), vec![153, 51, 102, 255]);
        if !force_fallback {
            assert_eq!(document.active_element().as_ref(), Some(input.as_ref()));
            assert_eq!(selection(), previous);
        }
        engine.rollback();
        assert_eq!(document.active_element().as_ref(), Some(input.as_ref()));
        assert_eq!(selection(), previous);
        assert_eq!(input.parent_element().as_ref(), Some(left_el.as_ref()));
        assert_eq!(label.text_content().as_deref(), Some("initial"));
        assert_eq!(scroll_el.scroll_top(), 75);
        assert_eq!(pixel_at(&canvas, 0.0), vec![100, 30, 20, 255]);
        assert_eq!(pixel_at(&original_bitmap, 3.0), vec![51, 102, 153, 255]);
        assert_eq!(input.get_attribute("aria-labelledby"), Some(label.id()));
        assert!(bitmap().is_same_node(Some(&original_bitmap)));
        assert_eq!(engine.geometry(editor), Some(mounted_rect));
        settle(&mut engine, &mut tree, delta);
        assert_eq!(input.parent_element().as_ref(), Some(right_el.as_ref()));

        let mut tx = tree.transaction();
        tx.change(editor_ref, NodeChange::Disabled(true));
        tx.retire(text_ref, Retirement::Destroy);
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        assert!(!label.is_connected());
        assert!(element(&surface, "editor").get_attribute("aria-labelledby").is_none());
        if !force_fallback {
            assert_eq!(document.active_element().as_ref(), Some(input.as_ref()));
        }
        engine.rollback();
        assert!(!input.disabled());
        assert!(label.is_connected());
        assert_eq!(document.active_element().as_ref(), Some(input.as_ref()));

        engine.set_measure_mode(MeasureMode::VerifyFullCssom);
        let mut cssom = input.get_attribute("style").unwrap();
        cssom.push_str("width:111px;border-left-width:7px;");
        input.set_attribute("style", &cssom)?;
        engine.project(&tree, &TreeDelta::default(), VIEWPORT, 0.0).unwrap();
        let changed = engine.geometry(editor).unwrap();
        assert_eq!((changed.rect.w, changed.border.left), (111.0, 7.0));
        engine.rollback();

        if force_fallback {
            Reflect::set(&root_proto, &"moveBefore".into(), &move_before)?;
        }
        drop(engine);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub async fn run_dom_exits(reduced: bool) -> Result<(), JsValue> {
        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;margin:20px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 8], 1), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let pixels = |n| Length::Px(n);
        let root = tree.node_ref(tree.root());
        tx.change(root, style([StyleProp::Direction(FlexDirection::Column)]));
        let cases = [
            ("fade", Curve::Fade(0.3), Display::Flex, 1.0),
            ("zero", Curve::Fade(0.0), Display::Flex, 1.0),
            ("hidden", Curve::Fade(0.3), Display::None, 1.0),
            ("transparent", Curve::Fade(0.3), Display::Flex, 0.0),
            ("clipped", Curve::Fade(0.3), Display::Flex, 1.0),
        ];
        let mut nodes = Vec::new();
        for (index, (name, curve, display, opacity)) in cases.into_iter().enumerate() {
            let node = tx.create(NodeSpec::new(NodeContent::Box));
            tx.change(node, NodeChange::TestId(Some(name.into())));
            let mut properties = vec![
                StyleProp::Width(pixels(100.0)),
                StyleProp::Height(pixels(30.0)),
                StyleProp::OpacityMotion(curve),
                StyleProp::Opacity(opacity),
                StyleProp::Display(display),
            ];
            if name == "clipped" {
                properties.extend([StyleProp::Position(Position::Absolute), StyleProp::Top(pixels(2000.0))]);
            }
            tx.change(node, style(properties));
            tx.place(root, index, node);
            nodes.push(node);
        }
        let survivor = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(survivor, NodeChange::TestId(Some("survivor".into())));
        tx.change(survivor, style([StyleProp::Width(pixels(100.0)), StyleProp::Height(pixels(30.0))]));
        tx.place(root, nodes.len(), survivor);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        settle(&mut engine, &mut tree, delta);

        let els: Vec<_> = cases.into_iter().map(|(name, ..)| element(&surface, name)).collect();
        let fade = &els[0];
        let survivor_el = element(&surface, "survivor");
        let original_style = fade.get_attribute("style");
        let rect = fade.get_bounding_client_rect();
        let retired: Vec<_> = nodes.iter().map(|&node| receipt.node(node).unwrap()).collect();
        let mut tx = tree.transaction();
        for &node in &retired {
            tx.retire(tree.node_ref(node), Retirement::VisibleExit);
        }
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        let tickets: Vec<_> = retired.iter().map(|&node| tree.pending_exit(node)).collect();
        assert!(tickets[1].is_none(), "zero-duration exit is destroyed in the transaction");
        let check_instant = |feedback: &octara::runtime::engine::ProjectFeedback, offset| {
            assert_eq!(feedback.replies.len(), tickets[offset..].iter().flatten().count());
            for &ticket in tickets[offset..].iter().flatten() {
                assert!(feedback.replies.contains(&HostReply::ExitFinished(ticket)));
            }
            assert!(els[offset..].iter().all(|element| element.get_client_rects().length() == 0));
        };
        let feedback = engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        check_instant(&feedback, usize::from(!reduced));
        assert_eq!(survivor_el.get_bounding_client_rect().y(), rect.y());
        if !reduced {
            assert!(fade.is_connected());
            assert!(fade.get_attribute("inert").is_some());
            assert_eq!(fade.get_attribute("aria-hidden").as_deref(), Some("true"));
            assert_eq!(fade.get_bounding_client_rect().x(), rect.x());
            assert_eq!(fade.get_bounding_client_rect().y(), rect.y());
            let point = Point::new(1.0, 1.0);
            assert_eq!(engine.hit_test(&tree, point).and_then(|target| target.hit.node()), receipt.node(survivor));
        }
        engine.rollback();
        assert_eq!(fade.get_attribute("style"), original_style);
        assert!(fade.get_attribute("inert").is_none());
        assert!(els.iter().all(|element| element.is_connected()));
        let animations = Reflect::get(fade, &"getAnimations".into())?.dyn_into::<Function>()?;
        let feedback = engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        check_instant(&feedback, usize::from(!reduced));
        let performance = browser.performance().unwrap();
        let start = performance.now();
        engine.commit();
        let native = animations.call0(fade)?.dyn_into::<Array>()?;
        if reduced {
            assert!(feedback.next_deadline.is_none());
            assert_eq!(native.length(), 0);
        } else {
            let duration = feedback.next_deadline.unwrap().as_secs_f64();
            assert!((duration - 0.3).abs() < 0.001);
            assert_eq!(native.length(), 1);
            let animation = native.get(0);
            let opacity =
                || browser.get_computed_style(fade).unwrap().unwrap().get_property_value("opacity").unwrap().parse::<f64>().unwrap();
            Reflect::set(&animation, &"currentTime".into(), &JsValue::from_f64(150.0))?;
            assert!((opacity() - 0.5).abs() < 0.02);
            Reflect::set(&animation, &"currentTime".into(), &JsValue::from_f64(0.0))?;
            let beyond_prediction = duration + 0.01;
            let early = engine.project(&tree, &TreeDelta::default(), VIEWPORT, beyond_prediction).unwrap();
            assert!(early.replies.is_empty());
            assert!(early.next_deadline.unwrap().as_secs_f64() > beyond_prediction);
            assert!(fade.is_connected());
            engine.commit();
            JsFuture::from(Reflect::get(&animation, &"finished".into())?.dyn_into::<Promise>()?).await?;
            assert!(performance.now() - start >= 250.0);
            assert_eq!(opacity(), 0.0);
            let now = ((performance.now() - start) / 1000.0).max(beyond_prediction);
            let done = engine.project(&tree, &TreeDelta::default(), VIEWPORT, now).unwrap();
            assert_eq!(done.replies, [HostReply::ExitFinished(tickets[0].unwrap())]);
            assert!(done.next_deadline.is_none());
        }
        let mut finished = TreeDelta::default();
        for ticket in tickets.into_iter().flatten() {
            finished.merge(tree.apply_runtime(RuntimeOp::FinishExit(ticket)));
            assert!(tree.node(ticket.node).is_none());
        }
        settle(&mut engine, &mut tree, finished);
        assert!(els.iter().all(|element| !element.is_connected()));
        drop(engine);
        surface.remove();
        nested_exit_and_reopen(reduced)?;
        Ok(())
    }

    fn nested_exit_and_reopen(reduced: bool) -> Result<(), JsValue> {
        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 7], 2), Theme::default());
        let mut initial = tree.initial_delta();
        let mut tx = tree.transaction();
        let owner = tx.create(NodeSpec::new(NodeContent::Box));
        let parent = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        let overlay = |owner| NodeChange::Overlay(OverlaySpec { mount: Some((owner, 1)), ..Default::default() });
        let fade =
            |seconds| style([StyleProp::Width(100.into()), StyleProp::Height(40.into()), StyleProp::OpacityMotion(Curve::Fade(seconds))]);
        tx.change(parent, NodeChange::TestId(Some("barrier".into())));
        tx.change(child, NodeChange::TestId(Some("participant".into())));
        tx.change(parent, fade(0.1));
        tx.change(child, fade(0.5));
        tx.change(parent, overlay(owner.into()));
        tx.place(tree.node_ref(tree.root()), 0, owner);
        tx.place(tree.node_ref(tree.overlay_root()), 0, parent);
        tx.place(parent, 0, child);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        initial.merge(mounted);
        let [owner, parent, child] = [owner, parent, child].map(|node| receipt.node(node).unwrap());
        settle(&mut engine, &mut tree, initial);
        let old = element(&surface, "barrier");
        let inner = element(&surface, "participant");

        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(parent), Retirement::VisibleExit);
        let (_, closing) = tree.apply(tx.finish()).unwrap();
        let [parent_exit, child_exit] = [parent, child].map(|node| tree.pending_exit(node).unwrap());
        let expected = [HostReply::ExitFinished(parent_exit), HostReply::ExitFinished(child_exit)];
        let feedback = engine.project(&tree, &closing, VIEWPORT, 0.0).unwrap();
        engine.commit();
        assert!(!tree.contains_target(tree.guard(child)));
        if reduced {
            assert_eq!(feedback.replies, expected);
            assert!(feedback.next_deadline.is_none());
        } else {
            assert!(feedback.replies.is_empty());
            assert!(feedback.next_deadline.is_some());
            assert_eq!(old.parent_element(), inner.parent_element());
            assert!(inner.has_attribute("inert"));
            let animation = |el: &HtmlElement| -> Result<JsValue, JsValue> {
                let animations = Reflect::get(el, &"getAnimations".into())?.dyn_into::<Function>()?;
                let list = animations.call0(el)?.dyn_into::<Array>()?;
                assert_eq!(list.length(), 1);
                Ok(list.get(0))
            };
            let parent_animation = animation(&old)?;
            let child_animation = animation(&inner)?;
            let finish = Reflect::get(&parent_animation, &"finish".into())?.dyn_into::<Function>()?;
            finish.call0(&parent_animation)?;
            Reflect::set(&child_animation, &"currentTime".into(), &JsValue::from_f64(150.0))?;
            let feedback = engine.project(&tree, &TreeDelta::default(), VIEWPORT, 0.15).unwrap();
            engine.commit();
            assert_eq!(feedback.replies, [HostReply::ExitFinished(parent_exit)]);
            assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
            assert!(old.is_connected() && inner.is_connected());
            assert!(tree.node(parent).is_some());
        }

        let mut tx = tree.transaction();
        let replacement = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(replacement, NodeChange::TestId(Some("barrier".into())));
        tx.change(replacement, overlay(tree.node_ref(owner)));
        tx.change(replacement, fade(0.1));
        tx.place(tree.node_ref(tree.overlay_root()), 0, replacement);
        let (receipt, reopened) = tree.apply(tx.finish()).unwrap();
        let replacement = receipt.node(replacement).unwrap();
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
        assert!(tree.apply_runtime(RuntimeOp::FinishExit(child_exit)).is_empty());
        let feedback = engine.project(&tree, &reopened, VIEWPORT, 0.16).unwrap();
        engine.commit();
        assert!(feedback.replies.is_empty());
        assert!(feedback.next_deadline.is_none());
        assert!(!old.is_connected() && !inner.is_connected());
        assert!(!element(&surface, "barrier").has_attribute("inert"));
        assert!(tree.contains_target(tree.guard(replacement)));
        drop(engine);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_transitions() -> Result<(), JsValue> {
        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 9], 2), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let node = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(node, NodeChange::TestId(Some("transition".into())));
        tx.change(node, style([StyleProp::ColorMotion(Curve::Fade(0.15))]));
        tx.place(tree.node_ref(tree.root()), 0, node);
        let (_, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        settle(&mut engine, &mut tree, delta);

        let node = element(&surface, "transition");
        let properties = || browser.get_computed_style(&node).unwrap().unwrap().get_property_value("transition-property").unwrap();
        let color_properties = properties();
        assert_eq!(color_properties, "color, background-color, border-color, box-shadow, outline-color");
        engine.set_reduced_motion(true);
        engine.project(&tree, &TreeDelta::default(), VIEWPORT, 0.0).unwrap();
        assert_eq!(properties(), "none");
        engine.rollback();
        assert_eq!(properties(), color_properties);
        settle(&mut engine, &mut tree, TreeDelta::default());
        assert_eq!(properties(), "none");
        engine.set_reduced_motion(false);
        settle(&mut engine, &mut tree, TreeDelta::default());
        assert_eq!(properties(), color_properties);
        drop(engine);
        surface.remove();
        Ok(())
    }

    #[wasm_bindgen]
    pub fn run_dom_measurements(clear_counts: &Function, reads: &Function) -> Result<(), JsValue> {
        let document = web_sys::window().unwrap().document().unwrap();
        let surface = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        surface.set_attribute("style", "width:400px;height:300px;")?;
        document.body().unwrap().append_child(&surface)?;
        let mut engine = DomEngine::new(surface.clone())?;
        let mut tree = Tree::new(TreeId::new([4, 9], 1), Theme::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let root = tree.node_ref(tree.root());
        let mut ports = Vec::new();
        let mut contents = Vec::new();
        for (index, name) in ["changed-port", "unrelated-port"].into_iter().enumerate() {
            let port = tx.create(NodeSpec::new(NodeContent::Box));
            tx.change(port, NodeChange::TestId(Some(name.into())));
            tx.change(
                port,
                style([
                    StyleProp::OverflowY(Overflow::Scroll),
                    StyleProp::Direction(FlexDirection::Column),
                    StyleProp::Width(Length::Px(170.0)),
                    StyleProp::Height(Length::Px(70.0)),
                ]),
            );
            tx.place(root, index, port);
            let text = tx.create(NodeSpec::new(NodeContent::Text("scope".into())));
            let text_id = if index == 0 { "edited-text" } else { "unrelated-text" };
            tx.change(text, NodeChange::TestId(Some(text_id.into())));
            tx.place(port, 0, text);
            let filler = tx.create(NodeSpec::new(NodeContent::Box));
            let filler_id = if index == 0 { "untouched-row" } else { "unrelated-row" };
            tx.change(filler, NodeChange::TestId(Some(filler_id.into())));
            tx.change(filler, style([StyleProp::Height(Length::Px(140.0)), StyleProp::Shrink(0.0)]));
            tx.place(port, 1, filler);
            ports.push(port);
            contents.push((text, filler));
        }
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let port = receipt.node(ports[0]).unwrap();
        let port_ref = tree.node_ref(port);
        let (text, filler) = contents[0];
        let text_ref = tree.node_ref(receipt.node(text).unwrap());
        let filler_ref = tree.node_ref(receipt.node(filler).unwrap());
        delta.merge(mounted);
        settle(&mut engine, &mut tree, delta);
        let port_element = element(&surface, "changed-port");
        let read_count = |id: &str| reads.call1(&JsValue::UNDEFINED, &id.into()).unwrap().as_f64().unwrap() as u32;
        let check_counts = |name| {
            // Three measured nodes and the surface origin.
            assert_eq!(read_count(""), 4);
            assert_eq!(read_count(name), 1);
            assert_eq!(read_count("changed-port"), 1);
            for id in ["untouched-row", "unrelated-port", "unrelated-text", "unrelated-row"] {
                assert_eq!(read_count(id), 0, "unexpectedly measured {id} during {name}");
            }
        };

        let mut tx = tree.transaction();
        tx.change(text_ref, NodeChange::Text("edited scope".into()));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        clear_counts.call0(&JsValue::UNDEFINED)?;
        settle(&mut engine, &mut tree, delta);
        check_counts("edited-text");

        let mut tx = tree.transaction();
        let node = tx.create(NodeSpec::new(NodeContent::Text("inserted".into())));
        tx.change(node, NodeChange::TestId(Some("inserted-row".into())));
        tx.place(port_ref, 1, node);
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        clear_counts.call0(&JsValue::UNDEFINED)?;
        settle(&mut engine, &mut tree, delta);
        check_counts("inserted-row");
        assert_eq!(read_count("edited-text"), 0);

        let delta = tree.apply_runtime(RuntimeOp::RequestScroll { node: port, target: Vec2::new(0.0, 65.0) });
        settle(&mut engine, &mut tree, delta);
        assert_eq!(port_element.scroll_top(), 65);
        let mut tx = tree.transaction();
        tx.change(filler_ref, style([StyleProp::Height(Length::Px(1.0)), StyleProp::Shrink(0.0)]));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
        assert!(port_element.scroll_top() < 65);
        engine.rollback();
        assert_eq!(port_element.scroll_top(), 65);
        drop(engine);
        surface.remove();
        Ok(())
    }
}
