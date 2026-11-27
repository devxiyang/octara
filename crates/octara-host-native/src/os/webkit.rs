//! The WebKit command, event and source protocol shared by AppKit and UIKit.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSError, NSISO8601DateFormatter, NSJSONSerialization, NSJSONWritingOptions, NSKeyValueChangeKey,
    NSKeyValueObservingOptions, NSNull, NSNumber, NSObjectNSKeyValueObserverRegistration, NSRect, NSString, NSURL, NSURLRequest, ns_string,
};
use objc2_web_kit::{
    WKFindConfiguration, WKFindResult, WKNavigation, WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler,
    WKUserContentController, WKWebViewConfiguration,
};
use octara_model::native::NativeEvent;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;

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

#[derive(PartialEq)]
struct Navigation {
    url: Option<String>,
    title: Option<String>,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
}

type SendEvent = Box<dyn Fn(NativeEvent)>;

struct Events {
    active: Cell<bool>,
    navigation: RefCell<Option<Navigation>>,
    send: SendEvent,
}

impl Events {
    fn send(&self, name: &str, data: Value) {
        if self.active.get() {
            (self.send)(NativeEvent { name: name.into(), data });
        }
    }

    fn navigation(&self, web: &WKWebView) {
        if !self.active.get() {
            return;
        }
        // SAFETY: The view and its properties are used on the main thread.
        let next = unsafe {
            Navigation {
                url: url(web),
                title: web.title().map(|title| title.to_string()),
                loading: web.isLoading(),
                can_go_back: web.canGoBack(),
                can_go_forward: web.canGoForward(),
            }
        };
        let (title, navigation) = {
            let mut remembered = self.navigation.borrow_mut();
            if remembered.as_ref() == Some(&next) {
                return;
            }
            let previous_title = remembered.as_ref().and_then(|previous| previous.title.as_deref());
            let title = (previous_title != next.title.as_deref()).then(|| json!({ "title": next.title.as_deref().unwrap_or_default() }));
            let navigation = json!({
                "url": next.url,
                "title": next.title,
                "loading": next.loading,
                "can_go_back": next.can_go_back,
                "can_go_forward": next.can_go_forward,
            });
            *remembered = Some(next);
            (title, navigation)
        };
        if let Some(title) = title {
            self.send("title", title);
        }
        self.send("navigation", navigation);
    }

    fn js_result(&self, request_id: Option<&str>, result: Result<Value, String>) {
        match result {
            Ok(value) => self.send("js_result", json!({ "request_id": request_id, "value": value })),
            Err(error) => self.send("js_result", json!({ "request_id": request_id, "error": error })),
        }
    }

    fn navigation_failed(&self, web: &WKWebView, error: &NSError) {
        self.navigation(web);
        self.send("fail", json!({ "error": error.localizedDescription().to_string() }));
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. WebKit calls these selectors on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWebDelegate"]
    #[ivars = Rc<Events>]
    struct WebDelegate;

    impl WebDelegate {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(web) = object.and_then(|object| object.downcast_ref::<WKWebView>()) {
                self.ivars().navigation(web);
            }
        }

        // objc2 omits the optional navigation selectors on iOS. Their Apple ABI is shared.
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finish(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().navigation(web);
            self.ivars().send("load", json!({ "url": url(web) }));
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn fail(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.ivars().navigation_failed(web, error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn fail_provisional(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.ivars().navigation_failed(web, error);
        }
    }

    unsafe impl NSObjectProtocol for WebDelegate {}
    unsafe impl WKNavigationDelegate for WebDelegate {}

    unsafe impl WKScriptMessageHandler for WebDelegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn receive_message(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: WebKit provides a valid message for the duration of the callback.
            let body = unsafe { message.body() };
            if let Some(value) = json_value(&body, 0) {
                self.ivars().send("post_message", value);
            }
        }
    }
);

pub(super) struct Web {
    view: Retained<WKWebView>,
    delegate: Retained<WebDelegate>,
    controller: Retained<WKUserContentController>,
    loaded: Option<Value>,
}

impl Web {
    pub(super) fn new(mtm: MainThreadMarker, props: &Value, send: impl Fn(NativeEvent) + 'static) -> Self {
        let events = Rc::new(Events { active: Cell::new(true), navigation: RefCell::default(), send: Box::new(send) });
        let delegate = WebDelegate::alloc(mtm).set_ivars(events);
        // SAFETY: Initialization and all configuration run on the UI thread.
        let (view, delegate, controller) = unsafe {
            let delegate: Retained<WebDelegate> = msg_send![super(delegate), init];
            let config = WKWebViewConfiguration::new(mtm);
            let controller = config.userContentController();
            controller.addScriptMessageHandler_name(ProtocolObject::from_ref(&*delegate), ns_string!("octara"));
            let view = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), NSRect::ZERO, &config);
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
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
        web.delegate.ivars().navigation(&web.view);
        web
    }

    pub(super) fn view(&self) -> &Retained<WKWebView> {
        &self.view
    }

    pub(super) fn update(&mut self, props: &Value) {
        let source = json!({ "url": props.get("url"), "html": props.get("html"), "base_url": props.get("base_url") });
        if self.loaded.as_ref() == Some(&source) {
            return;
        }
        self.loaded = Some(source);
        let url = |s: &str| NSURL::URLWithString(&NSString::from_str(s));
        // SAFETY: The view takes valid retained strings, URLs and URL requests.
        unsafe {
            if let Some(html) = props.get("html").and_then(Value::as_str) {
                let base = props.get("base_url").and_then(Value::as_str).and_then(url);
                self.view.loadHTMLString_baseURL(&NSString::from_str(html), base.as_deref());
            } else if let Some(url) = props.get("url").and_then(Value::as_str).and_then(url) {
                self.view.loadRequest(&NSURLRequest::requestWithURL(&url));
            }
        }
    }

    pub(super) fn busy(&self) -> Option<String> {
        // SAFETY: The view is used on the main thread.
        unsafe { self.view.isLoading() }.then(|| format!("loading {}", url(&self.view).unwrap_or_default()).trim_end().to_owned())
    }

    pub(super) fn command(&self, name: &str, data: &Value) {
        // SAFETY: WebKit navigation and evaluation run on the UI thread.
        unsafe {
            match name {
                "go_back" => {
                    self.view.goBack();
                }
                "go_forward" => {
                    self.view.goForward();
                }
                "reload" => {
                    self.view.reload();
                }
                "stop_loading" => self.view.stopLoading(),
                "evaluate_js" => self.evaluate(data),
                "find" => {
                    if let Some(query) = data.get("query").and_then(Value::as_str) {
                        let backwards = data.get("backwards").and_then(Value::as_bool).unwrap_or_default();
                        let case_sensitive = data.get("case_sensitive").and_then(Value::as_bool).unwrap_or_default();
                        self.find(query, backwards, case_sensitive);
                    }
                }
                "clear_find" => self.clear_find(),
                _ => {}
            }
        }
    }

    fn evaluate(&self, data: &Value) {
        let request_id = data.get("request_id").and_then(Value::as_str);
        let Some(script) = data.get("script").and_then(Value::as_str) else {
            self.delegate.ivars().js_result(request_id, Err("evaluate_js requires a string script".into()));
            return;
        };
        let events = self.delegate.ivars().clone();
        let request_id = request_id.map(str::to_owned);
        let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            if !events.active.get() {
                return;
            }
            // SAFETY: WebKit supplies valid objects or null for the duration of the completion.
            let result = unsafe {
                if let Some(error) = error.as_ref() {
                    Err(error.localizedDescription().to_string())
                } else {
                    value
                        .as_ref()
                        .map_or(Some(Value::Null), |value| json_value(value, 0))
                        .ok_or_else(|| "JavaScript result is not JSON-compatible".into())
                }
            };
            events.js_result(request_id.as_deref(), result);
        });
        // SAFETY: WebKit copies the block and invokes it on the main thread.
        unsafe { self.view.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&done)) };
    }

    fn find(&self, query: &str, backwards: bool, case_sensitive: bool) {
        if query.is_empty() {
            self.clear_find();
        } else if objc2::available!(macos = 11.0, ios = 14.0) {
            // SAFETY: Availability was checked and the blocks and configuration are used on the UI thread.
            unsafe {
                let config = WKFindConfiguration::new(MainThreadMarker::from(&*self.view));
                config.setBackwards(backwards);
                config.setCaseSensitive(case_sensitive);
                config.setWraps(true);
                let done = RcBlock::new(|_: NonNull<WKFindResult>| {});
                self.view.findString_withConfiguration_completionHandler(&NSString::from_str(query), Some(&config), &done);
            }
        } else {
            let query = json!(query);
            let script = format!("window.find({query}, {case_sensitive}, {backwards}, true, false, true, false)");
            // SAFETY: The query is encoded as a JSON string, and the evaluation runs on the UI thread.
            unsafe { self.view.evaluateJavaScript_completionHandler(&NSString::from_str(&script), None) };
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
        self.delegate.ivars().active.set(false);
        // SAFETY: Observers are unregistered while both objects are still alive.
        unsafe {
            self.controller.removeScriptMessageHandlerForName(ns_string!("octara"));
            for key in OBSERVED {
                self.view.removeObserver_forKeyPath(&self.delegate, &NSString::from_str(key));
            }
            self.view.setNavigationDelegate(None);
            self.view.stopLoading();
        }
    }
}
