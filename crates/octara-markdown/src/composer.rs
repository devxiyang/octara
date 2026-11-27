//! Markdown composition on the retained editor.

mod presentation;

use octara::prelude::*;
use octara_headless::popover::PopoverState;
use std::{ops::Range, sync::Arc};

/// A character that opens completion at a word boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerConfig {
    pub trigger: char,
    pub min_chars: usize,
    pub allow_spaces: bool,
}

impl TriggerConfig {
    pub const fn new(trigger: char) -> Self {
        Self { trigger, min_chars: 0, allow_spaces: false }
    }
}

/// The text between a trigger character and the current caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerQuery {
    pub trigger: char,
    pub query: String,
    /// UTF-8 byte range in the original Markdown, including the trigger.
    pub range: Range<usize>,
}

/// A suggestion offered by the application for an active trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: Arc<str>,
    item: CompletionItem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CompletionItem {
    Link(Arc<str>),
    Action(Arc<str>),
}

impl Completion {
    /// Inserts an inline Markdown link. Private URI schemes are styled and edited as one token.
    pub fn new(label: impl Into<Arc<str>>, destination: impl Into<Arc<str>>) -> Self {
        Self { label: label.into(), item: CompletionItem::Link(destination.into()) }
    }

    /// Removes the trigger text and reports the action key to the selection callback.
    pub fn action(label: impl Into<Arc<str>>, key: impl Into<Arc<str>>) -> Self {
        Self { label: label.into(), item: CompletionItem::Action(key.into()) }
    }

    pub fn action_key(&self) -> Option<&str> {
        match &self.item {
            CompletionItem::Action(key) => Some(key),
            CompletionItem::Link(_) => None,
        }
    }

    pub fn destination(&self) -> Option<&str> {
        match &self.item {
            CompletionItem::Link(destination) => Some(destination),
            CompletionItem::Action(_) => None,
        }
    }

    fn replacement(&self) -> String {
        let Some(destination) = self.destination() else { return String::new() };
        let mut markdown = String::from("[");
        for ch in self.label.chars() {
            if matches!(ch, '\\' | '[' | ']' | '*' | '_' | '`' | '<' | '!' | '&') {
                markdown.push('\\');
            }
            markdown.push(ch);
        }
        markdown.push_str("](");
        for ch in destination.chars() {
            match ch {
                ' ' => markdown.push_str("%20"),
                '\r' => markdown.push_str("%0D"),
                '\n' => markdown.push_str("%0A"),
                '\t' => markdown.push_str("%09"),
                '\\' | '(' | ')' | '<' | '>' | '&' => {
                    markdown.push('\\');
                    markdown.push(ch);
                }
                _ => markdown.push(ch),
            }
        }
        markdown.push(')');
        markdown
    }
}

/// A reusable handle to one Markdown composer and its active completion query.
///
/// Markdown is stored verbatim, including the destinations of inline completion
/// links. The editor folds their markup visually and edits each link as one token.
#[derive(Clone, Copy)]
pub struct MarkdownComposer {
    model: Binding<String>,
    markdown: ReadSignal<String>,
    editor: Signal<Option<EditorState>>,
    dismissed: Signal<Option<EditorState>>,
    active_trigger: ReadSignal<Option<TriggerQuery>>,
}

impl MarkdownComposer {
    pub fn new(build: &mut BuildCx<'_>, markdown: impl Into<String>) -> Self {
        let model = build.signal(markdown.into());
        Self::bind(build, model)
    }

    pub fn with_triggers(build: &mut BuildCx<'_>, markdown: impl Into<String>, triggers: impl IntoIterator<Item = TriggerConfig>) -> Self {
        let model = build.signal(markdown.into());
        Self::bind_with_triggers(build, model, triggers)
    }

    pub fn bind(build: &mut BuildCx<'_>, markdown: impl Into<Binding<String>>) -> Self {
        Self::bind_with_triggers(build, markdown, ['@', '#', '/'].map(TriggerConfig::new))
    }

    pub fn bind_with_triggers(
        build: &mut BuildCx<'_>,
        markdown: impl Into<Binding<String>>,
        triggers: impl IntoIterator<Item = TriggerConfig>,
    ) -> Self {
        let model = markdown.into();
        let markdown = build.memo(move |read| model.get(read));
        let editor = build.signal(None);
        let dismissed = build.signal(None);
        let triggers: Vec<_> = triggers.into_iter().collect();
        let active_trigger = build.memo(move |read| {
            let state: EditorState = read.get(editor)?;
            let (anchor, caret) = state.selection;
            if state.composing || anchor != caret || read.get(dismissed) == Some(state) {
                return None;
            }
            read.with(markdown, |source| trigger_query(source, caret, &triggers))
        });
        Self { model, markdown, editor, dismissed, active_trigger }
    }

    /// The exact Markdown submitted by the editor.
    pub fn markdown(self) -> ReadSignal<String> {
        self.markdown
    }

    pub fn set_markdown(self, write: &mut WriteCx<'_>, markdown: impl Into<String>) {
        self.model.set(write, markdown.into());
    }

    pub fn active_trigger(self) -> ReadSignal<Option<TriggerQuery>> {
        self.active_trigger
    }

    /// Queues a completion through the editor's normal controlled edit and undo path.
    pub fn insert(self, write: &mut WriteCx<'_>, completion: Completion) -> bool {
        self.replace_trigger(write, completion.replacement())
    }

    fn replace_trigger(self, write: &mut WriteCx<'_>, replacement: String) -> bool {
        let Some(state) = write.get(self.editor) else { return false };
        let Some(query) = write.get(self.active_trigger) else { return false };
        write.replace_editor_text(state, query.range, replacement)
    }

    /// Attaches the composer to a text input and shows suggestions at its live caret.
    pub fn editor(
        self,
        build: &mut BuildCx<'_>,
        editor: TextInput,
        suggestions: impl FnMut(&mut ReadCx<'_>, &TriggerQuery) -> Vec<Completion> + 'static,
    ) -> TextInput {
        self.editor_with_completion(build, editor, suggestions, |_, _| {})
    }

    /// Attaches a composer with a callback for accepted suggestions and actions.
    pub fn editor_with_completion(
        self,
        build: &mut BuildCx<'_>,
        editor: TextInput,
        mut suggestions: impl FnMut(&mut ReadCx<'_>, &TriggerQuery) -> Vec<Completion> + 'static,
        on_complete: impl Fn(&mut WriteCx<'_>, &Completion) + 'static,
    ) -> TextInput {
        let items = build.memo(move |read| read.get(self.active_trigger).map_or_else(Vec::new, |query| suggestions(read, &query)));
        let selected = build.signal(0usize);
        let pending = build.signal(None::<(String, Completion)>);
        let submitted = build.signal(None::<(String, Completion)>);
        let open = build.binding(
            move |read| read.with(items, |items| !items.is_empty()),
            move |write, is_open| {
                if !is_open {
                    let state = write.get(self.editor);
                    write.set_if_changed(self.dismissed, state);
                }
            },
        );
        let popup = PopoverState::controlled(open);
        let panel = popup.panel_id();
        let reset_panel = panel.clone();
        build.watch(
            move |read| (read.get(self.active_trigger), read.get(items)),
            move |write, (_, items), _| {
                write.set_if_changed(selected, 0);
                if !items.is_empty() {
                    write.reveal_row(&reset_panel, 0);
                }
            },
        );
        build.watch(
            move |read| read.get(submitted),
            move |write, submitted_item, _| {
                if let Some((expected, completion)) = submitted_item {
                    write.set(submitted, None);
                    if write.read(|read| self.model.with(read, |markdown| markdown == expected)) {
                        on_complete(write, completion);
                    }
                }
            },
        );
        let choose = move |write: &mut WriteCx<'_>, completion: Completion| {
            let Some(query) = write.get(self.active_trigger) else { return false };
            let replacement = completion.replacement();
            let mut expected = write.get(self.markdown);
            expected.replace_range(query.range, &replacement);
            if !self.replace_trigger(write, replacement) {
                return false;
            }
            write.set(pending, Some((expected, completion)));
            true
        };
        let controls = panel.clone();
        let option_id = move |index| format!("{}-option-{index}", popup.panel_id());
        let rows = build.derive_list(move |read| read.get(items).into_iter().enumerate().collect::<Vec<_>>());
        let editor = editor
            .value(self.model)
            .editor_backend(Arc::new(presentation::MarkdownBackend))
            .observe_editor(self.editor)
            .role(Role::EditableComboBox)
            .expanded(open)
            .controls(bind(move |read| open.get(read).then(|| controls.clone())))
            .active_descendant(bind(move |read| open.get(read).then(|| option_id(read.get(selected)))))
            .behavior_first(move |behavior| {
                behavior
                    .on_input(move |write, text| {
                        self.set_markdown(write, text);
                        if let Some((expected, completion)) = write.get(pending) {
                            write.set(pending, None);
                            if expected == text {
                                write.set(submitted, Some((expected, completion)));
                            }
                        }
                    })
                    .on_key(move |write, event| {
                        if !event.modifiers.is_empty() || !write.read(|read| open.get(read)) {
                            return false;
                        }
                        let count = write.with(items, Vec::len);
                        let index = write.get(selected).min(count - 1);
                        let next = match event.key {
                            Key::Escape => {
                                popup.set_open(write, false);
                                return true;
                            }
                            Key::Up => {
                                if index == 0 {
                                    count - 1
                                } else {
                                    index - 1
                                }
                            }
                            Key::Down => (index + 1) % count,
                            Key::Enter if event.repeat => return true,
                            Key::Enter => {
                                let item = write.with(items, |items| items[index].clone());
                                return choose(write, item);
                            }
                            _ => return false,
                        };
                        write.set(selected, next);
                        write.reveal_row(&panel, next);
                        true
                    })
            });
        popup.overlay_with_target(
            editor,
            Placement::BELOW,
            bind(move |read| read.get(self.editor).map(|state| AnchorTarget::EditorCaret(state.session.node))),
            move |_| {
                popup
                    .panel(col())
                    .labelled_by(None::<String>)
                    .label("Suggestions")
                    .role(Role::ListBox)
                    .min_w(160.0)
                    .max_h(240.0)
                    .overflow_y(Overflow::Auto)
                    .p(4.0)
                    .rounded(RadiusToken::Md)
                    .bg(ColorToken::SurfaceElevated)
                    .border(1.0)
                    .border_color(ColorToken::Border)
                    .preserve_focus_on_press()
                    .child(virtual_each(rows, VirtualOptions::default(), move |build, &index, item| {
                        let label = build.memo({
                            let item = item.clone();
                            move |read| item.with(read, |item| item.map_or_else(String::new, |item| item.label.to_string()))
                        });
                        text(move |read: &mut ReadCx<'_>| read.get(label))
                            .id(option_id(index))
                            .role(Role::ListBoxOption)
                            .label(bind(move |read| read.get(label)))
                            .selected(bind(move |read| read.get(selected) == index))
                            .px(8.0)
                            .py(5.0)
                            .rounded(RadiusToken::Sm)
                            .bg(bind(move |read| -> ColorValue {
                                if read.get(selected) == index { ColorToken::SurfacePressed.into() } else { Color::TRANSPARENT.into() }
                            }))
                            .preserve_focus_on_press()
                            .on_press(move |write| {
                                if let Some(item) = write.read(|read| item.read(read)) {
                                    choose(write, item);
                                }
                            })
                    }))
            },
        )
    }
}

fn trigger_query(source: &str, caret: usize, triggers: &[TriggerConfig]) -> Option<TriggerQuery> {
    let prefix = source.get(..caret)?;
    for (start, ch) in prefix.char_indices().rev() {
        if let Some(config) = triggers.iter().find(|config| config.trigger == ch)
            && !prefix[..start].ends_with(|before: char| !boundary(before))
        {
            let query = &prefix[start + ch.len_utf8()..];
            if query.chars().count() >= config.min_chars
                && (config.allow_spaces || !query.chars().any(char::is_whitespace))
                && !presentation::literal_at(source, start)
            {
                return Some(TriggerQuery { trigger: ch, query: query.to_owned(), range: start..caret });
            }
        }
        if !(ch.is_alphanumeric() || matches!(ch, '_' | '-') || (ch.is_whitespace() && !matches!(ch, '\r' | '\n'))) {
            break;
        }
    }
    None
}

fn boundary(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '"' | '\'')
}
