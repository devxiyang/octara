//! Numeric drafts, a slider, progress, and menu actions share application state.

use super::{menus::action, section};
use octara::prelude::*;
use octara_kit::headless::range::{Range, parse_number};
use octara_kit::{ButtonVariant, FieldInfo, PopupOptions, Size as KitSize, button, field, menu_button, number_field, progress, slider};

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let range = Range::new(0.0, 1.0, 0.05);
    let value = cx.signal(0.35);
    let draft = cx.signal("0.35".to_owned());
    let disabled = cx.signal(false);
    let unknown = cx.signal(false);
    let menu = cx.signal(false);
    let status = cx.signal("尚未提交。".to_owned());
    let error = cx.memo(move |r| {
        (!parse_number(&r.get(draft)).is_some_and(|value| (0.0..=1.0).contains(&value))).then(|| "请输入 0 到 1 之间的数字。".to_owned())
    });
    let info = cx.memo(move |r| FieldInfo {
        description: "范围 0–1，步长 0.05；Enter 或离开输入框时提交。".into(),
        error: r.get(error),
        required: true,
    });
    let percent = cx.memo(move |r| (!r.get(unknown)).then(|| r.get(value) * 100.0));
    let publish = move |w: &mut WriteCx<'_>| {
        if let Some(next) = parse_number(&w.get(draft)).filter(|number| (0.0..=1.0).contains(number)) {
            w.set(value, next);
            w.set(status, format!("已提交：{next}"));
        }
    };
    let commit = move |w: &mut WriteCx<'_>| {
        if w.get(error).is_none()
            && let Some(value) = parse_number(&w.get(draft))
        {
            w.set(draft, range.normalize_text(value));
            publish(w);
        }
    };
    let set_value = move |w: &mut WriteCx<'_>, next: f64| {
        w.set(value, next);
        w.set(draft, next.to_string());
        w.set(status, format!("滑块：{next}"));
    };
    let slider_value = cx.binding(move |r| r.get(value), set_value);
    let number = number_field(cx, draft.into(), 0.0, 1.0, 0.05).w(rel(1.0)).disabled(disabled).on_submit(publish).on_blur(publish);
    section("Numbers, progress and menus")
        .w(rel(1.0))
        .on_command("controls.reset", move |w| set_value(w, 0.35))
        .on_command("controls.toggle_disabled", move |w| w.update(disabled, |value| *value = !*value))
        .on_command("controls.minimum", move |w| set_value(w, 0.0))
        .on_command("controls.maximum", move |w| set_value(w, 1.0))
        .child(field(cx, "controls-number", "强度", number, info))
        .child(slider(cx, "controls-slider", slider_value, 0.0, 1.0, 0.05).label("强度滑块").w(rel(1.0)).disabled(disabled))
        .child(progress(cx, percent, 100.0).test_id("controls-progress").label("示例进度").w(rel(1.0)))
        .child(
            row()
                .wrap(FlexWrap::Wrap)
                .gap(sp(2.0))
                .child(
                    button("提交数字", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("controls-commit")
                        .disabled(move |r: &mut ReadCx<'_>| r.get(disabled) || r.get(error).is_some())
                        .on_press(commit),
                )
                .child(
                    button("切换未知进度", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("controls-progress-mode")
                        .on_press(move |w| w.update(unknown, |value| *value = !*value)),
                )
                .child(menu_button(
                    cx,
                    "controls-menu",
                    menu.into(),
                    PopupOptions::default(),
                    button("操作菜单", KitSize::Md, ButtonVariant::Secondary),
                    move |r| Menu {
                        name: "数值操作".into(),
                        items: vec![
                            action("controls.reset", "重置", None),
                            action("controls.toggle_disabled", "禁用编辑", Some(r.get(disabled))),
                            MenuItem::Separator,
                            action("controls.minimum", "最小值", None),
                            action("controls.maximum", "最大值", None),
                        ],
                    },
                )),
        )
        .child(text(status).test_id("controls-status").role(Role::Status).live(Live::Polite).text_size(FontSizeToken::Sm))
}
