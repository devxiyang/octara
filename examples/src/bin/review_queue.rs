//! A small application assembled from the public view and state APIs.

use octara::prelude::*;
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

fn action(label: impl IntoText, press: impl Fn() + 'static) -> El {
    text(label)
        .role(Role::Button)
        .focusable()
        .min_h(36)
        .px_3()
        .py_2()
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .hover(|p| p.bg(Token::SurfaceHover))
        .focus_visible(|p| p.border_color(Token::Focus))
        .on_press(press)
}

#[ui]
fn approval_form(id: &'static str, decisions: Decisions) -> El {
    let draft = state("opinion", String::new);
    let attempted = state("attempted", || false);
    let invalid = memo(move || attempted.get() && draft.with(|note| note.trim().is_empty()));
    let approve = move || {
        let note = draft.with(|note| note.trim().to_owned());
        if note.is_empty() {
            attempted.set(true);
            return;
        }
        decisions.update(|saved| {
            saved.insert(id, note);
        });
    };
    col()
        .gap_2()
        .child(text("审阅意见").id("review-note-label").font_semibold())
        .child(
            text_input()
                .id("review-note")
                .labelled_by("review-note-label")
                .required(true)
                .invalid(invalid)
                .description(move || if invalid.get() { NOTE_REQUIRED.to_owned() } else { String::new() })
                .value(draft)
                .on_input(move |text| draft.set(text))
                .on_submit(approve)
                .multiline()
                .rows(3, 4)
                .w_full()
                .p_3()
                .bg(Token::Background)
                .rounded_md()
                .border()
                .border_color(Token::Border)
                .focus_visible(|p| p.border_color(Token::Focus)),
        )
        .child(show(move || invalid.get(), || text(NOTE_REQUIRED).id("review-note-error").role(Role::Status).live(Live::Polite)))
        .child(action("通过", approve).id("review-approve").self_start())
}

#[ui]
fn review_detail(id: &'static str, decisions: Decisions) -> El {
    let Some(document) = DOCUMENTS.iter().find(|document| document.id == id) else {
        return col().p_4().child(text("此列表没有文档。").id("review-empty"));
    };
    let approved = memo(move || decisions.with(|saved| saved.contains_key(id)));
    col()
        .gap_3()
        .p_4()
        .child(text(document.title).id("review-detail-title").text_xl().font_semibold())
        .child(text(document.brief).text_color(Token::TextMuted))
        .child(show(move || !approved.get(), move || approval_form(id, decisions)))
        .child(show(
            move || approved.get(),
            move || {
                col()
                    .gap_2()
                    .child(text("已保存的审阅意见").font_semibold())
                    .child(text(move || decisions.with(|saved| saved.get(id).cloned().unwrap_or_default())).id("review-saved-note"))
            },
        ))
}

#[ui]
fn view() -> El {
    let decisions = state("decisions", HashMap::new);
    let approved = state("approved-feed", || false);
    let selection = state("selected-document", || None::<&'static str>);
    let visible = memo(move || {
        let approved = approved.get();
        decisions.with(|saved| {
            DOCUMENTS.iter().copied().filter(|doc| saved.contains_key(doc.id) == approved).map(|doc| (doc.id, doc)).collect::<Vec<_>>()
        })
    });
    let active = memo(move || {
        let selection = selection.get();
        visible.with(|rows| selection.filter(|id| rows.iter().any(|(key, _)| key == id)).or_else(|| rows.first().map(|(key, _)| *key)))
    });
    let list =
        col().id("review-list").role(Role::List).label("文档").gap_2().w_full().screen(Breakpoint::Medium, |p| p.w(220)).child(each(
            move || visible.get(),
            move |id, item| {
                let id = *id;
                col().role(Role::ListItem).child(
                    action(move || item.with(|doc| doc.title.to_owned()), move || selection.set(Some(id)))
                        .id(format!("review-item-{id}"))
                        .selected(move || active.get() == Some(id))
                        .style_when(State::SELECTED, |p| p.border_color(Token::Accent).font_semibold()),
                )
            },
        ));
    col()
        .size_full()
        .p_5()
        .gap_4()
        .bg(Token::Background)
        .text_color(Token::Text)
        .overflow_y_scroll()
        .child(text("资料审阅").text_xl().font_semibold())
        .child(
            row()
                .gap_2()
                .child(action("待审", move || approved.set(false)).id("review-pending").selected(move || !approved.get()))
                .child(action("已通过", move || approved.set(true)).id("review-approved").selected(approved))
                .child(
                    text(move || decisions.with(|saved| format!("{} 待审 · {} 已通过", DOCUMENTS.len() - saved.len(), saved.len())))
                        .id("review-counts")
                        .text_sm()
                        .text_color(Token::TextMuted)
                        .role(Role::Status)
                        .live(Live::Polite)
                        .self_center(),
                ),
        )
        .child(
            col().gap_4().screen(Breakpoint::Medium, |p| p.flex_row()).child(list).child(
                col()
                    .flex_1()
                    .min_w(0)
                    .rounded_lg()
                    .border()
                    .border_color(Token::Border)
                    .bg(Token::Surface)
                    .child(keyed(move || active.get().unwrap_or(""), move |id| review_detail(id, decisions))),
            ),
        )
}

fn main() {
    octara_shell::run("资料审阅", view);
}
