//! Explicit application modules, in-process calls and platform view factories.

mod dispatch;
mod views;
mod work;

pub use dispatch::ModuleDispatcher;
pub use octara_model::module::{
    AirspacePolicy, CallTicket, HostLifecycleEvent, HostSurfaceKind, ModuleError, ModuleId, PermissionStatus, SubscriptionId,
};
pub use views::*;
pub(crate) use work::ModuleWork;

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use octara_model::identity::WindowId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BuildCx, WriteCx};

/// A typed call implemented by a module. Delivery remains in-process. Inspection
/// summaries are optional and are evaluated only when tracing is enabled.
pub trait ModuleCall: 'static {
    const MODULE: ModuleId;
    const METHOD: &'static str;
    const SENSITIVE: bool = false;
    type Output: 'static;

    fn inspect_args(&self) -> Option<Value> {
        None
    }

    fn inspect_output(_value: &Self::Output) -> Option<Value> {
        None
    }
}

/// A typed event broadcast to subscriptions in live scopes.
pub trait ModuleEvent: Clone + 'static {
    const MODULE: ModuleId;
    const EVENT: &'static str;
    const SENSITIVE: bool = false;

    fn inspect_payload(&self) -> Option<Value> {
        None
    }
}

/// A module runs on the application thread. Only its dispatcher crosses threads.
pub trait OctaraModule: 'static {
    fn id(&self) -> ModuleId;

    fn install(&mut self, _cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        Ok(())
    }

    fn call(&mut self, ticket: CallTicket, request: Box<dyn Any>, cx: &mut ModuleCallCx<'_>);

    fn cancel(&mut self, _ticket: CallTicket) {}

    fn on_lifecycle(&mut self, _event: &HostLifecycleEvent, _cx: &mut ModuleLifecycleCx<'_>) {}

    fn shutdown(&mut self) {}
}

type ErasedResult = Result<Box<dyn Any>, ModuleError>;
type Stub = Box<dyn FnMut(Box<dyn Any>) -> ErasedResult>;
type Observe = Box<dyn FnMut(ModuleObservation)>;

/// Installed modules and factories, owned by one application's UI thread.
#[derive(Default)]
pub struct ModuleRegistry {
    modules: BTreeMap<ModuleId, Box<dyn OctaraModule>>,
    stubs: HashMap<TypeId, Stub>,
    views: NativeViewRegistry,
    observer: Option<Observe>,
    host: Option<HostSurfaceKind>,
    native_surfaces: BTreeMap<WindowId, RawNativeSurface>,
    pending: BTreeMap<CallTicket, work::PendingCall>,
}

impl ModuleRegistry {
    fn host_kind(&self) -> HostSurfaceKind {
        self.host.unwrap_or(HostSurfaceKind::Headless)
    }
}

pub struct ModuleInstallCx<'a> {
    build: BuildCx<'a>,
    native_views: &'a mut NativeViewRegistry,
    host: HostSurfaceKind,
    dispatcher: ModuleDispatcher,
}

impl<'a> ModuleInstallCx<'a> {
    pub fn build(&mut self) -> &mut BuildCx<'a> {
        &mut self.build
    }

    pub fn native_views(&mut self) -> &mut NativeViewRegistry {
        self.native_views
    }

    pub fn host_kind(&self) -> HostSurfaceKind {
        self.host
    }

    pub fn dispatcher(&self) -> ModuleDispatcher {
        self.dispatcher.clone()
    }
}

pub struct ModuleCallCx<'a> {
    call: CallMetadata,
    reply: &'a mut Option<ErasedResult>,
    dispatcher: ModuleDispatcher,
    host: HostSurfaceKind,
    window: Option<WindowId>,
    native_surface: Option<RawNativeSurface>,
}

impl ModuleCallCx<'_> {
    pub fn downcast<C: ModuleCall>(&self, request: Box<dyn Any>) -> Result<C, ModuleError> {
        request.downcast::<C>().map(|request| *request).map_err(|_| type_mismatch())
    }

    /// Reply synchronously on the UI thread, including with non-Send output.
    pub fn reply<C: ModuleCall>(&mut self, result: Result<C::Output, ModuleError>) -> Result<(), ModuleError> {
        if self.call.id != TypeId::of::<C>() {
            return Err(type_mismatch());
        }
        self.reply_erased(result.map(|value| Box::new(value) as Box<dyn Any>))
    }

    /// Reply with an error for an unrecognized call or return a dynamically typed output.
    pub fn reply_erased(&mut self, result: Result<Box<dyn Any>, ModuleError>) -> Result<(), ModuleError> {
        if self.reply.is_some() {
            return Err(ModuleError::Busy);
        }
        *self.reply = Some(result);
        Ok(())
    }

    pub fn dispatcher(&self) -> ModuleDispatcher {
        self.dispatcher.clone()
    }

    pub fn host_kind(&self) -> HostSurfaceKind {
        self.host
    }

    pub fn window(&self) -> Option<WindowId> {
        self.window
    }

    /// The host surface associated with the scope that initiated the request.
    pub fn surface(&self) -> Option<HostSurface<'_>> {
        let surface = HostSurface::new(self.host, Some(self.window?));
        // SAFETY: The host registered the handles for the live window. The
        // returned surface borrows the call context on the UI thread.
        Some(self.native_surface.map_or(surface, |raw| unsafe { surface.with_raw_native(raw) }))
    }

    pub fn method(&self) -> &'static str {
        self.call.name
    }
}

pub struct ModuleLifecycleCx<'a> {
    write: WriteCx<'a>,
    dispatcher: ModuleDispatcher,
    host: HostSurfaceKind,
}

impl<'a> ModuleLifecycleCx<'a> {
    pub fn write(&mut self) -> &mut WriteCx<'a> {
        &mut self.write
    }

    pub fn dispatcher(&self) -> ModuleDispatcher {
        self.dispatcher.clone()
    }

    pub fn host_kind(&self) -> HostSurfaceKind {
        self.host
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleObservationKind {
    Call,
    Reply,
    Event,
    NativeCommand,
    NativeEvent,
}

/// An optional, privacy-filtered summary. This is observation data, not a wire
/// representation of an in-process call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleObservation {
    pub at: Duration,
    pub kind: ModuleObservationKind,
    pub module: String,
    pub name: String,
    pub ticket: Option<CallTicket>,
    pub payload: Option<Value>,
}

type Inspect = fn(&dyn Any) -> Option<Value>;

#[derive(Clone, Copy)]
struct CallMetadata {
    id: TypeId,
    output: TypeId,
    module: ModuleId,
    name: &'static str,
    sensitive: bool,
    args: Inspect,
    inspect_output: Inspect,
}

impl CallMetadata {
    fn of<C: ModuleCall>() -> Self {
        Self {
            id: TypeId::of::<C>(),
            output: TypeId::of::<C::Output>(),
            module: C::MODULE,
            name: C::METHOD,
            sensitive: C::SENSITIVE,
            args: |value| value.downcast_ref::<C>().and_then(C::inspect_args),
            inspect_output: |value| value.downcast_ref::<C::Output>().and_then(C::inspect_output),
        }
    }
}

#[derive(Clone, Copy)]
struct EventMetadata {
    id: TypeId,
    module: ModuleId,
    name: &'static str,
    sensitive: bool,
    inspect: Inspect,
}

impl EventMetadata {
    fn of<E: ModuleEvent>() -> Self {
        Self {
            id: TypeId::of::<E>(),
            module: E::MODULE,
            name: E::EVENT,
            sensitive: E::SENSITIVE,
            inspect: |value| value.downcast_ref::<E>().and_then(E::inspect_payload),
        }
    }
}

fn type_mismatch() -> ModuleError {
    ModuleError::InvalidArgs("module call type does not match its ticket".into())
}
