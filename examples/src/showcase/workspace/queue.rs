use super::{panel, subheading};
use crate::showcase::menus::action;
use octara::prelude::*;
use octara_icons::{CHECK, ELLIPSIS, GRIP_VERTICAL};
use octara_kit::{PopupOptions, ToastOptions, ToastQueue, context_menu, headless, icon_button};

const CHECKS: [(&str, &str); 3] = [("smoke", "Run smoke tests"), ("notes", "Review release notes"), ("sign-off", "Collect sign-off")];

pub(super) fn view(b: &mut BuildCx<'_>, toasts: ToastQueue<String>) -> El {
    let checks = b.list_from(CHECKS);
    let notify = b.signal(true);
    let open = b.signal(false);
    let actions = context_menu(
        b,
        "queue-actions",
        open.into(),
        PopupOptions::default(),
        icon_button(ELLIPSIS, "Queue actions").on_press(move |w| w.set(open, true)),
        |_| Menu { name: "Queue actions".into(), items: vec![action("queue.reset", "Reset review order", None)] },
    );
    panel()
        .test_id("review-queue")
        .p(sp(3.0))
        .gap(sp(2.0))
        .on_command("queue.reset", move |w| {
            for (position, (key, _)) in CHECKS.iter().enumerate() {
                w.move_row(checks, key, position);
            }
            toasts.push(w, "Review order reset".into(), ToastOptions::default());
        })
        .child(
            row()
                .align(AlignItems::Center)
                .justify(JustifyContent::SpaceBetween)
                .wrap(FlexWrap::Wrap)
                .gap(sp(2.0))
                .child(subheading("Review queue"))
                .child(row().align(AlignItems::Center).gap(sp(2.0)).child(notify_on_move(notify)).child(actions)),
        )
        .child(text("Drag to prioritize, or use Space, ↑/↓ and Enter.").text_size(FontSizeToken::Xs).text_color(ColorToken::TextMuted))
        .child(col().role(Role::List).gap(sp(1.0)).child(each(checks, move |b, &key, item| {
            let label = b.sample(item);
            check_row(b, checks, key, label, notify, toasts)
        })))
}

fn check_row(
    b: &mut BuildCx<'_>,
    checks: StateList<&'static str, &'static str>,
    key: &'static str,
    label: &'static str,
    notify: Signal<bool>,
    toasts: ToastQueue<String>,
) -> El {
    const KIND: &str = "release-workspace.check";
    let hover = b.signal(None);
    row()
        .test_id(format!("queue-{key}"))
        .label(label)
        .role(Role::ListItem)
        .position(Position::Relative)
        .align(AlignItems::Center)
        .gap(sp(2.0))
        .px(sp(2.0))
        .min_h(32.0)
        .rounded(RadiusToken::Md)
        .border(1)
        .border_color(ColorToken::Border)
        .when(When::Touch, |p| p.min_h(44.0))
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus))
        .when(State::DRAGGING, |p| p.opacity(0.55))
        .when(State::DROP_HOVER, |p| p.bg(ColorToken::SurfacePressed))
        .draggable(DragPayload::new(KIND, key).preview_label(label))
        .drop_target([KIND], DropAxis::Vertical, move |w, drop| {
            let order = w.keys(checks);
            let Some(from) = order.iter().position(|candidate| Some(*candidate) == drop.data.as_str()) else { return };
            let anchor = order.iter().position(|candidate| *candidate == key).expect("the receiving check");
            if from == anchor {
                return;
            }
            let after = drop.position == DropPosition::After || (drop.position == DropPosition::Inside && from < anchor);
            let to = anchor - usize::from(from < anchor) + usize::from(after);
            if from != to {
                let source = order[from];
                w.move_row(checks, &source, to);
                if w.get(notify) {
                    let label = w.get_row(checks, &source).expect("the moved check");
                    toasts.push(w, format!("Moved \"{label}\" to position {}", to + 1), ToastOptions::default());
                }
            }
        })
        .on_drop_hover(move |w, event| {
            w.set_if_changed(hover, event.position);
        })
        .reorder_keys()
        .child(icon(GRIP_VERTICAL.into()).size(16.0).text_color(ColorToken::TextMuted).a11y_hidden())
        .child(text(label).text_size(FontSizeToken::Sm).select_none())
        .children([DropPosition::Before, DropPosition::After].map(|position| {
            let indicator = el()
                .position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .h(2.0)
                .rounded(RadiusToken::Md)
                .bg(ColorToken::Accent)
                .a11y_hidden()
                .style_with(move |r, p| p.display(if r.get(hover) == Some(position) { Display::Flex } else { Display::None }));
            if position == DropPosition::Before { indicator.top(0.0) } else { indicator.bottom(0.0) }
        }))
}

fn notify_on_move(notify: Signal<bool>) -> El {
    let mark = el()
        .size(18.0)
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .border(1)
        .rounded(RadiusToken::Sm)
        .style_with(move |r, p| {
            if r.get(notify) {
                p.border_color(ColorToken::Accent).bg(ColorToken::Accent)
            } else {
                p.border_color(ColorToken::Border).bg(ColorToken::Surface)
            }
        })
        .child(show(move |r| r.get(notify), |_| icon(CHECK.into()).size(12.0).text_color(ColorToken::AccentText)))
        .a11y_hidden();
    headless::checkbox::checkbox(
        row()
            .test_id("queue-notify")
            .label("Notify on move")
            .align(AlignItems::Center)
            .gap(sp(2.0))
            .px(sp(2.0))
            .min_h(28.0)
            .rounded(Radius::Full)
            .border(1)
            .border_color(ColorToken::Border)
            .when(When::Touch, |p| p.min_h(44.0))
            .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
            .when(State::FOCUS_VISIBLE, |p| p.border_color(ColorToken::Focus))
            .child(mark)
            .child(text("Notify on move").text_size(FontSizeToken::Xs).select_none()),
        notify.into(),
    )
}
