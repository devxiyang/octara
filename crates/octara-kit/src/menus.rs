//! Styled command menus, with Headless navigation and dismissal.

use crate::{PopupOptions, TOUCH_TARGET, parts::popup_surface};
use octara::prelude::*;
use octara_headless::{Choice, ContextMenuState, MenuState};

/// Opens a command menu from an authored button. The menu is sampled once per opening.
/// Actions dispatch their IDs as commands after dismissal restores focus.
#[ui]
pub fn menu_button(
    b: &mut BuildCx<'_>,
    id: &str,
    open: Binding<bool>,
    options: PopupOptions,
    trigger: El,
    menu: impl Fn(&mut ReadCx<'_>) -> Menu + 'static,
) -> El {
    let (state, snapshot) = menu_state(b, open, menu);
    let trigger = state.button(trigger.test_id(id.to_owned()).preserve_focus_on_press());
    let (trigger, geometry) = options.anchor(b, trigger);
    let id = id.to_owned();
    state.overlay(trigger, options.placement, options.anchor_target, move |b| {
        let menu = b.sample(snapshot);
        let panel = options.surface(menu_surface(), geometry);
        menu_panel(b, panel, &id, state, open, menu)
    })
}

/// Opens the same command menu at a secondary press, or from the keyboard or accessibility action.
#[ui]
pub fn context_menu(
    b: &mut BuildCx<'_>,
    id: &str,
    open: Binding<bool>,
    options: PopupOptions,
    trigger: El,
    menu: impl Fn(&mut ReadCx<'_>) -> Menu + 'static,
) -> El {
    let (state, snapshot) = menu_state(b, open, menu);
    let context = ContextMenuState::new(b, state);
    let id = id.to_owned();
    let (trigger, geometry) = options.anchor(b, trigger.test_id(id.clone()));
    context.trigger(trigger, options.placement, options.anchor_target, move |b| {
        let menu = b.sample(snapshot);
        let panel = options.surface(menu_surface(), geometry);
        menu_panel(b, panel, &id, state, open, menu)
    })
}

fn menu_state(
    b: &mut BuildCx<'_>,
    open: Binding<bool>,
    menu: impl Fn(&mut ReadCx<'_>) -> Menu + 'static,
) -> (MenuState<String>, ReadSignal<Menu>) {
    let mut cached = None;
    let snapshot = b.memo(move |r| {
        if open.get(r) {
            cached.get_or_insert_with(|| menu(r)).clone()
        } else {
            cached = None;
            Menu::default()
        }
    });
    let rows = b.derive_list(move |r| r.with(snapshot, |menu| choices(menu).collect()));
    (MenuState::new(b, open, rows), snapshot)
}

fn choices(menu: &Menu) -> impl Iterator<Item = (String, Choice)> + '_ {
    menu.items.iter().enumerate().filter_map(|(index, item)| match item {
        MenuItem::Action { id, label, enabled, .. } => Some((action_key(id), Choice::new(label.clone()).disabled(!enabled))),
        MenuItem::Submenu { menu, enabled } => Some((submenu_key(index), Choice::new(menu.name.clone()).disabled(!enabled))),
        MenuItem::Separator => None,
    })
}

fn action_key(id: &str) -> String {
    format!("action-{id}")
}

fn submenu_key(index: usize) -> String {
    format!("submenu-{index}")
}

fn menu_surface() -> El {
    popup_surface(col()).p(sp(1.0)).gap(sp(0.5)).min_w(144).max_h(300).scroll_y()
}

fn menu_panel(b: &mut BuildCx<'_>, panel: El, id: &str, state: MenuState<String>, root: Binding<bool>, menu: Menu) -> El {
    let has_enabled = choices(&menu).any(|(_, choice)| !choice.disabled);
    let checkable = menu.items.iter().any(|item| matches!(item, MenuItem::Action { checked: Some(_), .. }));
    let panel = panel.test_id(format!("{id}-menu")).label(menu.name);
    let panel = if has_enabled { panel } else { panel.focusable() };
    state.items(panel).children(menu.items.into_iter().enumerate().map(|(index, item)| match item {
        MenuItem::Action { id: command, label, checked, shortcut, .. } => {
            let key = action_key(&command);
            let item = menu_row(format!("{id}-{key}"), checkable, checked.unwrap_or_default())
                .child(text(label).grow(1.0))
                .children(shortcut.map(|label| text(label).text_color(ColorToken::TextMuted)))
                .on_press(move |w| {
                    if !w.read(|r| root.get(r)) {
                        w.request_command_after_commit(&command);
                    }
                });
            let item = state.item(item, &key);
            if let Some(checked) = checked { item.role(Role::MenuItemCheckbox).checked(checked) } else { item }
        }
        MenuItem::Submenu { menu, .. } => {
            let key = submenu_key(index);
            let item_id = format!("{id}-{key}");
            let rows = b.list_from(choices(&menu));
            let submenu = MenuState::new(b, octara_headless::Control::Uncontrolled(false), rows);
            let item = menu_row(item_id.clone(), checkable, false)
                .child(text(menu.name.clone()).grow(1.0))
                .child(icon(octara_icons::CHEVRON_RIGHT.into()).a11y_hidden());
            state.submenu(item, &key, submenu, move |b, submenu| menu_panel(b, menu_surface(), &item_id, submenu, root, menu.clone()))
        }
        MenuItem::Separator => state.separator(el().h(1).bg(ColorToken::Border)),
    }))
}

fn menu_row(id: String, checkable: bool, checked: bool) -> El {
    row()
        .test_id(id)
        .align(AlignItems::Center)
        .gap(sp(2.0))
        .px(sp(2.0))
        .py(sp(1.0))
        .rounded(RadiusToken::Sm)
        .text_size(FontSizeToken::Sm)
        .when(When::Touch, |p| p.min_h(TOUCH_TARGET))
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::FOCUSED, |p| p.bg(ColorToken::SurfacePressed))
        .when(State::DISABLED, |p| p.opacity(0.5))
        .children(checkable.then(|| {
            let mark = if checked { icon(octara_icons::CHECK.into()) } else { el() };
            mark.size(14).shrink(0.0).a11y_hidden()
        }))
}
