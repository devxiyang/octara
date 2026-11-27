//! A small application assembled from the public view and state APIs.

use octara::prelude::*;
use octara_examples::window_options;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Document {
    id: &'static str,
    title: &'static str,
    brief: &'static str,
}

const DOCUMENTS: [Document; 3] = [
    Document { id: "release", title: "发布说明", brief: "确认升级步骤、依赖版本和已知问题。" },
    Document { id: "navigation", title: "导航说明", brief: "确认页面入口清晰，键盘可以到达所有主要操作。" },
    Document { id: "handoff", title: "交接清单", brief: "确认维护人、运行步骤和待处理问题有明确记录。" },
];
const NOTE_REQUIRED: &str = "请填写审阅意见。";
type Decisions = Signal<HashMap<&'static str, String>>;

fn action(label: impl IntoText, press: impl Fn(&mut WriteCx<'_>) + 'static) -> El {
    text(label)
        .role(Role::Button)
        .focusable()
        .min_h(36)
        .px(sp(3.0))
        .py(sp(2.0))
        .rounded(RadiusToken::Md)
        .border(1)
        .border_color(ColorToken::Border)
        .bg(ColorToken::Surface)
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus))
        .on_press(press)
}

#[ui]
fn approval_form(cx: &mut BuildCx<'_>, id: &'static str, decisions: Decisions) -> El {
    let draft = cx.signal(String::new());
    let attempted = cx.signal(false);
    let invalid = cx.memo(move |r| r.get(attempted) && r.with(draft, |note| note.trim().is_empty()));
    let approve = move |w: &mut WriteCx<'_>| {
        let note = w.with(draft, |note| note.trim().to_owned());
        if note.is_empty() {
            w.set(attempted, true);
            return;
        }
        w.update(decisions, |saved| {
            saved.insert(id, note);
        });
    };
    col()
        .gap(sp(2.0))
        .child(text("审阅意见").test_id("review-note-label").text_weight(600))
        .child(
            textarea()
                .test_id("review-note")
                .labelled_by("review-note-label")
                .required(true)
                .invalid(invalid)
                .description(move |r: &mut ReadCx<'_>| if r.get(invalid) { NOTE_REQUIRED.to_owned() } else { String::new() })
                .value(draft)
                .on_input(move |w, text| w.set(draft, text.to_owned()))
                .on_submit(approve)
                .rows(3, 4)
                .w(rel(1.0))
                .p(sp(3.0))
                .bg(ColorToken::Background)
                .rounded(RadiusToken::Md)
                .border(1)
                .border_color(ColorToken::Border)
                .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus)),
        )
        .child(show(move |r| r.get(invalid), |_| text(NOTE_REQUIRED).test_id("review-note-error").role(Role::Status).live(Live::Polite)))
        .child(action("通过", approve).test_id("review-approve").align_self(AlignItems::Start))
}

#[ui]
fn review_detail(cx: &mut BuildCx<'_>, id: &'static str, decisions: Decisions) -> El {
    let Some(document) = DOCUMENTS.iter().find(|document| document.id == id) else {
        return col().p(sp(4.0)).child(text("此列表没有文档。").test_id("review-empty"));
    };
    let approved = cx.memo(move |r| r.with(decisions, |saved| saved.contains_key(id)));
    col()
        .gap(sp(3.0))
        .p(sp(4.0))
        .child(text(document.title).test_id("review-detail-title").text_size(FontSizeToken::Xl).text_weight(600))
        .child(text(document.brief).text_color(ColorToken::TextMuted))
        .child(show(move |r| !r.get(approved), move |b| approval_form(b, id, decisions)))
        .child(show(
            move |r| r.get(approved),
            move |_| {
                col().gap(sp(2.0)).child(text("已保存的审阅意见").text_weight(600)).child(
                    text(move |r: &mut ReadCx<'_>| r.with(decisions, |saved| saved.get(id).cloned().unwrap_or_default()))
                        .test_id("review-saved-note"),
                )
            },
        ))
}

#[ui]
fn view(cx: &mut BuildCx<'_>) -> El {
    let decisions = cx.signal(HashMap::new());
    let approved = cx.signal(false);
    let selection = cx.signal(None::<&'static str>);
    let visible = cx.memo(move |r| {
        let approved = r.get(approved);
        r.with(decisions, |saved| {
            DOCUMENTS.iter().copied().filter(|doc| saved.contains_key(doc.id) == approved).map(|doc| (doc.id, doc)).collect::<Vec<_>>()
        })
    });
    let active = cx.memo(move |r| {
        let selection = r.get(selection);
        r.with(visible, |rows| selection.filter(|id| rows.iter().any(|(key, _)| key == id)).or_else(|| rows.first().map(|(key, _)| *key)))
    });
    let list = col()
        .test_id("review-list")
        .role(Role::List)
        .label("文档")
        .gap(sp(2.0))
        .w(rel(1.0))
        .when(Breakpoint::Medium, |p| p.w(220))
        .children(DOCUMENTS.into_iter().map(|doc| {
            let id = doc.id;
            show(
                move |r| r.with(visible, |rows| rows.iter().any(|(key, _)| *key == id)),
                move |_| {
                    col().role(Role::ListItem).child(
                        action(doc.title, move |w| w.set(selection, Some(id)))
                            .test_id(format!("review-item-{id}"))
                            .selected(move |r: &mut ReadCx<'_>| r.get(active) == Some(id))
                            .when(State::SELECTED, |p| p.border_color(ColorToken::Accent).text_weight(600)),
                    )
                },
            )
        }));
    col()
        .size(rel(1.0))
        .p(sp(5.0))
        .gap(sp(4.0))
        .bg(ColorToken::Background)
        .text_color(ColorToken::Text)
        .overflow_y(Overflow::Scroll)
        .child(text("资料审阅").test_id("review-heading").role(Role::Heading).text_size(FontSizeToken::Xl).text_weight(600))
        .child(
            row()
                .gap(sp(2.0))
                .child(
                    action("待审", move |w| w.set(approved, false))
                        .test_id("review-pending")
                        .selected(move |r: &mut ReadCx<'_>| !r.get(approved)),
                )
                .child(action("已通过", move |w| w.set(approved, true)).test_id("review-approved").selected(approved))
                .child(
                    text(move |r: &mut ReadCx<'_>| {
                        r.with(decisions, |saved| format!("{} 待审 · {} 已通过", DOCUMENTS.len() - saved.len(), saved.len()))
                    })
                    .test_id("review-counts")
                    .text_size(FontSizeToken::Sm)
                    .text_color(ColorToken::TextMuted)
                    .role(Role::Status)
                    .live(Live::Polite)
                    .align_self(AlignItems::Center),
                ),
        )
        .child(
            col().gap(sp(4.0)).when(Breakpoint::Medium, |p| p.direction(FlexDirection::Row)).child(list).child(
                col()
                    .grow(1.0)
                    .min_w(0)
                    .rounded(RadiusToken::Lg)
                    .border(1)
                    .border_color(ColorToken::Border)
                    .bg(ColorToken::Surface)
                    .child(keyed(move |r| r.get(active).unwrap_or(""), move |b, id| review_detail(b, id, decisions))),
            ),
        )
}

fn main() -> Result<(), octara_host_native::ShellError> {
    octara_examples::native::run(window_options("资料审阅", 830.0, 650.0), view)
}
