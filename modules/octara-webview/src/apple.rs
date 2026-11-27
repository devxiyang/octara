//! WKWebView common to AppKit and UIKit.

use crate::protocol::{INLINE_ORIGIN, INLINE_SCHEME, RpcCall, canonical_origin};
use crate::{NavigationDecision, NavigationRequest, NavigationState, WebViewCommand, WebViewEvent, WebViewProps, WebViewSource, emit};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, msg_send};
use objc2_foundation::NSObjectNSKeyValueObserverRegistration;
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSError, NSISO8601DateFormatter, NSJSONSerialization, NSJSONWritingOptions, NSKeyValueObservingOptions,
    NSNull, NSNumber, NSRect, NSString, NSURL, NSURLRequest, NSUUID, ns_string,
};
use objc2_web_kit::{
    WKContentWorld, WKFindConfiguration, WKFindResult, WKFrameInfo, WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
    WKWebViewConfiguration,
};
use octara::module::{ModuleError, NativeViewEmitter};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;
use url::Url;

mod delegate;
mod platform;
use delegate::WebDelegate;
pub(crate) use platform::create;

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
use ios::WKWebView;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

const OBSERVED: [&str; 5] = ["URL", "title", "loading", "canGoBack", "canGoForward"];

fn url(web: &WKWebView) -> Option<String> {
    // SAFETY: The view and its properties are used on the main thread.
    unsafe { web.URL() }.and_then(|url| url.absoluteString()).map(|url| url.to_string())
}

/// Match WebKit's Foundation payload types, including values nested in arrays and objects.
fn json_value(object: &AnyObject, depth: usize) -> Option<Value> {
    if depth >= 128 {
        return None;
    }
    if let Some(text) = object.downcast_ref::<NSString>() {
        return Some(Value::String(text.to_string()));
    }
    if object.downcast_ref::<NSNull>().is_some() {
        return Some(Value::Null);
    }
    if let Some(number) = object.downcast_ref::<NSNumber>() {
        if !number.doubleValue().is_finite() {
            return None;
        }
        // SAFETY: Foundation accepts NSNumber as a JSON fragment and distinguishes booleans from numbers.
        let data = unsafe { NSJSONSerialization::dataWithJSONObject_options_error(number, NSJSONWritingOptions::FragmentsAllowed) };
        return serde_json::from_slice(&data.ok()?.to_vec()).ok();
    }
    if let Some(date) = object.downcast_ref::<NSDate>() {
        return Some(Value::String(NSISO8601DateFormatter::new().stringFromDate(date).to_string()));
    }
    if let Some(array) = object.downcast_ref::<NSArray>() {
        return array.iter().map(|item| json_value(&item, depth + 1)).collect::<Option<_>>().map(Value::Array);
    }
    let dict = object.downcast_ref::<NSDictionary>()?;
    let (keys, values) = dict.to_vecs();
    keys.into_iter()
        .zip(values)
        .map(|(key, value)| Some((key.downcast_ref::<NSString>()?.to_string(), json_value(&value, depth + 1)?)))
        .collect::<Option<_>>()
        .map(Value::Object)
}

struct Events {
    active: Cell<bool>,
    navigation: RefCell<Option<NavigationState>>,
    props: RefCell<WebViewProps>,
    emitter: NativeViewEmitter,
    inline: RefCell<VecDeque<(String, String)>>,
    schemes: BTreeSet<String>,
    generation: Cell<u64>,
    main_request: RefCell<Option<String>>,
}

impl Events {
    fn send(&self, event: WebViewEvent) {
        if self.active.get() {
            emit(&self.emitter, event);
        }
    }

    fn navigation(&self, web: &WKWebView) {
        if !self.active.get() {
            return;
        }
        // SAFETY: The view and its properties are used on the main thread.
        let next = unsafe {
            NavigationState {
                url: url(web),
                title: web.title().map(|title| title.to_string()),
                loading: web.isLoading(),
                can_go_back: web.canGoBack(),
                can_go_forward: web.canGoForward(),
            }
        };
        if self.navigation.borrow().as_ref() != Some(&next) {
            self.navigation.replace(Some(next.clone()));
            self.send(WebViewEvent::Navigation(next));
        }
    }

    fn bump_generation(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    fn decision(&self, request: &NavigationRequest) -> NavigationDecision {
        let inline = self.inline.borrow().iter().any(|(url, _)| url == request.url.split('#').next().unwrap_or_default());
        if Url::parse(&request.url)
            .is_ok_and(|url| !matches!(url.scheme(), "http" | "https" | "data" | INLINE_SCHEME) && !self.schemes.contains(url.scheme()))
        {
            return NavigationDecision::Cancel;
        }
        let props = self.props.borrow().clone();
        props.decision(request, inline)
    }

    fn trusted_url(&self, url: &Url) -> bool {
        if url.scheme() == INLINE_SCHEME {
            let mut url = url.clone();
            url.set_fragment(None);
            return self.inline.borrow().iter().any(|(allowed, _)| allowed == url.as_str());
        }
        let registered = matches!(url.scheme(), "http" | "https") || self.schemes.contains(url.scheme());
        registered && self.props.borrow().origin_is_trusted(url)
    }

    fn trusted_frame(&self, frame: &WKFrameInfo) -> bool {
        // SAFETY: WebKit guarantees valid main-thread frame metadata while invoking its delegate.
        let (request, security_origin) = unsafe {
            if !self.active.get() || !frame.isMainFrame() {
                return false;
            }
            (frame.request(), frame.securityOrigin())
        };
        let Some(url) = request_url(&request).and_then(|url| Url::parse(&url).ok()) else { return false };
        if !self.trusted_url(&url) {
            return false;
        }
        // SAFETY: Security origin belongs to the invoking frame.
        let (scheme, host, port) = unsafe { (security_origin.protocol(), security_origin.host(), security_origin.port()) };
        let mut host = host.to_string();
        if host.contains(':') && !host.starts_with('[') {
            host = format!("[{host}]");
        }
        let origin = if port == 0 { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") };
        let actual = Url::parse(&origin).ok().and_then(|origin| canonical_origin(&origin));
        actual.is_some() && actual == canonical_origin(&url)
    }
}

fn request_url(request: &NSURLRequest) -> Option<String> {
    request.URL()?.absoluteString().map(|url| url.to_string())
}

struct Web {
    view: Retained<WKWebView>,
    delegate: Retained<WebDelegate>,
    controller: Retained<WKUserContentController>,
    loaded: Option<WebViewSource>,
}

impl Web {
    fn new(mtm: MainThreadMarker, props: WebViewProps, emitter: NativeViewEmitter) -> Self {
        let schemes = props
            .schemes
            .keys()
            // SAFETY: Availability of built-in scheme handling is queried on the UI thread.
            .filter(|name| !unsafe { WKWebView::handlesURLScheme(&NSString::from_str(name), mtm) })
            .cloned()
            .collect();
        let events = Rc::new(Events {
            active: Cell::new(true),
            navigation: RefCell::default(),
            schemes,
            props: RefCell::new(props.clone()),
            emitter,
            inline: RefCell::default(),
            generation: Cell::default(),
            main_request: RefCell::default(),
        });
        let delegate = WebDelegate::alloc(mtm).set_ivars(events.clone());
        // SAFETY: Initialization and configuration run on the main thread. Scheme names are
        // unique and WebKit's built-in names were excluded.
        let (view, delegate, controller) = unsafe {
            let delegate: Retained<WebDelegate> = msg_send![super(delegate), init];
            let config = WKWebViewConfiguration::new(mtm);
            // All window requests still pass through the application's UI delegate policy.
            config.preferences().setJavaScriptCanOpenWindowsAutomatically(true);
            let schemes = events.schemes.iter().map(String::as_str).chain([INLINE_SCHEME]);
            for scheme in schemes {
                config.setURLSchemeHandler_forURLScheme(Some(ProtocolObject::from_ref(&*delegate)), &NSString::from_str(scheme));
            }
            let controller = config.userContentController();
            controller.addScriptMessageHandlerWithReply_contentWorld_name(
                ProtocolObject::from_ref(&*delegate),
                &WKContentWorld::pageWorld(mtm),
                ns_string!("octara"),
            );
            controller.addUserScript(&WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(include_str!("apple/bridge.js")),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            ));
            let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config);
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            for key in OBSERVED {
                view.addObserver_forKeyPath_options_context(
                    &delegate,
                    &NSString::from_str(key),
                    NSKeyValueObservingOptions::New,
                    null_mut(),
                );
            }
            (view, delegate, controller)
        };
        let mut web = Self { view, delegate, controller, loaded: None };
        web.update(props);
        web.events().navigation(&web.view);
        web
    }

    fn events(&self) -> &Rc<Events> {
        self.delegate.ivars()
    }

    fn update(&mut self, props: WebViewProps) {
        let source = props.source.clone();
        self.events().props.replace(props);
        if self.loaded.as_ref() == Some(&source) {
            return;
        }
        self.loaded = Some(source.clone());
        self.events().bump_generation();

        let to_url = |text: &str| NSURL::URLWithString(&NSString::from_str(text));
        match source {
            WebViewSource::Html { document, base_url: Some(base) } => {
                let request = NavigationRequest { url: base.clone(), main_frame: true, new_window: false };
                let valid_base =
                    Url::parse(&base).is_ok_and(|url| canonical_origin(&url).is_some() && matches!(url.scheme(), "http" | "https"));
                if !valid_base || self.events().decision(&request) == NavigationDecision::Cancel {
                    self.events().send(WebViewEvent::NavigationBlocked(request));
                } else if let Some(base) = to_url(&base) {
                    // SAFETY: The view is used on the UI thread and WebKit retains its arguments.
                    unsafe { self.view.loadHTMLString_baseURL(&NSString::from_str(&document), Some(&base)) };
                }
            }
            source => {
                let url = match source {
                    WebViewSource::Url(url) => url,
                    WebViewSource::Html { document, .. } => {
                        let uuid = NSUUID::UUID().UUIDString();
                        let url = format!("{INLINE_ORIGIN}/{uuid}");
                        let mut docs = self.events().inline.borrow_mut();
                        // WebKit can revisit recent inline pages after the source property changes.
                        if docs.len() == 32 {
                            docs.pop_front();
                        }
                        docs.push_back((url.clone(), document));
                        url
                    }
                };
                if Url::parse(&url).is_ok()
                    && let Some(url) = to_url(&url)
                {
                    // SAFETY: The view is used on the UI thread and WebKit retains its arguments.
                    unsafe { self.view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
                } else {
                    self.events().send(WebViewEvent::Failed { error: "Invalid web view URL".into() });
                }
            }
        }
    }

    fn busy(&self) -> Option<String> {
        // SAFETY: The view is used on the main thread.
        unsafe { self.view.isLoading() }.then(|| format!("loading {}", url(&self.view).unwrap_or_default()).trim_end().to_owned())
    }

    fn command(&self, command: WebViewCommand) {
        // SAFETY: WebKit navigation and evaluation run on the UI thread.
        unsafe {
            match command {
                WebViewCommand::GoBack => {
                    self.view.goBack();
                }
                WebViewCommand::GoForward => {
                    self.view.goForward();
                }
                WebViewCommand::Reload => {
                    self.view.reload();
                }
                WebViewCommand::StopLoading => self.view.stopLoading(),
                WebViewCommand::Find { query, backwards, case_sensitive } => self.find(&query, backwards, case_sensitive),
                WebViewCommand::ClearFind => self.clear_find(),
                WebViewCommand::Evaluate { script, request_id } => {
                    self.run_script(&script, false, move |result| WebViewEvent::JavaScript { request_id, result });
                }
            }
        }
    }

    fn rpc(&self, call: RpcCall) {
        let args = json!(call.args.to_string());
        let script = format!("return await window.octara.dispatch({}, JSON.parse({args}));", json!(call.method));
        self.run_script(&script, true, |result| call.reply(result));
    }

    fn run_script(&self, script: &str, await_promise: bool, event: impl FnOnce(Result<Value, ModuleError>) -> WebViewEvent + 'static) {
        let Some(url) = url(&self.view).and_then(|url| Url::parse(&url).ok()).filter(|url| self.events().trusted_url(url)) else {
            self.events().send(event(Err(ModuleError::PermissionDenied)));
            return;
        };

        let origin = canonical_origin(&url).expect("trusted pages have a named origin");
        let guard = if matches!(url.scheme(), "http" | "https") {
            format!("window.location.origin === {}", json!(origin))
        } else {
            format!("window.location.protocol + '//' + window.location.host === {}", json!(origin))
        };
        // Recheck inside the executing document. A navigation can replace the frame
        // after the native URL check and before WebKit evaluates this body.
        let body = if await_promise {
            format!("if (!({guard})) throw new Error('Untrusted document');\n{script}")
        } else {
            format!("if ({guard}) {{\n{script}\n}} else {{ throw new Error('Untrusted document'); }}")
        };
        let body = NSString::from_str(&body);
        let events = self.events().clone();
        let generation = events.generation.get();
        let event = Cell::new(Some(event));
        let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            let Some(event) = event.take() else { return };
            if !events.active.get() {
                return;
            }
            // SAFETY: WebKit supplies valid objects or null for the duration of the completion.
            let result = unsafe {
                if generation != events.generation.get() {
                    Err(ModuleError::Cancelled)
                } else if let Some(error) = error.as_ref() {
                    Err(ModuleError::Failed(error.localizedDescription().to_string()))
                } else {
                    value
                        .as_ref()
                        .map_or(Some(Value::Null), |value| json_value(value, 0))
                        .ok_or_else(|| ModuleError::Failed("JavaScript result is not JSON-compatible".into()))
                }
            };
            events.send(event(result));
        });
        // SAFETY: WebKit copies the block and invokes it on the main thread.
        unsafe {
            if await_promise {
                self.view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &body,
                    None,
                    None,
                    &WKContentWorld::pageWorld(MainThreadMarker::from(&*self.view)),
                    Some(&done),
                );
            } else {
                self.view.evaluateJavaScript_completionHandler(&body, Some(&done));
            }
        }
    }

    fn find(&self, query: &str, backwards: bool, case_sensitive: bool) {
        if query.is_empty() {
            self.clear_find();
            return;
        }
        // SAFETY: The block and configuration are used on the UI thread.
        unsafe {
            let config = WKFindConfiguration::new(MainThreadMarker::from(&*self.view));
            config.setBackwards(backwards);
            config.setCaseSensitive(case_sensitive);
            config.setWraps(true);
            let done = RcBlock::new(|_: NonNull<WKFindResult>| {});
            self.view.findString_withConfiguration_completionHandler(&NSString::from_str(query), Some(&config), &done);
        }
    }

    fn clear_find(&self) {
        // WebKit's public find API selects the match and has no clear operation.
        let script = ns_string!(
            "(function clear(view) {
                var selection = view.getSelection();
                if (selection) selection.removeAllRanges();
                for (var i = 0; i < view.length; i++) {
                    try { clear(view.frames[i]); } catch (_) {}
                }
            })(window)"
        );
        // SAFETY: Evaluation runs on the UI thread.
        unsafe { self.view.evaluateJavaScript_completionHandler(script, None) };
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        self.events().active.set(false);
        // SAFETY: Observers are unregistered while both objects are still alive.
        unsafe {
            self.controller.removeScriptMessageHandlerForName(ns_string!("octara"));
            for key in OBSERVED {
                self.view.removeObserver_forKeyPath(&self.delegate, &NSString::from_str(key));
            }
            self.view.setNavigationDelegate(None);
            self.view.setUIDelegate(None);
            self.view.stopLoading();
        }
    }
}
