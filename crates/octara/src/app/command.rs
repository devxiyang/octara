//! Resolve commands against the window's current focus and owners before invoking user code.

use super::{Chord, Handler, Window, keys::BUILTIN_COMMANDS, selection::DocumentCommand};
use crate::diagnostics as codes;
use crate::kernel::{Arrow, HandlerId, HandlerKind, Kind, NodeId};
use crate::reactive::graph::{Operation, Runtime};
use crate::reactive::{Signal, untrack};
use crate::view::ScopedHandler;
use std::panic::Location;
use std::rc::Rc;

enum Action<'a> {
    Handler(ScopedHandler),
    Submit(NodeId, ScopedHandler),
    FollowLink(NodeId),
    MoveFocus(NodeId, Option<NodeId>),
    ClearFocus,
    ClearSelection,
    Edit(NodeId, &'a str),
    Document(DocumentCommand),
}

impl Window {
    pub(super) fn live_handler(&self, id: HandlerId) -> Option<ScopedHandler> {
        let handler = self.w.handlers.borrow().get(&id.0).cloned()?;
        handler.is_live(&self.app.rt).then_some(handler)
    }

    fn resolve_command<'a>(&self, command: &'a str, path: &[NodeId]) -> Option<Action<'a>> {
        self.check_running().ok()?;
        let modal = self.top_modal();
        let focused = self.w.interaction.borrow().focused;
        let k = self.w.kernel.borrow();
        let focused = focused.filter(|&id| k.check_activation(id, modal).is_ok());
        // Keys can finish a composition before resolving. An explicit edit or submission
        // waits until the editor has an accepted value and no pending proposal.
        if (command == "submit" || command.starts_with("edit.")) && focused.is_some_and(|n| k.edit_in_progress(n)) {
            return None;
        }
        for &node in path {
            if k.check_activation(node, modal).is_ok()
                && let Some(handler) = k.command_handler(node, command).and_then(|id| self.live_handler(id))
            {
                return Some(Action::Handler(handler));
            }
        }
        drop(k);
        self.resolve_builtin(command, focused, modal).or_else(|| {
            self.w
                .commands
                .borrow()
                .iter()
                .rev()
                .filter(|(name, _)| name == command)
                .find_map(|(_, id)| self.live_handler(HandlerId(*id)))
                .map(Action::Handler)
        })
    }

    fn resolve_builtin<'a>(&self, command: &'a str, focused: Option<NodeId>, modal: Option<NodeId>) -> Option<Action<'a>> {
        let k = self.w.kernel.borrow();
        let handler = |node, kind| k.handler(node, kind).and_then(|id| self.live_handler(id));
        let group = |arrow| {
            let from = focused?;
            let to = k.group_step(from, arrow)?;
            k.check_activation(to, modal).ok()?;
            Some(Action::MoveFocus(to, k.group_activation(from, to)))
        };
        match command {
            "focus.next" | "focus.prev" => self.tab_target(command == "focus.next").map(|node| Action::MoveFocus(node, None)),
            "focus.up" => group(Arrow::Up),
            "focus.down" => group(Arrow::Down),
            "focus.left" => group(Arrow::Left),
            "focus.right" => group(Arrow::Right),
            "focus.first" => group(Arrow::Home),
            "focus.last" => group(Arrow::End),
            "focus.dismiss" => match (k.dismissal_layer(modal), focused) {
                (Some(node), _) => handler(node, HandlerKind::Dismiss).map(Action::Handler),
                (None, Some(_)) => Some(Action::ClearFocus),
                (None, None) if self.w.selection.borrow().is_some() => Some(Action::ClearSelection),
                _ => None,
            },
            "press" => {
                let node = focused?;
                handler(node, HandlerKind::Press)
                    .or_else(|| handler(node, HandlerKind::Toggle))
                    .map(Action::Handler)
                    .or_else(|| k.link_target(node).map(|_| Action::FollowLink(node)))
            }
            // Enter falls through to press when the focused editor has no submit callback.
            "submit" => {
                let node = focused.filter(|&node| k.kind(node) == Kind::TextInput)?;
                handler(node, HandlerKind::Submit).map(|handler| Action::Submit(node, handler))
            }
            "menu" => {
                let node = k.handler_above(focused, HandlerKind::SecondaryPress)?;
                k.check_activation(node, modal).ok()?;
                handler(node, HandlerKind::SecondaryPress).map(Action::Handler)
            }
            command if command.starts_with("edit.") => {
                drop(k);
                match focused {
                    Some(node) => self.edit_enabled(node, command).then_some(Action::Edit(node, command)),
                    None => self.resolve_document_command(command).map(Action::Document),
                }
            }
            _ => None,
        }
    }

    pub(super) fn run_command(&self, command: &str, path: &[NodeId]) -> bool {
        self.resolve_command(command, path).is_some_and(|action| self.execute_command(action))
    }

    fn execute_command(&self, action: Action<'_>) -> bool {
        match action {
            Action::Handler(handler) | Action::Submit(_, handler) => handler
                .run(&self.app.rt, |handler| match handler {
                    Handler::Press(f) | Handler::Toggle(f) => {
                        f();
                        true
                    }
                    _ => false,
                })
                .unwrap_or(false),
            Action::FollowLink(node) => self.run(node, &[HandlerKind::Press, HandlerKind::Toggle], None),
            Action::MoveFocus(node, activate_in) => self.move_command_focus(node, activate_in),
            Action::ClearFocus => self.set_focus(None).is_ok(),
            Action::ClearSelection => self.clear_doc_selection(),
            Action::Edit(node, command) => self.edit_command(node, command),
            Action::Document(command) => self.document_command(command),
        }
    }

    /// The editor that would handle a built-in submit now, for Inspector's node actions.
    pub(super) fn submit_target(&self) -> Option<NodeId> {
        match self.resolve_command("submit", &self.focus_path()) {
            Some(Action::Submit(node, _)) => Some(node),
            _ => None,
        }
    }

    /// A key equivalent follows the current keymap, including a context's unbinding. A direct
    /// menu selection invokes the command by name.
    pub(super) fn on_menu_command(&mut self, command: &str, chord: Option<&str>) -> bool {
        match chord {
            Some(chord) => Chord::parse(chord).is_ok_and(|chord| self.on_key(chord.event(self.app.platform.get()))),
            None => self.invoke(command).is_ok(),
        }
    }

    /// Whether the window can dispatch `command` in its current context. A binding in this
    /// App also observes changes after edit, focus and modal updates settle.
    pub fn command_enabled(&self, command: &str) -> bool {
        if self.check_running().is_err() {
            return false;
        }
        if Runtime::try_current().is_some_and(|rt| Rc::ptr_eq(&rt, &self.app.rt) && rt.collecting_dependencies()) {
            let source = self.w.command_availability.get().unwrap_or_else(|| {
                let source = Signal::detached(self.available_command_names(&self.focus_path()));
                self.w.command_availability.set(Some(source));
                source
            });
            return source.with(|names| names.iter().any(|name| name == command));
        }
        self.resolve_command(command, &self.focus_path()).is_some()
    }

    // Availability is feedback from the settled window, like measured element sizes. Only
    // a window observed by a binding needs this signal; command delivery always resolves again.
    pub(super) fn sync_command_availability(&self) -> bool {
        let Some(source) = self.w.command_availability.get() else { return false };
        let names = self.available_command_names(&self.focus_path());
        if untrack(|| source.with(|previous| *previous == names)) {
            return false;
        }
        source.set_internal(names);
        true
    }

    fn available_command_names(&self, path: &[NodeId]) -> Vec<String> {
        let mut names = self.w.kernel.borrow().commands_on(path);
        for (command, _) in BUILTIN_COMMANDS {
            if !names.iter().any(|name| name == command) {
                names.push((*command).to_owned());
            }
        }
        for (name, _) in self.w.commands.borrow().iter() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.retain(|command| self.resolve_command(command, path).is_some());
        names
    }

    /// The first keymap shortcut for `command` in the current focus context.
    pub fn chord_for(&self, command: &str) -> Option<String> {
        self.chords_for(command, &self.focus_path()).into_iter().next()
    }

    fn chords_for(&self, command: &str, path: &[NodeId]) -> Vec<String> {
        let keymap = self.app.effective_keymap();
        let platform = self.app.platform.get();
        let mut chords = Vec::new();
        for binding in keymap.iter().rev().filter(|binding| binding.command.as_deref() == Some(command)) {
            let candidates = self.resolve(&binding.chord.event(platform), path);
            let Some(declared) = candidates.first().and_then(Option::as_deref) else { continue };
            // An unavailable declaration can still be shown dimmed. When a different command
            // would take the key, show its shortcut alone.
            let effective =
                candidates.iter().map_while(Option::as_deref).find(|name| self.resolve_command(name, path).is_some()).unwrap_or(declared);
            if effective == command {
                let chord = binding.chord.to_string();
                if !chords.contains(&chord) {
                    chords.push(chord);
                }
            }
        }
        chords
    }

    /// Dispatches a command in the current window context. Returns an error when an eligible
    /// action cannot be delivered. Host calls, handlers and tasks may invoke it.
    #[track_caller]
    pub fn invoke(&mut self, command: &str) -> Result<(), String> {
        let at = Location::caller();
        if !Runtime::caller_allows(Operation::Write, at) {
            return Err("command invocation is not allowed in this execution phase".into());
        }
        self.check_running()?;
        if !self.app.rt.allows(Operation::Write, at) {
            return Err("command invocation is not allowed in this execution phase".into());
        }
        let unavailable = || {
            if BUILTIN_COMMANDS.iter().any(|(name, _)| *name == command) {
                format!("command `{command}` is unavailable in this window's current context")
            } else {
                format!("nothing handles `{command}` here (see `Window::commands`)")
            }
        };
        let action = self.resolve_command(command, &self.focus_path()).ok_or_else(unavailable)?;
        let mut done = false;
        self.input_frame(|| done = self.execute_command(action));
        done.then_some(()).ok_or_else(unavailable)
    }

    pub(super) fn run_after_update_command(&self, command: &str, at: &'static Location<'static>) -> bool {
        if self.w.closed.get() {
            self.w.interaction.borrow_mut().after_update.clear();
            return false;
        }
        if !self.run_command(command, &self.focus_path()) {
            self.app.rt.diag(
                &codes::COMMAND_UNHANDLED,
                format!("nothing handles deferred command `{command}` on the restored focus path"),
                at,
            );
        }
        true
    }

    /// Commands available now, each with its shortcuts in the current focus context.
    pub fn commands(&self) -> Vec<(String, Vec<String>)> {
        if self.check_running().is_err() {
            return Vec::new();
        }
        let path = self.focus_path();
        self.available_command_names(&path)
            .into_iter()
            .map(|command| {
                let chords = self.chords_for(&command, &path);
                (command, chords)
            })
            .collect()
    }
}
