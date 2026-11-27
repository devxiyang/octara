use crate::protocol::RpcCall;
use crate::{
    HostSurface, NavigationDecision, NavigationRequest, NavigationState, WebViewCommand, WebViewEvent, WebViewProps, WebViewSource, emit,
    unavailable_snapshot, unsupported_command,
};
use octara::module::{
    CallTicket, ModuleDispatcher, ModuleError, NativePlacement, NativeSnapshotRequest, NativeViewEmitter, NativeViewInstance,
};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, HtmlElement, HtmlIFrameElement};

pub(crate) fn create(
    surface: HostSurface<'_>,
    props: WebViewProps,
    emitter: NativeViewEmitter,
) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
    let parent = surface.downcast_ref::<HtmlElement>().ok_or(ModuleError::Unavailable)?;
    let document = parent.owner_document().ok_or(ModuleError::Unavailable)?;
    let iframe: HtmlIFrameElement = document.create_element("iframe").map_err(dom_error)?.unchecked_into();
    // Opaque origin, no popups, top navigation, downloads, or modal permission.
    iframe.set_attribute("sandbox", "allow-scripts allow-forms").map_err(dom_error)?;
    iframe.set_attribute("referrerpolicy", "no-referrer").map_err(dom_error)?;
    iframe.set_title("Web view");
    let style = iframe.style();
    for (key, value) in [("width", "100%"), ("height", "100%"), ("border", "0"), ("display", "block")] {
        style.set_property(key, value).map_err(dom_error)?;
    }

    let current = Rc::new(RefCell::new(None::<NavigationState>));
    let state = current.clone();
    let events = emitter.clone();
    let load = Closure::<dyn FnMut(Event)>::new(move |_| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.loading = false;
            emit(&events, WebViewEvent::Navigation(state.clone()));
            emit(&events, WebViewEvent::Loaded { url: state.url.clone() });
        }
    });
    iframe.add_event_listener_with_callback("load", load.as_ref().unchecked_ref()).map_err(dom_error)?;
    let mut view = Iframe { iframe, load, emitter, current, source: None };
    view.navigate(props);
    parent.append_child(&view.iframe).map_err(dom_error)?;
    Ok(Box::new(view))
}

fn dom_error(error: JsValue) -> ModuleError {
    ModuleError::Failed(error.as_string().unwrap_or_else(|| format!("{error:?}")))
}

struct Iframe {
    iframe: HtmlIFrameElement,
    load: Closure<dyn FnMut(Event)>,
    emitter: NativeViewEmitter,
    current: Rc<RefCell<Option<NavigationState>>>,
    source: Option<WebViewSource>,
}

impl Iframe {
    fn navigate(&mut self, props: WebViewProps) {
        if self.source.as_ref() == Some(&props.source) {
            return;
        }
        let url = match &props.source {
            WebViewSource::Url(url) => Some(url.clone()),
            WebViewSource::Html { base_url, .. } => base_url.clone(),
        };
        if let Some(url) = &url {
            let request = NavigationRequest { url: url.clone(), main_frame: true, new_window: false };
            let supported = url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "data"));
            if !supported || props.decision(&request, false) == NavigationDecision::Cancel {
                emit(&self.emitter, WebViewEvent::NavigationBlocked(request));
                return;
            }
        }
        let state = NavigationState { url, loading: true, ..Default::default() };
        *self.current.borrow_mut() = Some(state.clone());
        self.source = Some(props.source);
        emit(&self.emitter, WebViewEvent::Navigation(state));
        self.load_source();
    }

    fn load_source(&self) {
        match &self.source {
            Some(WebViewSource::Url(url)) => {
                self.iframe.set_src(url);
                let _ = self.iframe.remove_attribute("srcdoc");
            }
            Some(WebViewSource::Html { document, base_url }) => {
                if let Some(url) = base_url {
                    let url = url.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
                    self.iframe.set_srcdoc(&format!("<!doctype html><base href=\"{url}\">{document}"));
                } else {
                    self.iframe.set_srcdoc(document);
                }
                let _ = self.iframe.remove_attribute("src");
            }
            None => {}
        }
    }
}

impl NativeViewInstance for Iframe {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(props) = props.downcast::<WebViewProps>() {
            self.navigate(*props);
        }
    }

    fn place(&mut self, placement: &NativePlacement) {
        let _ = self.iframe.style().set_property("visibility", if placement.visible { "visible" } else { "hidden" });
    }

    fn call(&mut self, _ticket: CallTicket, command: Box<dyn Any>, _reply: ModuleDispatcher) {
        match command.downcast::<WebViewCommand>() {
            Ok(command) if *command == WebViewCommand::Reload => {
                if let Some(state) = self.current.borrow_mut().as_mut() {
                    state.loading = true;
                    emit(&self.emitter, WebViewEvent::Navigation(state.clone()));
                }
                self.load_source();
            }
            Ok(command) => unsupported_command(&self.emitter, *command),
            Err(command) => {
                if let Ok(call) = command.downcast::<RpcCall>() {
                    emit(&self.emitter, call.reply(Err(ModuleError::Unavailable)));
                }
            }
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, _reply: ModuleDispatcher) {
        unavailable_snapshot(&self.emitter, request, "Browser iframe snapshots are unavailable".into());
    }
}

impl Drop for Iframe {
    fn drop(&mut self) {
        let _ = self.iframe.remove_event_listener_with_callback("load", self.load.as_ref().unchecked_ref());
        self.iframe.remove();
    }
}
