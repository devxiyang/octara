use super::{Events, WKWebView, request_url, url};
use crate::protocol::INLINE_SCHEME;
use crate::{
    DialogRequest, DialogResponse, FilePickerRequest, NavigationDecision, NavigationRequest, SchemeRequest, SchemeResponse, WebViewEvent,
};
use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, define_class};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSKeyValueChangeKey, NSString, NSURL, NSURLResponse, ns_string};
use objc2_web_kit::{
    WKFrameInfo, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKOpenPanelParameters, WKScriptMessage, WKScriptMessageHandlerWithReply, WKUIDelegate, WKURLSchemeHandler,
    WKURLSchemeTask, WKUserContentController, WKWebViewConfiguration, WKWindowFeatures,
};
use octara::module::ModuleError;
use serde::Deserialize;
use serde_json::Value;
use std::ffi::{CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;
use url::Url;

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Message {
    Call { method: String, args: Value },
    Event { payload: Value },
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. WebKit calls these delegates on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraModuleWebViewDelegate"]
    #[ivars = Rc<Events>]
    pub(super) struct WebDelegate;

    unsafe impl NSObjectProtocol for WebDelegate {}
    unsafe impl WKNavigationDelegate for WebDelegate {}
    unsafe impl WKUIDelegate for WebDelegate {}

    // The UIKit bindings omit protocol selectors containing WKWebView.
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

        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn navigation_action(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            complete: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: WebKit provides a valid action and completion during the callback.
            let (target, request) = unsafe { (action.targetFrame(), action.request()) };
            let policy = if let Some(target) = target {
                let request = NavigationRequest {
                    url: request_url(&request).unwrap_or_default(),
                    // SAFETY: The target was returned by this navigation action.
                    main_frame: unsafe { target.isMainFrame() },
                    new_window: false,
                };
                if self.allow(&request) {
                    if request.main_frame {
                        self.ivars().main_request.replace(Some(request.url));
                    }
                    WKNavigationActionPolicy::Allow
                } else {
                    WKNavigationActionPolicy::Cancel
                }
            } else {
                // The UI delegate handles window.open and target=_blank exactly once.
                WKNavigationActionPolicy::Allow
            };
            complete.call((policy,));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn navigation_response(
            &self,
            _web: &WKWebView,
            response: &WKNavigationResponse,
            complete: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            // SAFETY: WebKit provides a valid response and completion during the callback.
            let (main_frame, url) = unsafe { (response.isForMainFrame(), response.response().URL()) };
            let url = url.and_then(|url| url.absoluteString()).map(|url| url.to_string()).unwrap_or_default();
            // An HTTP redirect can present a distinct URL without a new navigation action.
            let approved = !main_frame || self.ivars().main_request.borrow().as_deref() == Some(&url);
            let allowed = approved || self.allow(&NavigationRequest { url, main_frame, new_window: false });
            complete.call((if allowed { WKNavigationResponsePolicy::Allow } else { WKNavigationResponsePolicy::Cancel },));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn start(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().bump_generation();
            self.ivars().navigation(web);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finish(&self, web: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().navigation(web);
            self.ivars().send(WebViewEvent::Loaded { url: url(web) });
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn fail(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.failure(web, error);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn fail_provisional(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.failure(web, error);
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn open(
            &self,
            web: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: WebKit supplies the request and web view on the UI thread.
            let native = unsafe { action.request() };
            let request = NavigationRequest { url: request_url(&native).unwrap_or_default(), main_frame: true, new_window: true };
            self.ivars().send(WebViewEvent::NewWindowRequested(request.clone()));
            if self.ivars().decision(&request) == NavigationDecision::OpenInView {
                // SAFETY: Loading in the originating view implements the chosen popup route.
                unsafe { web.loadRequest(&native) };
            } else {
                self.ivars().send(WebViewEvent::NavigationBlocked(request));
            }
            None
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert(&self, _web: &WKWebView, message: &NSString, frame: &WKFrameInfo, done: &DynBlock<dyn Fn()>) {
            self.dialog(frame, DialogRequest::Alert { message: message.to_string() });
            done.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm(
            &self, _web: &WKWebView, message: &NSString, frame: &WKFrameInfo, done: &DynBlock<dyn Fn(Bool)>
        ) {
            let response = self.dialog(frame, DialogRequest::Confirm { message: message.to_string() });
            done.call((Bool::new(response == DialogResponse::Confirm(true)),));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt(
            &self, _web: &WKWebView, message: &NSString, default: Option<&NSString>, frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSString)>
        ) {
            let request = DialogRequest::Prompt { message: message.to_string(), default_text: default.map(ToString::to_string) };
            let text = match self.dialog(frame, request) {
                DialogResponse::Text(Some(text)) => Some(NSString::from_str(&text)),
                _ => None,
            };
            done.call((text.as_ref().map_or(null_mut(), |text| Retained::as_ptr(text).cast_mut()),));
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn pick_files(
            &self, _web: &WKWebView, params: &WKOpenPanelParameters, frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>
        ) {
            let urls = self.picked_urls(frame, params);
            done.call((urls.as_ref().map_or(null_mut(), |urls| Retained::as_ptr(urls).cast_mut()),));
        }

    }

    unsafe impl WKURLSchemeHandler for WebDelegate {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_scheme_task(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // SAFETY: WebKit holds the task alive and expects exactly one terminal callback. This
            // handler completes synchronously, so stopURLSchemeTask has no in-flight work to cancel.
            unsafe {
                let request = task.request();
                let native_url = request.URL();
                if let (Some(url), Ok(response)) = (native_url, self.scheme_response(&request)) {
                    let mime = NSString::from_str(&response.content_type);
                    let native = NSURLResponse::initWithURL_MIMEType_expectedContentLength_textEncodingName(
                        NSURLResponse::alloc(), &url, Some(&mime), isize::try_from(response.body.len()).unwrap_or(-1),
                        Some(ns_string!("utf-8")),
                    );
                    task.didReceiveResponse(&native);
                    task.didReceiveData(&NSData::from_vec(response.body));
                    task.didFinish();
                } else {
                    let error = NSError::errorWithDomain_code_userInfo(ns_string!("OctaraWebViewScheme"), 1, None);
                    task.didFailWithError(&error);
                }
            }
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_scheme_task(&self, _web: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }

    unsafe impl WKScriptMessageHandlerWithReply for WebDelegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:replyHandler:))]
        fn receive(
            &self, _controller: &WKUserContentController, message: &WKScriptMessage, done: &DynBlock<dyn Fn(*mut AnyObject, *mut NSString)>
        ) {
            match self.message(message) {
                Ok(reply) => {
                    let reply = NSString::from_str(&reply.to_string());
                    done.call((Retained::as_ptr(&reply).cast::<AnyObject>().cast_mut(), null_mut()));
                }
                Err(error) => {
                    let error = NSString::from_str(&error.to_string());
                    done.call((null_mut(), Retained::as_ptr(&error).cast_mut()));
                }
            }
        }
    }
);

impl WebDelegate {
    fn allow(&self, request: &NavigationRequest) -> bool {
        if self.ivars().decision(request) == NavigationDecision::Cancel {
            self.ivars().send(WebViewEvent::NavigationBlocked(request.clone()));
            return false;
        }
        true
    }

    fn failure(&self, web: &WKWebView, error: &NSError) {
        self.ivars().navigation(web);
        // NSURLErrorCancelled also accompanies application policy cancellations.
        if error.code() != -999 || error.domain().to_string() != "NSURLErrorDomain" {
            self.ivars().send(WebViewEvent::Failed { error: error.localizedDescription().to_string() });
        }
    }

    fn dialog(&self, frame: &WKFrameInfo, request: DialogRequest) -> DialogResponse {
        if self.ivars().trusted_frame(frame)
            && let Some(handler) = self.ivars().props.borrow().dialog.clone()
        {
            return handler(request);
        }
        DialogResponse::Dismiss
    }

    fn picked_urls(&self, frame: &WKFrameInfo, params: &WKOpenPanelParameters) -> Option<Retained<NSArray<NSURL>>> {
        if !self.ivars().trusted_frame(frame) {
            return None;
        }
        let handler = self.ivars().props.borrow().picker.clone()?;
        // SAFETY: WebKit calls this method only on supported versions and supplies its open panel parameters.
        let request = unsafe {
            FilePickerRequest { allows_multiple: params.allowsMultipleSelection(), allows_directories: params.allowsDirectories() }
        };
        let paths = handler(request)?;
        let limit = if request.allows_multiple { usize::MAX } else { 1 };
        let urls: Vec<_> = paths
            .into_iter()
            .filter_map(|path| {
                let directory = path.is_dir();
                if !path.is_absolute() || (directory && !request.allows_directories) {
                    return None;
                }
                let path = CString::new(path.as_os_str().as_bytes()).ok()?;
                let raw = NonNull::new(path.as_ptr().cast_mut())?;
                // SAFETY: The C string is valid for the duration of the call and Foundation copies it.
                Some(unsafe { NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(raw, directory, None) })
            })
            .take(limit)
            .collect();
        (!urls.is_empty()).then(|| NSArray::from_retained_slice(&urls))
    }

    fn scheme_response(&self, request: &objc2_foundation::NSURLRequest) -> Result<SchemeResponse, ModuleError> {
        let events = self.ivars();
        if !events.active.get() {
            return Err(ModuleError::Cancelled);
        }
        let url = request_url(request).ok_or(ModuleError::Unavailable)?;
        let parsed = Url::parse(&url).map_err(|error| ModuleError::InvalidArgs(error.to_string()))?;
        if parsed.scheme() == INLINE_SCHEME {
            let docs = events.inline.borrow();
            let document = docs.iter().find(|(known, _)| known == &url).ok_or(ModuleError::Unavailable)?;
            return Ok(SchemeResponse { content_type: "text/html".into(), body: document.1.as_bytes().to_vec() });
        }
        let handler = events.props.borrow().schemes.get(parsed.scheme()).cloned().ok_or(ModuleError::Unavailable)?;
        let method = request.HTTPMethod().map_or_else(|| "GET".into(), |method| method.to_string());
        handler(SchemeRequest { url, method })
    }

    fn message(&self, message: &WKScriptMessage) -> Result<Value, ModuleError> {
        let events = self.ivars();
        // SAFETY: WebKit supplies a live message and frame on the main thread.
        let frame = unsafe { message.frameInfo() };
        if !events.trusted_frame(&frame) {
            return Err(ModuleError::PermissionDenied);
        }
        // SAFETY: WebKit retains the returned body.
        let body = unsafe { message.body() };
        let text = body.downcast_ref::<NSString>().ok_or_else(|| ModuleError::InvalidArgs("expected a JSON string".into()))?.to_string();
        match serde_json::from_str(&text).map_err(|error| ModuleError::InvalidArgs(error.to_string()))? {
            Message::Call { method, args } => {
                let handler = events.props.borrow().rpc.get(method.as_str()).cloned().ok_or(ModuleError::Unavailable)?;
                handler(args)
            }
            Message::Event { payload } => {
                events.emitter.emit("post_message", WebViewEvent::PostMessage(payload), None)?;
                Ok(Value::Null)
            }
        }
    }
}
