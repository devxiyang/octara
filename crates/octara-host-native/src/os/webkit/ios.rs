//! UIKit WKWebView methods omitted by objc2-web-kit 0.3.2.

use block2::DynBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{extern_class, extern_methods};
use objc2_foundation::{NSError, NSRect, NSString, NSURL, NSURLRequest};
use objc2_ui_kit::{UIImage, UIResponder, UIView};
use objc2_web_kit::{
    WKFindConfiguration, WKFindResult, WKNavigation, WKNavigationDelegate, WKSnapshotConfiguration, WKWebViewConfiguration,
};
use std::ptr::NonNull;

extern_class!(
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = objc2::MainThreadOnly]
    pub(crate) struct WKWebView;
);

#[allow(non_snake_case)]
impl WKWebView {
    extern_methods!(
        #[unsafe(method(initWithFrame:configuration:))]
        #[unsafe(method_family = init)]
        pub(crate) unsafe fn initWithFrame_configuration(
            this: Allocated<Self>,
            frame: NSRect,
            configuration: &WKWebViewConfiguration,
        ) -> Retained<Self>;

        #[unsafe(method(setNavigationDelegate:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn setNavigationDelegate(&self, delegate: Option<&ProtocolObject<dyn WKNavigationDelegate>>);

        #[unsafe(method(loadRequest:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn loadRequest(&self, request: &NSURLRequest) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(loadHTMLString:baseURL:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn loadHTMLString_baseURL(&self, html: &NSString, base: Option<&NSURL>) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(title))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn title(&self) -> Option<Retained<NSString>>;

        #[unsafe(method(URL))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn URL(&self) -> Option<Retained<NSURL>>;

        #[unsafe(method(isLoading))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn isLoading(&self) -> bool;

        #[unsafe(method(canGoBack))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn canGoBack(&self) -> bool;

        #[unsafe(method(canGoForward))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn canGoForward(&self) -> bool;

        #[unsafe(method(goBack))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn goBack(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(goForward))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn goForward(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(reload))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn reload(&self) -> Option<Retained<WKNavigation>>;

        #[unsafe(method(stopLoading))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn stopLoading(&self);

        #[unsafe(method(evaluateJavaScript:completionHandler:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn evaluateJavaScript_completionHandler(
            &self,
            script: &NSString,
            completion: Option<&DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>>,
        );

        #[unsafe(method(findString:withConfiguration:completionHandler:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn findString_withConfiguration_completionHandler(
            &self,
            query: &NSString,
            configuration: Option<&WKFindConfiguration>,
            completion: &DynBlock<dyn Fn(NonNull<WKFindResult>)>,
        );

        #[unsafe(method(takeSnapshotWithConfiguration:completionHandler:))]
        #[unsafe(method_family = none)]
        pub(crate) unsafe fn takeSnapshotWithConfiguration_completionHandler(
            &self,
            configuration: Option<&WKSnapshotConfiguration>,
            completion: &DynBlock<dyn Fn(*mut UIImage, *mut NSError)>,
        );
    );
}
