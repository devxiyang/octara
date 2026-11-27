use crate::protocol::{INLINE_ORIGIN, RpcCall};
use crate::{
    NavigationDecision, NavigationRequest, NavigationState, WebViewCommand, WebViewEvent, WebViewProps, WebViewSource, emit,
    unavailable_snapshot, unsupported_command,
};
use octara::module::{
    CallTicket, ModuleDispatcher, ModuleError, NativePlacement, NativeSnapshotRequest, NativeViewEmitter, NativeViewInstance,
};
use std::any::Any;

#[derive(Default)]
struct History {
    entries: Vec<String>,
    index: usize,
}

impl History {
    fn push(&mut self, url: String) {
        self.entries.truncate(self.index + 1);
        self.entries.push(url);
        self.index = self.entries.len() - 1;
    }

    fn state(&self) -> NavigationState {
        NavigationState {
            url: self.entries.get(self.index).cloned(),
            can_go_back: self.index > 0,
            can_go_forward: self.index + 1 < self.entries.len(),
            ..Default::default()
        }
    }
}

pub(crate) struct Mock {
    emitter: NativeViewEmitter,
    source: Option<WebViewSource>,
    history: History,
}

impl Mock {
    pub(crate) fn new(props: WebViewProps, emitter: NativeViewEmitter) -> Self {
        let mut view = Self { emitter, source: None, history: History::default() };
        view.load(props);
        view
    }

    fn load(&mut self, props: WebViewProps) {
        if self.source.as_ref() == Some(&props.source) {
            return;
        }
        let url = match &props.source {
            WebViewSource::Url(url) => url.clone(),
            WebViewSource::Html { base_url, .. } => base_url.clone().unwrap_or_else(|| format!("{INLINE_ORIGIN}/")),
        };
        let inline = matches!(&props.source, WebViewSource::Html { base_url: None, .. });
        let request = NavigationRequest { url, main_frame: true, new_window: false };
        let allowed = props.decision(&request, inline) != NavigationDecision::Cancel;
        self.source = Some(props.source);
        if allowed {
            self.history.push(request.url);
            self.loaded();
        } else {
            emit(&self.emitter, WebViewEvent::NavigationBlocked(request));
        }
    }

    fn loaded(&self) {
        let state = self.history.state();
        let url = state.url.clone();
        emit(&self.emitter, WebViewEvent::Navigation(state));
        emit(&self.emitter, WebViewEvent::Loaded { url });
    }
}

impl NativeViewInstance for Mock {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(props) = props.downcast::<WebViewProps>() {
            self.load(*props);
        }
    }

    fn place(&mut self, _placement: &NativePlacement) {}

    fn call(&mut self, _ticket: CallTicket, command: Box<dyn Any>, _reply: ModuleDispatcher) {
        match command.downcast::<WebViewCommand>() {
            Ok(command) => match *command {
                WebViewCommand::GoBack if self.history.index > 0 => {
                    self.history.index -= 1;
                    self.loaded();
                }
                WebViewCommand::GoForward if self.history.state().can_go_forward => {
                    self.history.index += 1;
                    self.loaded();
                }
                WebViewCommand::Reload if !self.history.entries.is_empty() => self.loaded(),
                WebViewCommand::GoBack | WebViewCommand::GoForward | WebViewCommand::Reload | WebViewCommand::StopLoading => {}
                command => unsupported_command(&self.emitter, command),
            },
            Err(command) => {
                if let Ok(call) = command.downcast::<RpcCall>() {
                    emit(&self.emitter, call.reply(Err(ModuleError::Unavailable)));
                }
            }
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, _reply: ModuleDispatcher) {
        unavailable_snapshot(&self.emitter, request, "The headless web view mock has no pixels".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_discards_forward_entries_after_navigation() {
        let mut history = History::default();
        history.push("https://first.example".into());
        history.push("https://second.example".into());
        history.index -= 1;
        assert!(history.state().can_go_forward);
        history.push("https://third.example".into());
        let state = history.state();
        assert_eq!(state.url.as_deref(), Some("https://third.example"));
        assert!(state.can_go_back);
        assert!(!state.can_go_forward);
    }
}
