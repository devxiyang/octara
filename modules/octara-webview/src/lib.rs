//! A typed web view with WebKit, sandboxed browser iframe, and deterministic mock backends.
//!
//! Register [module] before building a [view], and listen with
//! `El::on_typed_native::<WebViewEvent>`. Only trusted main-frame documents receive
//! native bridge privileges. Inline HTML is app-owned. For an HTML base URL or a
//! remote document, explicitly trust the origin that serves the bridge code.
//!
//! The browser backend isolates its iframe using a sandbox without
//! `allow-same-origin`. Browser framing policy (including CSP and X-Frame-Options)
//! still applies. Iframe load notifications identify the last source requested
//! by the app and can fire for pages blocked by framing policy. The browser
//! supports reloading that source. History, internal navigation policy,
//! custom schemes, dialog/file hooks, evaluation, RPC and snapshots are unavailable.

use octara::WriteCx;
use octara::module::{
    AirspacePolicy, CallTicket, HostSurface, HostSurfaceKind, ModuleCallCx, ModuleError, ModuleId, ModuleInstallCx, NativeId, NativeSlot,
    NativeSnapshot, NativeSnapshotRequest, NativeViewEmitter, NativeViewFactory, NativeViewInstance, OctaraModule, TypedNativeProps,
};
use octara::{El, IntoProp, Prop, typed_native_view};
use std::any::Any;

#[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
mod apple;
#[cfg(all(feature = "platform", target_arch = "wasm32", target_os = "unknown"))]
mod browser;
mod headless;
mod protocol;

pub use protocol::{
    DialogRequest, DialogResponse, FilePickerRequest, NavigationDecision, NavigationRequest, NavigationState, RpcMethod, RpcReply,
    SchemeRequest, SchemeResponse, WebViewCommand, WebViewEvent, WebViewProps, WebViewSource,
};

pub const ID: ModuleId = ModuleId("octara.webview");
pub const KIND: &str = ID.0;

/// Builds a web view in the platform's default stacking slot. Override with `El::native_slot`.
pub fn view(props: impl IntoProp<WebViewProps>) -> El {
    let slot = if cfg!(target_os = "ios") { NativeSlot::Overlay } else { NativeSlot::Underlay };
    typed_native_view(KIND, props.into_prop().map(|props| TypedNativeProps::new(props, Some(serde_json::json!({ "kind": KIND })))))
        .native_slot(slot)
}

impl IntoProp<WebViewProps> for WebViewProps {
    fn into_prop(self) -> Prop<WebViewProps> {
        Prop::Static(self)
    }
}

pub fn send(w: &mut WriteCx<'_>, test_id: &str, command: WebViewCommand) -> bool {
    w.send_typed_native_command(test_id, command, None)
}

/// Sends typed input to a Promise handler registered by the page using `octara.handle`.
/// Decode the matching [WebViewEvent::Rpc] with [RpcReply::decode].
pub fn send_rpc<M: RpcMethod>(
    w: &mut WriteCx<'_>,
    test_id: &str,
    request_id: impl Into<String>,
    args: M::Input,
) -> Result<bool, ModuleError> {
    let call = protocol::RpcCall::new::<M>(request_id.into(), args)?;
    Ok(w.send_typed_native_command(test_id, call, None))
}

pub fn module() -> impl OctaraModule {
    WebModule { mock: false }
}

/// Installs a pure Rust view with deterministic source, load and history events.
/// Use `default-features = false` for a dependency without platform bindings.
pub fn mock() -> impl OctaraModule {
    WebModule { mock: true }
}

struct WebModule {
    mock: bool,
}

impl OctaraModule for WebModule {
    fn id(&self) -> ModuleId {
        ID
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        cx.native_views().register(Factory { mock: self.mock })
    }

    fn call(&mut self, _ticket: CallTicket, _request: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        let _ = cx.reply_erased(Err(ModuleError::Unavailable));
    }
}

struct Factory {
    mock: bool,
}

impl NativeViewFactory for Factory {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn airspace_policy(&self) -> AirspacePolicy {
        if cfg!(target_os = "ios") { AirspacePolicy::SnapshotOnOcclude } else { AirspacePolicy::UnderlayPreferred }
    }

    fn supports_slot(&self, surface: HostSurfaceKind, slot: NativeSlot) -> bool {
        match surface {
            HostSurfaceKind::Headless => self.mock,
            HostSurfaceKind::MacOs => !self.mock && cfg!(feature = "platform") && cfg!(target_os = "macos"),
            HostSurfaceKind::Ios => !self.mock && cfg!(feature = "platform") && cfg!(target_os = "ios") && slot == NativeSlot::Overlay,
            HostSurfaceKind::Web => !self.mock && cfg!(all(feature = "platform", target_arch = "wasm32", target_os = "unknown")),
            _ => false,
        }
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        _id: NativeId,
        slot: NativeSlot,
        props: Box<dyn Any>,
        emitter: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
        if !self.supports_slot(surface.kind(), slot) {
            return Err(ModuleError::Unavailable);
        }
        let props = props.downcast::<WebViewProps>().map_err(|_| ModuleError::InvalidArgs("expected WebViewProps".into()))?;
        match surface.kind() {
            HostSurfaceKind::Headless => Ok(Box::new(headless::Mock::new(*props, emitter))),
            #[cfg(all(feature = "platform", any(target_os = "macos", target_os = "ios")))]
            HostSurfaceKind::MacOs | HostSurfaceKind::Ios => apple::create(surface, slot, *props, emitter),
            #[cfg(all(feature = "platform", target_arch = "wasm32", target_os = "unknown"))]
            HostSurfaceKind::Web => browser::create(surface, *props, emitter),
            _ => Err(ModuleError::Unavailable),
        }
    }
}

fn emit(emitter: &NativeViewEmitter, event: WebViewEvent) {
    let _ = emitter.emit(event.name(), event, None);
}

fn unavailable_snapshot(emitter: &NativeViewEmitter, request: NativeSnapshotRequest, reason: String) {
    let picture = octara::Picture::new(1, 1, vec![0; 4]).expect("one transparent pixel");
    let _ = emitter.finish_snapshot(request, NativeSnapshot { picture, misses: vec![reason] });
}

fn unsupported_command(emitter: &NativeViewEmitter, command: WebViewCommand) {
    emit(
        emitter,
        match command {
            WebViewCommand::Evaluate { request_id, .. } => WebViewEvent::JavaScript { request_id, result: Err(ModuleError::Unavailable) },
            command => WebViewEvent::CommandFailed { command: command.name(), error: ModuleError::Unavailable },
        },
    );
}
