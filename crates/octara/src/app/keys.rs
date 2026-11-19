//! The keyboard: key events, chords written as data (`"mod+shift+k"`, Rune), a keymap of
//! `(chord, command, context)` bindings resolved along the focus path (GPUI), and commands
//! handled by elements, by the runtime, or by the app.
//!
//! A key goes, in order: to an input method composing in the focused field (Enter and Escape
//! are its own); to `on_key` handlers on the focus path, innermost first; then to the command
//! the keymap binds it to, which the focused element or its ancestors (`on_command`), the
//! runtime (built-in commands) or the app (`on_command`) handles. An unhandled key types its
//! text, which the shell sends separately.

use super::pointer::Modifiers;
use super::{App, Handler, Modality, TargetPlatform, Window};
use crate::kernel::{HandlerKind, NodeId};
use crate::reactive::graph::{Operation, Runtime};
use std::fmt;
use std::panic::Location;
use std::rc::Rc;

/// A key, as the keyboard layout produces it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Tab,
    Enter,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// The context-menu key.
    Menu,
    F(u8),
    /// A key that types a character (with Shift applied: `'A'`, `'?'`).
    Char(char),
}

/// A key press.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct KeyEvent {
    pub key: Key,
    /// The physical key, as the character it types on a US layout (`'k'`): shortcuts still
    /// work under other layouts (Exact's `code`).
    pub code: Option<char>,
    pub modifiers: Modifiers,
    /// Held down and repeating.
    pub repeat: bool,
}

impl KeyEvent {
    pub fn new(key: Key, modifiers: Modifiers) -> Self {
        let code = match key {
            Key::Char(c) if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        KeyEvent { key, code, modifiers, repeat: false }
    }
}

/// A key with modifiers, written `"mod+shift+k"`: `mod` is Command on Apple platforms and
/// Control elsewhere; also `cmd` / `meta`, `ctrl`, `alt` / `option`, `shift`; keys by name
/// (`tab`, `enter`, `escape`, `backspace`, `delete`, `space`, `up`, `down`, `left`, `right`,
/// `home`, `end`, `pageup`, `pagedown`, `menu`, `f1`…) or as the character they type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
    /// Includes the platform's shortcut modifier (`mod`).
    pub platform_mod: bool,
}

impl Chord {
    pub fn parse(s: &str) -> Result<Chord, String> {
        let s = s.trim();
        let (mods, key) = match s.strip_suffix("++") {
            Some(rest) => (rest, "+"),
            None if s == "+" => ("", "+"),
            None => s.rsplit_once('+').unwrap_or(("", s)),
        };
        let mut modifiers = Modifiers::empty();
        let mut platform_mod = false;
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "mod" => platform_mod = true,
                "cmd" | "command" | "meta" | "super" | "win" => modifiers |= Modifiers::META,
                "ctrl" | "control" => modifiers |= Modifiers::CTRL,
                "alt" | "option" | "opt" => modifiers |= Modifiers::ALT,
                "shift" => modifiers |= Modifiers::SHIFT,
                other => return Err(format!("unknown modifier `{other}` in `{s}`")),
            }
        }
        let lower = key.to_ascii_lowercase();
        let key = match lower.as_str() {
            "tab" => Key::Tab,
            "enter" | "return" => Key::Enter,
            "escape" | "esc" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "space" => Key::Space,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "menu" => Key::Menu,
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => {
                Key::F(f[1..].parse().unwrap())
            }
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                    _ => return Err(format!("unknown key `{key}` in `{s}`")),
                }
            }
        };
        Ok(Chord { key, modifiers, platform_mod })
    }

    /// The modifiers this chord needs on `platform`.
    pub fn modifiers_on(&self, platform: TargetPlatform) -> Modifiers {
        let mut m = self.modifiers;
        if self.platform_mod {
            m |= if platform.is_apple() { Modifiers::META } else { Modifiers::CTRL };
        }
        m
    }

    fn matches(&self, key: Key, modifiers: Modifiers, platform: TargetPlatform) -> bool {
        let want = self.modifiers_on(platform);
        match (self.key, key) {
            (Key::Char(a), Key::Char(b)) if a.is_alphabetic() => a.to_lowercase().eq(b.to_lowercase()) && want == modifiers,
            // A symbol typed with Shift (`?`) matches a chord without it.
            (Key::Char(a), Key::Char(b)) => a == b && (want == modifiers || want | Modifiers::SHIFT == modifiers),
            (a, b) => a == b && want == modifiers,
        }
    }

    /// The key event this chord describes on `platform`.
    pub fn event(&self, platform: TargetPlatform) -> KeyEvent {
        let mut ev = KeyEvent::new(self.key, self.modifiers_on(platform));
        if let Key::Char(c) = self.key
            && ev.modifiers.contains(Modifiers::SHIFT)
            && c.is_alphabetic()
        {
            ev.key = Key::Char(c.to_ascii_uppercase());
        }
        ev
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.platform_mod {
            parts.push("mod".into());
        }
        for (m, name) in [(Modifiers::META, "cmd"), (Modifiers::CTRL, "ctrl"), (Modifiers::ALT, "alt"), (Modifiers::SHIFT, "shift")] {
            if self.modifiers.contains(m) {
                parts.push(name.into());
            }
        }
        parts.push(match self.key {
            Key::Char(c) => c.to_string(),
            Key::F(n) => format!("f{n}"),
            k => format!("{k:?}").to_ascii_lowercase(),
        });
        write!(f, "{}", parts.join("+"))
    }
}

/// One binding: `chord` runs `command` while focus is inside an element with key context
/// `context` (anywhere, without one). A `None` command unbinds the chord there (the key then
/// types, or reaches an outer binding's command no more).
#[derive(Clone, PartialEq, Debug)]
pub struct Binding {
    pub chord: Chord,
    pub command: Option<String>,
    pub context: Option<String>,
}

impl Binding {
    fn parse(chord: &str, command: Option<&str>, context: Option<&str>) -> Result<Self, String> {
        Ok(Self { chord: Chord::parse(chord)?, command: command.map(str::to_owned), context: context.map(str::to_owned) })
    }
}

/// Editable key bindings. Within a focus context, later bindings have priority.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

impl Keymap {
    /// Adds a binding to this configuration. `None` for the command unbinds the chord.
    pub fn bind<'a>(&mut self, chord: &str, command: impl Into<Option<&'a str>>, context: Option<&str>) -> Result<(), String> {
        let binding = Binding::parse(chord, command.into(), context)?;
        self.add(binding);
        Ok(())
    }

    fn add(&mut self, binding: Binding) {
        self.bindings.retain(|previous| previous != &binding);
        self.bindings.push(binding);
    }

    /// The built-in bindings on `platform` (Rune's and the platforms' conventions).
    pub fn standard(platform: TargetPlatform) -> Keymap {
        let mut k = Keymap::default();
        let mut b = |chord: &str, command: Option<&str>, context: Option<&str>| k.bind(chord, command, context).expect("standard chord");
        for (chord, command) in [
            ("tab", "focus.next"),
            ("shift+tab", "focus.prev"),
            ("up", "focus.up"),
            ("down", "focus.down"),
            ("left", "focus.left"),
            ("right", "focus.right"),
            ("home", "focus.first"),
            ("end", "focus.last"),
            ("escape", "focus.dismiss"),
            ("enter", "press"),
            ("space", "press"),
            ("menu", "menu"),
            ("shift+f10", "menu"),
        ] {
            b(chord, Some(command), None);
        }
        // Text fields: Enter submits (a multi-line field breaks the line; Cmd/Ctrl+Enter
        // submits), Space types; arrows, Home and End edit (by visual line in a multi-line field).
        b("enter", Some("submit"), Some("text_input"));
        b("enter", Some("edit.newline"), Some("text_area"));
        b("shift+enter", Some("edit.newline"), Some("text_area"));
        b("mod+enter", Some("submit"), Some("text_area"));
        // A chat composer (multi-line, Enter submits) breaks lines with Shift or Option.
        b("shift+enter", Some("edit.newline"), Some("text_input"));
        b("alt+enter", Some("edit.newline"), Some("text_input"));
        let word = if platform.is_apple() { "alt" } else { "ctrl" };
        // The same editing chords in both kinds of field: lines are visual lines, and a
        // single-line field is one line (Up goes to its start, Down to its end).
        let mut edits: Vec<(String, Option<&str>)> = vec![
            ("space".into(), None),
            ("left".into(), Some("edit.move_left")),
            ("right".into(), Some("edit.move_right")),
            (format!("{word}+left"), Some("edit.move_word_left")),
            (format!("{word}+right"), Some("edit.move_word_right")),
            ("home".into(), Some("edit.move_to_line_start")),
            ("end".into(), Some("edit.move_to_line_end")),
            ("up".into(), Some("edit.move_up")),
            ("down".into(), Some("edit.move_down")),
            ("pageup".into(), Some("edit.move_page_up")),
            ("pagedown".into(), Some("edit.move_page_down")),
            ("shift+left".into(), Some("edit.select_left")),
            ("shift+right".into(), Some("edit.select_right")),
            (format!("{word}+shift+left"), Some("edit.select_word_left")),
            (format!("{word}+shift+right"), Some("edit.select_word_right")),
            ("shift+home".into(), Some("edit.select_to_line_start")),
            ("shift+end".into(), Some("edit.select_to_line_end")),
            ("shift+up".into(), Some("edit.select_up")),
            ("shift+down".into(), Some("edit.select_down")),
            ("shift+pageup".into(), Some("edit.select_page_up")),
            ("shift+pagedown".into(), Some("edit.select_page_down")),
            ("mod+a".into(), Some("edit.select_all")),
            ("backspace".into(), Some("edit.delete_backward")),
            ("shift+backspace".into(), Some("edit.delete_backward")),
            ("delete".into(), Some("edit.delete_forward")),
            (format!("{word}+backspace"), Some("edit.delete_word_backward")),
            (format!("{word}+delete"), Some("edit.delete_word_forward")),
            ("mod+c".into(), Some("edit.copy")),
            ("mod+x".into(), Some("edit.cut")),
            ("mod+v".into(), Some("edit.paste")),
            ("mod+z".into(), Some("edit.undo")),
            ("mod+shift+z".into(), Some("edit.redo")),
        ];
        if platform.is_apple() {
            edits.extend([
                ("cmd+left".to_owned(), Some("edit.move_to_line_start")),
                ("cmd+right".into(), Some("edit.move_to_line_end")),
                ("cmd+shift+left".into(), Some("edit.select_to_line_start")),
                ("cmd+shift+right".into(), Some("edit.select_to_line_end")),
                ("cmd+up".into(), Some("edit.move_to_start")),
                ("cmd+down".into(), Some("edit.move_to_end")),
                ("cmd+shift+up".into(), Some("edit.select_to_start")),
                ("cmd+shift+down".into(), Some("edit.select_to_end")),
                ("cmd+backspace".into(), Some("edit.delete_to_line_start")),
            ]);
        } else {
            edits.extend([
                ("ctrl+home".to_owned(), Some("edit.move_to_start")),
                ("ctrl+end".into(), Some("edit.move_to_end")),
                ("ctrl+shift+home".into(), Some("edit.select_to_start")),
                ("ctrl+shift+end".into(), Some("edit.select_to_end")),
                ("ctrl+y".into(), Some("edit.redo")),
            ]);
        }
        for field in ["text_input", "text_area"] {
            for (chord, command) in &edits {
                b(chord, *command, Some(field));
            }
        }
        // A document selection (a selectable region's), where no field has focus.
        b("mod+c", Some("edit.copy"), None);
        b("mod+alt+c", Some("edit.copy_markdown"), None);
        b("mod+a", Some("edit.select_all"), None);
        k
    }
}

/// Built-in commands and what they do.
pub const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("focus.next", "focus the next Tab stop"),
    ("focus.prev", "focus the previous Tab stop"),
    ("focus.up", "move focus up in a focus group"),
    ("focus.down", "move focus down in a focus group"),
    ("focus.left", "move focus left in a focus group"),
    ("focus.right", "move focus right in a focus group"),
    ("focus.first", "focus a focus group's first member"),
    ("focus.last", "focus a focus group's last member"),
    ("focus.dismiss", "dismiss the open modal, else drop focus"),
    ("press", "press the focused element"),
    ("submit", "submit the focused text field (its `on_submit`)"),
    ("menu", "open the focused element's context menu"),
    ("edit.move_left", "caret one character left (or to the selection's start)"),
    ("edit.move_right", "caret one character right (or to the selection's end)"),
    ("edit.move_word_left", "caret to the start of the word before it"),
    ("edit.move_word_right", "caret to the end of the word after it"),
    ("edit.move_to_start", "caret to the start"),
    ("edit.move_to_end", "caret to the end"),
    ("edit.select_left", "extend the selection one character left"),
    ("edit.select_right", "extend the selection one character right"),
    ("edit.select_word_left", "extend the selection a word left"),
    ("edit.select_word_right", "extend the selection a word right"),
    ("edit.select_to_start", "extend the selection to the start"),
    ("edit.select_to_end", "extend the selection to the end"),
    ("edit.move_up", "caret one visual line up, keeping its x (at the first line: to the start)"),
    ("edit.move_down", "caret one visual line down, keeping its x (at the last line: to the end)"),
    ("edit.move_page_up", "caret up by the lines the field shows"),
    ("edit.move_page_down", "caret down by the lines the field shows"),
    ("edit.move_to_line_start", "caret to the start of its visual line"),
    ("edit.move_to_line_end", "caret to the end of its visual line"),
    ("edit.select_up", "extend the selection one visual line up"),
    ("edit.select_down", "extend the selection one visual line down"),
    ("edit.select_page_up", "extend the selection up by the lines the field shows"),
    ("edit.select_page_down", "extend the selection down by the lines the field shows"),
    ("edit.select_to_line_start", "extend the selection to the start of the visual line"),
    ("edit.select_to_line_end", "extend the selection to the end of the visual line"),
    ("edit.select_all", "select everything"),
    ("edit.delete_backward", "delete the selection, else the character before the caret"),
    ("edit.delete_forward", "delete the selection, else the character after the caret"),
    ("edit.delete_word_backward", "delete the selection, else back to the word's start"),
    ("edit.delete_word_forward", "delete the selection, else to the word's end"),
    ("edit.delete_to_start", "delete the selection, else back to the start"),
    ("edit.delete_to_line_start", "delete the selection, else back to the start of the visual line"),
    ("edit.newline", "break the line (multi-line fields)"),
    ("edit.copy", "copy the selection"),
    ("edit.copy_markdown", "copy the selection as Markdown (a document's)"),
    ("edit.cut", "cut the selection"),
    ("edit.paste", "paste at the caret"),
    ("edit.undo", "undo the last edit"),
    ("edit.redo", "redo what was undone"),
];

impl App {
    /// The app's configured bindings. The returned value can be edited and passed to
    /// [`Self::set_keymap`].
    pub fn keymap(&self) -> Keymap {
        self.app.keymap.borrow().clone()
    }

    /// A read-only snapshot of the platform defaults followed by the app's configuration.
    /// Each window resolves these bindings against its own focus path and command availability.
    ///
    /// ```compile_fail
    /// use octara::{app::App, platform::HeadlessPlatform};
    /// let app = App::new(HeadlessPlatform::default());
    /// app.set_keymap(app.effective_keymap()).unwrap();
    /// ```
    pub fn effective_keymap(&self) -> Rc<[Binding]> {
        self.app.effective_keymap()
    }

    /// Replaces the app's configuration. All windows see the same new bindings.
    /// The platform defaults can be overridden or unbound by entries in this configuration.
    #[track_caller]
    pub fn set_keymap(&self, keymap: Keymap) -> Result<(), String> {
        self.update_keymap(Location::caller(), |_| keymap)
    }

    /// Adds a batch of `(chord, command, context)` bindings. All chords are parsed before the
    /// batch is applied. Use a command name to bind, or `None` to unbind. Use `Some(name)` for
    /// bindings in a batch that also unbinds keys. An absent context applies in every window.
    /// Host calls, construction, handlers and tasks can configure bindings.
    #[track_caller]
    pub fn bind_keys<'a, C: Into<Option<&'a str>>>(
        &self,
        bindings: impl IntoIterator<Item = (&'a str, C, Option<&'a str>)>,
    ) -> Result<(), String> {
        let at = Location::caller();
        self.check_keymap_registration(at)?;
        let bindings = bindings
            .into_iter()
            .map(|(key, command, context)| Binding::parse(key, command.into(), context))
            .collect::<Result<Vec<_>, _>>()?;
        // The iterator can run app code. Recheck admission and apply to the latest configuration.
        self.update_keymap(at, |current| {
            let mut next = current.clone();
            for binding in bindings {
                next.add(binding);
            }
            next
        })
    }

    fn check_keymap_registration(&self, at: &'static Location<'static>) -> Result<(), String> {
        self.app.check_running()?;
        if Runtime::caller_allows(Operation::Register, at) && self.app.rt.allows(Operation::Register, at) {
            Ok(())
        } else {
            Err("key binding registration is not allowed in this execution phase".into())
        }
    }

    fn update_keymap(&self, at: &'static Location<'static>, update: impl FnOnce(&Keymap) -> Keymap) -> Result<(), String> {
        self.check_keymap_registration(at)?;
        let mut current = self.app.keymap.borrow_mut();
        let next = update(&current);
        if *current == next {
            return Ok(());
        }
        *current = next;
        drop(current);
        self.app.keymap_effective.borrow_mut().take();
        if self.app.depth.get() == 0 {
            self.app.frame(None, at);
        }
        Ok(())
    }
}

impl Window {
    /// The logical focus path, innermost first, ending at the active modal or the window.
    pub(super) fn focus_path(&self) -> Vec<NodeId> {
        let k = self.w.kernel.borrow();
        let modal = self.top_modal();
        let focused = self.w.interaction.borrow().focused.filter(|&n| k.check_activation(n, modal).is_ok());
        let fallback = || modal.or_else(|| k.children(k.root()).first().copied()).filter(|&n| k.is_shown(n));
        let mut path = Vec::new();
        let mut n = focused.or_else(fallback);
        while let Some(id) = n {
            if !k.is_mounted(id) || path.contains(&id) {
                break;
            }
            path.push(id);
            if Some(id) == modal {
                break;
            }
            n = k.interaction_parent(id);
        }
        path
    }

    /// The commands a key resolves to, best first: bindings whose context is on the focus path,
    /// deeper contexts first, later bindings first. `None` entries unbind.
    pub(super) fn resolve(&self, ev: &KeyEvent, path: &[NodeId]) -> Vec<Option<String>> {
        let k = self.w.kernel.borrow();
        let platform = self.app.platform.get();
        // Depth of each context on the path (the window is depth 0).
        let depth_of = |ctx: &str| -> Option<usize> {
            path.iter().enumerate().find(|(_, id)| k.key_context(**id) == Some(ctx) && k.is_shown(**id)).map(|(i, _)| path.len() - i)
        };
        let keymap = self.app.effective_keymap();
        let found = |key: Key| -> Vec<(usize, usize, Option<String>)> {
            keymap
                .iter()
                .enumerate()
                .filter(|(_, b)| b.chord.matches(key, ev.modifiers, platform))
                .filter_map(|(i, b)| {
                    let depth = match &b.context {
                        None => 0,
                        Some(c) => depth_of(c)?,
                    };
                    Some((depth, i, b.command.clone()))
                })
                .collect()
        };
        let mut hits = found(ev.key);
        // Another layout: a shortcut matches by the physical key (Cmd+К is Cmd+R).
        let shortcut = ev.modifiers.intersects(Modifiers::CTRL | Modifiers::META | Modifiers::ALT);
        if hits.is_empty()
            && shortcut
            && let Some(c) = ev.code
            && !matches!(ev.key, Key::Char(k) if k.is_ascii())
        {
            hits = found(Key::Char(c));
        }
        hits.sort_by_key(|h| std::cmp::Reverse((h.0, h.1)));
        hits.into_iter().map(|(_, _, c)| c).collect()
    }

    /// A key press, as a window delivers it. Returns whether something handled it; if not, the
    /// backend sends the key's text (`InputEvent::Text`).
    pub(super) fn on_key(&mut self, ev: KeyEvent) -> bool {
        self.on_key_guarded(ev, None)
    }

    pub(super) fn on_key_guarded(&mut self, ev: KeyEvent, guard: Option<&super::TargetGuard>) -> bool {
        let mut handled = false;
        self.input_frame(|| handled = self.route_key(&ev, guard));
        handled
    }

    fn route_key(&self, ev: &KeyEvent, guard: Option<&super::TargetGuard>) -> bool {
        if matches!(
            ev.key,
            Key::Tab | Key::Up | Key::Down | Key::Left | Key::Right | Key::Home | Key::End | Key::Enter | Key::Space | Key::Escape
        ) {
            self.set_modality(Modality::Keyboard);
        }
        // Composition owns confirmation, cancellation and unmodified candidate navigation.
        // An editor-owned suggestion list must not consume the IME's arrows or Space/Tab.
        // A shortcut commits the composition first (Exact).
        if let Some((n, _)) = self.focused_input() {
            let composing = !self.w.kernel.borrow().preedit(n).is_empty();
            if composing {
                if matches!(ev.key, Key::Enter | Key::Escape) {
                    return true;
                }
                if ev.modifiers.is_empty()
                    && matches!(
                        ev.key,
                        Key::Up
                            | Key::Down
                            | Key::Left
                            | Key::Right
                            | Key::Home
                            | Key::End
                            | Key::PageUp
                            | Key::PageDown
                            | Key::Tab
                            | Key::Space
                    )
                {
                    return true;
                }
                if ev.modifiers.intersects(Modifiers::CTRL | Modifiers::META) {
                    let text = self.w.kernel.borrow().preedit(n).to_owned();
                    self.insert_text(&text);
                    self.settle_frame();
                    if self.is_closed() || self.app.shutdown.get() || self.w.kernel.borrow().edit_pending(n) {
                        return true;
                    }
                }
            }
        }
        if guard.is_some_and(|guard| !guard.is_current(self)) {
            return false;
        }
        let path = self.focus_path();
        // `on_key`, innermost first.
        for &n in &path {
            let handler = {
                let k = self.w.kernel.borrow();
                if !k.is_alive(n) {
                    continue;
                }
                k.handler(n, HandlerKind::Key)
                    .filter(|_| k.check_activation(n, self.top_modal()).is_ok())
                    .and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
            };
            if let Some(handler) = handler {
                let took = handler
                    .run(&self.app.rt, |handler| match handler {
                        Handler::Key(f) => f(ev),
                        _ => false,
                    })
                    .unwrap_or(false);
                if took {
                    return true;
                }
                if guard.is_some_and(|guard| !guard.is_current(self)) {
                    return false;
                }
            }
        }
        if self.check_running().is_err() {
            return false;
        }
        let path = self.focus_path();
        for command in self.resolve(ev, &path) {
            let Some(command) = command else { return false };
            if self.run_command(&command, &path) {
                return true;
            }
        }
        false
    }

    /// Presses the keys of `chord` (`"mod+s"`, `"tab"`, `"a"`), as a keyboard would: what the
    /// keymap binds runs; an unbound character key types itself.
    pub fn press_key(&mut self, chord: &str) -> Result<(), String> {
        self.check_running()?;
        let ev = Chord::parse(chord)?.event(self.app.platform.get());
        let handled = self.event(super::event::InputEvent::Key(ev));
        if !handled
            && let Key::Char(c) = ev.key
            && !ev.modifiers.intersects(Modifiers::CTRL | Modifiers::META)
        {
            self.type_text(&c.to_string());
        }
        if !handled && matches!(ev.key, Key::Space) && self.focused_input().is_some() {
            self.type_text(" ");
        }
        Ok(())
    }
}
