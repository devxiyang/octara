//! Command menus, submenus, and pointer- or keyboard-invoked context menus.

use std::{collections::BTreeMap, fmt::Display, hash::Hash};

use octara::prelude::*;

use crate::{
    Control,
    checkbox::CheckValue,
    collection::{Choice, Collection, Navigation},
    id::Id,
};

/// Connects a menu section with its accessible heading.
#[derive(Clone, Copy)]
pub struct MenuSection(Id);

impl MenuSection {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for MenuSection {
    fn default() -> Self {
        Self(Id::new())
    }
}

#[derive(Clone, Copy)]
struct Submenu {
    open: Binding<bool>,
    blocked: bool,
}

#[derive(Clone, Copy)]
struct MenuRoot {
    open: Binding<bool>,
    scope: ScopeId,
    // An opener's business-key path survives retirement of the popup that built it.
    submenus: Signal<BTreeMap<Vec<String>, Submenu>>,
}

impl MenuRoot {
    fn authorize(self, write: &mut WriteCx<'_>, path: &[String]) {
        if write.with(self.submenus, |submenus| submenus.get(path).is_some_and(|submenu| submenu.blocked)) {
            write.update(self.submenus, |submenus| {
                submenus.remove(path);
            });
        }
    }

    fn close_descendants(self, write: &mut WriteCx<'_>, path: &[String]) {
        let descendants = write.with(self.submenus, |submenus| {
            submenus.iter().rev().filter(|(key, _)| key.starts_with(path)).map(|(key, submenu)| (key.clone(), *submenu)).collect::<Vec<_>>()
        });
        for (key, submenu) in descendants {
            if key.len() > path.len() && submenu.open.is_alive(write) && !submenu.blocked && write.read(|read| submenu.open.get(read)) {
                submenu.open.set(write, false);
            }
            // A controlled submenu can decline the reset. It still needs a fresh open request.
            let blocked = key.len() > path.len() && submenu.open.is_alive(write) && write.read(|read| submenu.open.get(read));
            write.update(self.submenus, |submenus| {
                if blocked {
                    submenus.insert(key, Submenu { blocked: true, ..submenu });
                } else {
                    submenus.remove(&key);
                }
            });
        }
    }
}

#[derive(Clone)]
struct Attachment {
    root: MenuRoot,
    path: Vec<String>,
}

/// Controls a menu whose interactive items are keyed by a reactive collection.
/// Keys must have distinct display forms within the collection.
pub struct MenuState<K, T = Choice> {
    open: Binding<bool>,
    root: MenuRoot,
    path: Option<Signal<Vec<String>>>,
    attachment: Signal<Option<Attachment>>,
    entry: Signal<Option<Navigation>>,
    collection: Collection<K, T>,
    id: Id,
}

impl<K, T> Copy for MenuState<K, T> {}

impl<K, T> Clone for MenuState<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + Display + 'static> MenuState<K> {
    pub fn new(build: &mut BuildCx<'_>, open: impl Into<Control<bool>>, items: impl IntoReadList<K, Choice>) -> Self {
        let collection = Collection::choices(build, items);
        Self::with_collection(build, open, collection)
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, T: 'static> MenuState<K, T> {
    pub fn with_collection(build: &mut BuildCx<'_>, open: impl Into<Control<bool>>, collection: Collection<K, T>) -> Self {
        let open = open.into().bind(build);
        let root = MenuRoot { open, scope: build.scope(), submenus: build.signal(BTreeMap::new()) };
        let attachment = build.signal(None::<Attachment>);
        let entry = build.signal(None);
        build.watch(
            move |read| open.get(read),
            move |write, open, _| {
                if !open {
                    collection.set_active(write, None);
                    collection.reset_typeahead(write);
                    write.set_if_changed(entry, None);
                    if let Some(attachment) = write.get(attachment) {
                        if write.is_scope_alive(attachment.root.scope) {
                            attachment.root.close_descendants(write, &attachment.path);
                        }
                    } else {
                        root.close_descendants(write, &[]);
                    }
                }
            },
        );
        Self { open, root, path: None, attachment, entry, collection, id: Id::new() }
    }

    pub fn rows(self) -> ReadList<K, T> {
        self.collection.rows()
    }

    pub fn is_open(self, read: &mut ReadCx<'_>) -> bool {
        self.open.get(read)
    }

    pub fn set_open(self, write: &mut WriteCx<'_>, open: bool) {
        if open
            && let Some(attachment) = write.get(self.attachment)
            && write.is_scope_alive(attachment.root.scope)
        {
            attachment.root.authorize(write, &attachment.path);
        }
        self.open.set(write, open);
        if !open && !write.read(|read| self.is_open(read)) {
            let path = write.read(|read| self.path(read));
            self.root.close_descendants(write, &path);
        }
    }

    pub fn button<C>(self, element: El<C>) -> El<C> {
        self.trigger(element.role(Role::Button)).behavior_first(|behavior| {
            behavior
                .on_press(move |write| {
                    let open = write.read(|read| self.is_open(read));
                    self.set_open(write, !open);
                })
                .on_key(move |write, event| {
                    if !event.modifiers.is_empty() {
                        return false;
                    }
                    let movement = match event.key {
                        Key::Down => Navigation::First,
                        Key::Up => Navigation::Last,
                        _ => return false,
                    };
                    self.open_from(write, movement);
                    true
                })
        })
    }

    /// Opens the caller's menu surface beside its owner.
    pub fn overlay<C>(
        self,
        owner: El<C>,
        placement: Placement,
        target: Option<AnchorTarget>,
        mut content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> El<C> {
        owner.overlay_target(
            placement,
            target,
            move |read| self.is_open(read),
            move |build| {
                self.provide_close(build);
                content(build)
            },
        )
    }

    pub fn items<C>(self, element: El<C>) -> El<C> {
        element
            .id(self.id.part("items"))
            .role(Role::Menu)
            .labelled_by(self.id.part("trigger"))
            .focus_group(FocusGroup::vertical().wrap())
            .layer(
                LayerPolicy {
                    focus_trap: FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS | FocusTrapFeatures::AUTO_FOCUS,
                    ..LayerPolicy::non_modal()
                },
                move |write| self.set_open(write, false),
            )
            .behavior_first(|behavior| {
                behavior.on(HandlerKind::Focus, move |write, _| self.apply_entry(write)).on_key(move |write, event| self.key(write, event))
            })
    }

    /// Attaches an action item. Press handlers authored on the element run after dismissal is requested.
    pub fn item<C>(self, element: El<C>, key: &K) -> El<C> {
        self.row(element.role(Role::MenuItem), key).behavior_first(|behavior| behavior.on_press(move |write| self.close_root(write)))
    }

    pub fn checkbox_item<C, V: CheckValue>(self, element: El<C>, key: &K, checked: impl Into<Binding<V>>) -> El<C> {
        let checked = checked.into();
        self.row(element.role(Role::MenuItemCheckbox), key).checked(bind(move |read| checked.get(read).checked())).behavior_first(
            |behavior| {
                behavior.on_press(move |write| {
                    let next = write.read(|read| checked.get(read)).toggle();
                    self.close_root(write);
                    checked.set(write, next);
                })
            },
        )
    }

    pub fn radio_item<C>(self, element: El<C>, key: &K, selected: impl Into<Binding<Option<K>>>) -> El<C> {
        let selected = selected.into();
        let checked_key = key.clone();
        let key = key.clone();
        self.row(element.role(Role::MenuItemRadio), &key)
            .checked(move |read: &mut ReadCx<'_>| selected.with(read, |value| value.as_ref() == Some(&checked_key)))
            .behavior_first(|behavior| {
                behavior.on_press(move |write| {
                    self.close_root(write);
                    if !write.read(|read| selected.with(read, |value| value.as_ref() == Some(&key))) {
                        selected.set(write, Some(key.clone()));
                    }
                })
            })
    }

    pub fn section<C>(self, element: El<C>, section: MenuSection) -> El<C> {
        element.role(Role::Group).labelled_by(section.0.part("heading"))
    }

    pub fn heading<C>(self, element: El<C>, section: MenuSection) -> El<C> {
        element.id(section.0.part("heading")).role(Role::Heading)
    }

    pub fn separator<C>(self, element: El<C>) -> El<C> {
        element.role(Role::Separator)
    }

    /// Opens a nested menu from one item. Build its surface with the child state supplied to `content`.
    pub fn submenu<C, L, U>(
        self,
        element: El<C>,
        key: &K,
        child: MenuState<L, U>,
        mut content: impl FnMut(&mut BuildCx<'_>, MenuState<L, U>) -> El + 'static,
    ) -> El<C>
    where
        L: Clone + Eq + Hash + Display + 'static,
        U: 'static,
    {
        let label = self.item_id(key);
        let submenu_key = key.to_string();
        let activation_key = submenu_key.clone();
        let open_child = move |write: &mut WriteCx<'_>| {
            let path = write.read(|read| self.submenu_path(read, &activation_key));
            self.root.authorize(write, &path);
            child.open_from(write, Navigation::First);
        };
        let key_open_child = open_child.clone();
        let expanded_key = submenu_key.clone();
        let row = self
            .row(element.role(Role::MenuItem), key)
            .expanded(bind(move |read| self.submenu_open(read, &expanded_key, child.open)))
            .controls(child.id.part("items"))
            .behavior_first(|behavior| {
                behavior.on_press(open_child).on_key(move |write, event| {
                    if event.modifiers.is_empty() && event.key == Key::Right {
                        key_open_child(write);
                        return true;
                    }
                    false
                })
            });
        let gate_key = submenu_key.clone();
        row.overlay_part(
            Placement::RIGHT,
            move |read| self.submenu_open(read, &gate_key, child.open),
            move |build| {
                let path = build.read(|read| self.submenu_path(read, &submenu_key));
                let attachment = Attachment { root: self.root, path: path.clone() };
                build.watch(
                    |_| (),
                    move |write, _, _| {
                        write.set(child.attachment, Some(attachment.clone()));
                        write.update(self.root.submenus, |submenus| {
                            submenus.insert(attachment.path.clone(), Submenu { open: child.open, blocked: false });
                        });
                    },
                );
                let child = MenuState { root: self.root, path: Some(build.signal(path)), ..child };
                child.provide_close(build);
                content(build, child).labelled_by(label.clone())
            },
        )
    }

    fn trigger<C>(self, element: El<C>) -> El<C> {
        element.id(self.id.part("trigger")).focusable().expanded(self.open).controls(self.id.part("items"))
    }

    fn row<C>(self, element: El<C>, key: &K) -> El<C> {
        let label = key.clone();
        let disabled = key.clone();
        let active = key.clone();
        let focused = key.clone();
        element
            .id(self.item_id(key))
            .focusable()
            .label(move |read: &mut ReadCx<'_>| self.collection.label(read, &label).unwrap_or_default())
            .disabled_when(bind(move |read| !self.collection.is_enabled(read, &disabled)))
            .focus_preferred(move |read: &mut ReadCx<'_>| self.collection.active(read).as_ref() == Some(&active))
            .behavior_first(|behavior| {
                behavior
                    .on(HandlerKind::Focus, move |write, _| {
                        self.collection.set_active(write, Some(focused.clone()));
                        self.apply_entry(write);
                    })
                    .on_key(move |write, event| self.key(write, event))
            })
    }

    fn item_id(self, key: &K) -> String {
        self.id.part(&format!("item-{key}"))
    }

    fn provide_close(self, build: &mut BuildCx<'_>) {
        build.provide_close(move |write| self.set_open(write, false));
    }

    fn path(self, read: &mut ReadCx<'_>) -> Vec<String> {
        self.path.map_or_else(Vec::new, |path| read.get(path))
    }

    fn submenu_path(self, read: &mut ReadCx<'_>, key: &str) -> Vec<String> {
        let mut path = self.path(read);
        path.push(key.to_owned());
        path
    }

    fn submenu_open(self, read: &mut ReadCx<'_>, key: &str, open: Binding<bool>) -> bool {
        let path = self.submenu_path(read, key);
        open.get(read) && read.with(self.root.submenus, |submenus| !submenus.get(&path).is_some_and(|submenu| submenu.blocked))
    }

    fn close_root(self, write: &mut WriteCx<'_>) {
        self.root.close_descendants(write, &[]);
        self.root.open.set(write, false);
    }

    fn navigate(self, write: &mut WriteCx<'_>, movement: Navigation) {
        if let Some(key) = self.collection.navigate_when(write, movement, |write, key| self.item_available(write, key)) {
            write.focus(&self.item_id(&key));
        }
    }

    fn open_from(self, write: &mut WriteCx<'_>, movement: Navigation) {
        self.set_open(write, true);
        if write.named_interactive(&self.id.part("items")).is_some() {
            self.navigate(write, movement);
        } else if write.read(|read| self.is_open(read)) {
            write.set_if_changed(self.entry, Some(movement));
        }
    }

    fn apply_entry(self, write: &mut WriteCx<'_>) {
        if let Some(movement) = write.get(self.entry) {
            write.set(self.entry, None);
            self.navigate(write, movement);
        }
    }

    fn key(self, write: &mut WriteCx<'_>, event: &KeyEvent) -> bool {
        if let Key::Char(character) = event.key
            && (event.modifiers - Modifiers::SHIFT).is_empty()
        {
            if let Some(key) = self.collection.typeahead_when(write, character, |write, key| self.item_available(write, key)) {
                write.focus(&self.item_id(&key));
            }
            return !character.is_control();
        }
        if !event.modifiers.is_empty() {
            return false;
        }
        let movement = match event.key {
            Key::Up => Navigation::Previous,
            Key::Down => Navigation::Next,
            Key::Home => Navigation::First,
            Key::End => Navigation::Last,
            Key::Left if self.path.is_some() => {
                self.set_open(write, false);
                return true;
            }
            _ => return false,
        };
        self.navigate(write, movement);
        true
    }

    fn item_available(self, write: &WriteCx<'_>, key: &K) -> bool {
        write.named_interactive(&self.item_id(key)) == Some(true)
    }
}

/// Adds context-menu invocation and positioning to a [`MenuState`].
pub struct ContextMenuState<K, T = Choice> {
    menu: MenuState<K, T>,
    point: Signal<Option<Point>>,
}

impl<K, T> Copy for ContextMenuState<K, T> {}

impl<K, T> Clone for ContextMenuState<K, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + Eq + Hash + Display + 'static, T: 'static> ContextMenuState<K, T> {
    pub fn new(build: &mut BuildCx<'_>, menu: MenuState<K, T>) -> Self {
        let point = build.signal(None);
        build.watch(
            move |read| menu.is_open(read),
            move |write, open, _| {
                if !open {
                    write.set_if_changed(point, None);
                }
            },
        );
        Self { menu, point }
    }

    /// Adds right-click, Context Menu key, Shift+F10, and assistive-technology invocation.
    pub fn trigger<C>(
        self,
        element: El<C>,
        placement: Placement,
        target: Option<AnchorTarget>,
        mut content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> El<C> {
        self.menu
            .trigger(element)
            .on_context_menu(move |write, event| {
                self.open_at(write, event.position);
                true
            })
            .overlay_target(
                placement,
                move |read: &mut ReadCx<'_>| read.get(self.point).map(AnchorTarget::Point).or(target),
                move |read| self.menu.is_open(read),
                move |build| {
                    self.menu.provide_close(build);
                    content(build)
                },
            )
    }

    fn open_at(self, write: &mut WriteCx<'_>, point: Option<Point>) {
        write.set_if_changed(self.point, point);
        self.menu.set_open(write, true);
    }
}
