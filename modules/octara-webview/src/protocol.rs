use octara::module::ModuleError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

pub(crate) const INLINE_SCHEME: &str = "octara-inline";
pub(crate) const INLINE_ORIGIN: &str = "octara-inline://document";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebViewSource {
    Url(String),
    Html { document: String, base_url: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavigationState {
    pub url: Option<String>,
    pub title: Option<String>,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationRequest {
    pub url: String,
    pub main_frame: bool,
    pub new_window: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationDecision {
    Allow,
    Cancel,
    /// For a new window request, load the destination in this view.
    OpenInView,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogRequest {
    Alert { message: String },
    Confirm { message: String },
    Prompt { message: String, default_text: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogResponse {
    Dismiss,
    Confirm(bool),
    Text(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePickerRequest {
    pub allows_multiple: bool,
    pub allows_directories: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeRequest {
    pub url: String,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeResponse {
    pub content_type: String,
    pub body: Vec<u8>,
}

/// A JSON-compatible RPC contract. The method name is shared with the JavaScript page.
pub trait RpcMethod: 'static {
    const NAME: &'static str;
    type Input: Serialize + DeserializeOwned;
    type Output: Serialize + DeserializeOwned;
}

pub(crate) struct RpcCall {
    pub(crate) method: &'static str,
    pub(crate) request_id: String,
    #[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
    pub(crate) args: Value,
    method_type: TypeId,
}

impl RpcCall {
    pub(crate) fn new<M: RpcMethod>(request_id: String, args: M::Input) -> Result<Self, ModuleError> {
        let args = serde_json::to_value(args).map_err(|error| ModuleError::InvalidArgs(error.to_string()))?;
        #[cfg(not(all(feature = "platform", any(target_os = "macos", target_os = "ios"))))]
        drop(args);
        Ok(Self {
            method: M::NAME,
            request_id,
            #[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
            args,
            method_type: TypeId::of::<M>(),
        })
    }

    pub(crate) fn reply(self, result: Result<Value, ModuleError>) -> WebViewEvent {
        WebViewEvent::Rpc(RpcReply { method: self.method, request_id: self.request_id, method_type: self.method_type, result })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcReply {
    pub method: &'static str,
    pub request_id: String,
    method_type: TypeId,
    result: Result<Value, ModuleError>,
}

impl RpcReply {
    pub fn decode<M: RpcMethod>(&self) -> Result<M::Output, ModuleError> {
        if self.method_type != TypeId::of::<M>() {
            return Err(ModuleError::InvalidArgs("unexpected RPC response type".into()));
        }
        serde_json::from_value(self.result.clone()?).map_err(|error| ModuleError::Failed(error.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebViewCommand {
    GoBack,
    GoForward,
    Reload,
    StopLoading,
    Find {
        query: String,
        backwards: bool,
        case_sensitive: bool,
    },
    ClearFind,
    /// Evaluate in the trusted main frame and emit `WebViewEvent::JavaScript`.
    Evaluate {
        script: String,
        request_id: Option<String>,
    },
}

impl WebViewCommand {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::GoBack => "go_back",
            Self::GoForward => "go_forward",
            Self::Reload => "reload",
            Self::StopLoading => "stop_loading",
            Self::Find { .. } => "find",
            Self::ClearFind => "clear_find",
            Self::Evaluate { .. } => "evaluate",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WebViewEvent {
    Navigation(NavigationState),
    Loaded { url: Option<String> },
    Failed { error: String },
    NavigationBlocked(NavigationRequest),
    NewWindowRequested(NavigationRequest),
    PostMessage(Value),
    JavaScript { request_id: Option<String>, result: Result<Value, ModuleError> },
    Rpc(RpcReply),
    CommandFailed { command: &'static str, error: ModuleError },
}

impl WebViewEvent {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Navigation(_) => "navigation",
            Self::Loaded { .. } => "load",
            Self::Failed { .. } => "fail",
            Self::NavigationBlocked(_) => "navigation_blocked",
            Self::NewWindowRequested(_) => "new_window",
            Self::PostMessage(_) => "post_message",
            Self::JavaScript { .. } => "js_result",
            Self::Rpc(_) => "rpc_result",
            Self::CommandFailed { .. } => "command_failed",
        }
    }
}

type Handler<I, O> = Arc<dyn Fn(I) -> O + Send + Sync>;
type RpcHandler = Handler<Value, Result<Value, ModuleError>>;
type SchemeHandler = Handler<SchemeRequest, Result<SchemeResponse, ModuleError>>;

#[derive(Clone)]
pub struct WebViewProps {
    pub(crate) source: WebViewSource,
    pub(crate) trusted: BTreeSet<String>,
    navigation: Option<Handler<NavigationRequest, NavigationDecision>>,
    pub(crate) dialog: Option<Handler<DialogRequest, DialogResponse>>,
    pub(crate) picker: Option<Handler<FilePickerRequest, Option<Vec<PathBuf>>>>,
    pub(crate) schemes: BTreeMap<String, SchemeHandler>,
    pub(crate) rpc: BTreeMap<&'static str, RpcHandler>,
}

impl WebViewProps {
    fn new(source: WebViewSource) -> Self {
        Self {
            source,
            trusted: BTreeSet::new(),
            navigation: None,
            dialog: None,
            picker: None,
            schemes: BTreeMap::new(),
            rpc: BTreeMap::new(),
        }
    }

    pub fn url(url: impl Into<String>) -> Self {
        Self::new(WebViewSource::Url(url.into()))
    }

    /// Displays app-owned HTML. Only use this with content the app trusts to execute.
    /// WebKit retains the 32 most recent private inline documents for history navigation.
    pub fn html(document: impl Into<String>) -> Self {
        Self::new(WebViewSource::Html { document: document.into(), base_url: None })
    }

    /// Resolves relative URLs for inline HTML. The base origin must be explicitly trusted to use the bridge.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        if let WebViewSource::Html { base_url, .. } = &mut self.source {
            *base_url = Some(url.into());
        }
        self
    }

    /// Trusts a complete HTTP(S) origin or an app-owned custom scheme origin.
    /// Invalid origins, credentials, paths, query strings and fragments are ignored.
    pub fn trust_origin(mut self, url: &str) -> Self {
        if let Ok(url) = Url::parse(url)
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none()
            && let Some(origin) = canonical_origin(&url)
        {
            self.trusted.insert(origin);
        }
        self
    }

    /// Runs synchronously for navigation actions. Ordinary HTTP(S) and data
    /// documents are allowed by default, and popup destinations are cancelled by default.
    pub fn on_navigation(mut self, policy: impl Fn(NavigationRequest) -> NavigationDecision + Send + Sync + 'static) -> Self {
        self.navigation = Some(Arc::new(policy));
        self
    }

    pub fn on_dialog(mut self, handler: impl Fn(DialogRequest) -> DialogResponse + Send + Sync + 'static) -> Self {
        self.dialog = Some(Arc::new(handler));
        self
    }

    /// Supplies local paths for a trusted document's file input. Return `None` to cancel.
    /// The custom open panel callback is supported on macOS and iOS 18.4+.
    /// Earlier iOS releases manage web file inputs with WebKit's system picker.
    pub fn on_file_picker(mut self, handler: impl Fn(FilePickerRequest) -> Option<Vec<PathBuf>> + Send + Sync + 'static) -> Self {
        self.picker = Some(Arc::new(handler));
        self
    }

    /// Registers an application-owned URL scheme, fixed for the lifetime of a mounted view.
    /// Use a lowercase private scheme name. Requests run synchronously on the UI
    /// thread. Any loaded page can request these resources.
    pub fn with_scheme(
        mut self,
        scheme: &str,
        handler: impl Fn(SchemeRequest) -> Result<SchemeResponse, ModuleError> + Send + Sync + 'static,
    ) -> Self {
        if scheme.starts_with(|ch: char| ch.is_ascii_lowercase())
            && scheme.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '+' | '-' | '.'))
            && !matches!(scheme, INLINE_SCHEME | "http" | "https" | "data" | "file" | "about" | "blob" | "javascript")
        {
            self.schemes.insert(scheme.to_owned(), Arc::new(handler));
        }
        self
    }

    /// Registers the Rust handler for `octara.call(M::NAME, input)` in a trusted document.
    pub fn on_rpc<M: RpcMethod>(mut self, handler: impl Fn(M::Input) -> Result<M::Output, ModuleError> + Send + Sync + 'static) -> Self {
        self.rpc.insert(
            M::NAME,
            Arc::new(move |args| {
                let args = serde_json::from_value(args).map_err(|error| ModuleError::InvalidArgs(error.to_string()))?;
                serde_json::to_value(handler(args)?).map_err(|error| ModuleError::Failed(error.to_string()))
            }),
        );
        self
    }

    pub(crate) fn decision(&self, request: &NavigationRequest, inline: bool) -> NavigationDecision {
        let Ok(url) = Url::parse(&request.url) else { return NavigationDecision::Cancel };
        let scheme_allowed = match url.scheme() {
            "http" | "https" | "data" => true,
            INLINE_SCHEME => inline && canonical_origin(&url).as_deref() == Some(INLINE_ORIGIN),
            scheme => self.schemes.contains_key(scheme),
        };
        if !scheme_allowed {
            return NavigationDecision::Cancel;
        }
        match &self.navigation {
            Some(policy) => policy(request.clone()),
            None if request.new_window => NavigationDecision::Cancel,
            None => NavigationDecision::Allow,
        }
    }

    #[cfg(any(test, all(feature = "platform", any(target_os = "macos", target_os = "ios"))))]
    pub(crate) fn origin_is_trusted(&self, url: &Url) -> bool {
        let Some(origin) = canonical_origin(url) else { return false };
        self.trusted.contains(&origin) && (matches!(url.scheme(), "http" | "https") || self.schemes.contains_key(url.scheme()))
    }
}

pub(crate) fn canonical_origin(url: &Url) -> Option<String> {
    if !url.username().is_empty() || url.password().is_some() || matches!(url.scheme(), "file" | "blob" | "about" | "data" | "javascript") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let mut origin = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    Some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sum;

    impl RpcMethod for Sum {
        const NAME: &'static str = "sum";
        type Input = Vec<u32>;
        type Output = u32;
    }

    struct DifferentSum;

    impl RpcMethod for DifferentSum {
        const NAME: &'static str = Sum::NAME;
        type Input = Vec<u32>;
        type Output = u32;
    }

    #[test]
    fn typed_rpc_validates_payload_and_response_type() {
        let props = WebViewProps::html("").on_rpc::<Sum>(|values| Ok(values.into_iter().sum()));
        let handler = &props.rpc[Sum::NAME];
        assert_eq!(handler(serde_json::json!([1, 2])).unwrap(), 3);
        assert!(matches!(handler(serde_json::json!({"amount": 3})), Err(ModuleError::InvalidArgs(_))));

        let call = RpcCall::new::<Sum>("request".into(), vec![1, 2]).unwrap();
        let WebViewEvent::Rpc(reply) = call.reply(handler(serde_json::json!([1, 2]))) else { panic!("expected RPC event") };
        assert_eq!(reply.decode::<Sum>(), Ok(3));
        assert!(matches!(reply.decode::<DifferentSum>(), Err(ModuleError::InvalidArgs(_))));
    }

    #[test]
    fn trust_matches_canonical_scheme_host_and_port() {
        let props = WebViewProps::html("")
            .trust_origin("https://APP.example:443")
            .trust_origin("https://credentials@other.example")
            .trust_origin("https://path.example/path")
            .trust_origin("https://query.example/?value=1");
        assert_eq!(props.trusted.iter().collect::<Vec<_>>(), vec!["https://app.example"]);
        for (url, expected) in [
            ("https://app.example/path#fragment", true),
            ("https://app.example:444/path", false),
            ("http://app.example/path", false),
            ("https://app.example.evil.example", false),
            ("https://unrelated@app.example", false),
            ("https://app.example@evil.example", false),
            ("data:text/html,hello", false),
            ("blob:https://app.example/uuid", false),
        ] {
            let matching = Url::parse(url).is_ok_and(|url| props.origin_is_trusted(&url));
            assert_eq!(matching, expected, "{url}");
        }
    }

    #[test]
    fn navigation_policy_cannot_enable_unregistered_or_reserved_schemes() {
        let props = WebViewProps::html("")
            .on_navigation(|_| NavigationDecision::Allow)
            .with_scheme("example-assets", |_| Ok(SchemeResponse { content_type: "text/plain".into(), body: vec![] }));
        for (url, allowed) in [
            ("https://example.org", true),
            ("file:///tmp/example.html", false),
            ("javascript:alert(1)", false),
            ("example-assets://app/image.png", true),
            ("unregistered://app/image.png", false),
            ("octara-inline://document/not-owned", false),
        ] {
            let request = NavigationRequest { url: url.into(), main_frame: true, new_window: false };
            assert_eq!(props.decision(&request, false) == NavigationDecision::Allow, allowed, "{url}");
        }
    }
}
