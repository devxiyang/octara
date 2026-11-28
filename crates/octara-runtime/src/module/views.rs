//! Host surfaces and mounting for registered native views.

use super::ModuleDispatcher;
use octara_model::geometry::{Size, WindowViewport};
use octara_model::identity::{NativeId, WindowId};
use octara_model::module::{AirspacePolicy, CallTicket, HostSurfaceKind, ModuleError};
use octara_model::native::{
    NativeChange, NativeMeasureRequest, NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest, TypedNativeCommand,
    TypedNativeEvent,
};
use serde_json::Value;
use slotmap::Key as _;
use std::any::Any;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Weak};

/// Raw host handles. A host lends these handles for the duration of the SPI call.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum RawNativeSurface {
    AppKit { ns_window: NonNull<c_void>, ns_view: NonNull<c_void> },
    UiKit { ui_view: NonNull<c_void>, ui_view_controller: NonNull<c_void> },
    Win32 { hwnd: NonNull<c_void>, hinstance: Option<NonNull<c_void>> },
    Wayland { display: NonNull<c_void>, surface: NonNull<c_void> },
    Xlib { display: NonNull<c_void>, window: u64 },
}

/// Target-neutral access to a window's host surface.
#[derive(Clone, Copy)]
pub struct HostSurface<'a> {
    kind: HostSurfaceKind,
    window: Option<WindowId>,
    native: Option<RawNativeSurface>,
    opaque: Option<&'a dyn Any>,
}

impl<'a> HostSurface<'a> {
    pub fn new(kind: HostSurfaceKind, window: Option<WindowId>) -> Self {
        Self { kind, window, native: None, opaque: None }
    }

    /// Attach handles supplied by the platform host.
    ///
    /// # Safety
    /// The handles must identify live platform objects of the indicated types
    /// on their owning UI thread and remain valid for this surface's lifetime.
    pub unsafe fn with_raw_native(mut self, native: RawNativeSurface) -> Self {
        self.native = Some(native);
        self
    }

    pub fn with_opaque(mut self, opaque: &'a dyn Any) -> Self {
        self.opaque = Some(opaque);
        self
    }

    pub fn kind(&self) -> HostSurfaceKind {
        self.kind
    }

    pub fn window(&self) -> Option<WindowId> {
        self.window
    }

    pub fn raw_native(&self) -> Option<RawNativeSurface> {
        self.native
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&'a T> {
        self.opaque?.downcast_ref()
    }
}

/// Sends callbacks from a native view to its owning window and node.
#[derive(Clone)]
pub struct NativeViewEmitter {
    window: WindowId,
    id: NativeId,
    dispatcher: ModuleDispatcher,
    mount: Weak<()>,
}

impl NativeViewEmitter {
    pub fn emit<E: Send + 'static>(&self, name: &'static str, payload: E, inspect: Option<Value>) -> Result<(), ModuleError> {
        self.dispatcher.emit_mounted_event(self.window, self.id, TypedNativeEvent::new(name, payload, inspect), self.mount.clone())
    }

    pub fn finish_snapshot(&self, request: NativeSnapshotRequest, snapshot: NativeSnapshot) -> Result<(), ModuleError> {
        if self.mount.strong_count() == 0 {
            return Err(ModuleError::Cancelled);
        }
        if request.id != self.id || request.tree.window != self.window.data().as_ffi() {
            return Err(ModuleError::InvalidArgs("snapshot request belongs to another native view".into()));
        }
        self.dispatcher.resolve_native_snapshot(request, snapshot)
    }
}

pub trait NativeViewFactory: 'static {
    fn kind(&self) -> &'static str;

    fn airspace_policy(&self) -> AirspacePolicy {
        AirspacePolicy::UnderlayPreferred
    }

    fn supports_slot(&self, _surface: HostSurfaceKind, _slot: NativeSlot) -> bool {
        true
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        id: NativeId,
        slot: NativeSlot,
        props: Box<dyn Any>,
        emitter: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError>;
}

pub trait NativeViewInstance: 'static {
    fn update(&mut self, props: Box<dyn Any>);
    fn place(&mut self, placement: &NativePlacement);
    fn call(&mut self, ticket: CallTicket, command: Box<dyn Any>, reply: ModuleDispatcher);

    fn measure(&mut self, _viewport: WindowViewport) -> Option<Size> {
        None
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, reply: ModuleDispatcher);

    fn busy_reason(&self) -> Option<String> {
        None
    }

    fn set_occluded_by_snapshot(&mut self, _hidden: bool) {}
}

/// Factories installed by the application's modules, owned by its runtime.
#[derive(Default)]
pub struct NativeViewRegistry {
    factories: BTreeMap<&'static str, Box<dyn NativeViewFactory>>,
}

impl NativeViewRegistry {
    pub fn register(&mut self, factory: impl NativeViewFactory) -> Result<(), ModuleError> {
        let kind = factory.kind();
        match self.factories.entry(kind) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Box::new(factory));
                Ok(())
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                Err(ModuleError::InvalidArgs(format!("native view factory {kind:?} is already registered")))
            }
        }
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.factories.contains_key(kind)
    }
}

struct MountedView {
    slot: NativeSlot,
    instance: Box<dyn NativeViewInstance>,
    _mount: Arc<()>,
    placed: Option<bool>,
    snapshot_target: Option<bool>,
    snapshot_hidden: bool,
}

/// Host-owned native views for a single window. Dropping a mount releases its instance.
#[derive(Default)]
pub struct NativeViewMounts {
    views: BTreeMap<NativeId, MountedView>,
}

impl NativeViewMounts {
    /// Apply committed changes. Returns whether this helper handled the change exclusively.
    /// Placement updates are shared with the host's built-in native views.
    pub fn apply_change(
        &mut self,
        change: &NativeChange,
        surface: HostSurface<'_>,
        registry: &mut NativeViewRegistry,
        dispatcher: &ModuleDispatcher,
    ) -> Result<bool, ModuleError> {
        match change {
            NativeChange::CreateTyped { id, kind, slot, props } => {
                let window = surface.window().ok_or(ModuleError::Unavailable)?;
                let factory = registry.factories.get_mut(kind.as_str()).ok_or(ModuleError::Unavailable)?;
                if !factory.supports_slot(surface.kind(), *slot) {
                    return Err(ModuleError::Unavailable);
                }
                if self.contains(*id) {
                    return Ok(true);
                }
                let payload = props.boxed_payload().ok_or_else(|| ModuleError::InvalidArgs("missing typed native props".into()))?;
                let mount = Arc::new(());
                let emitter = NativeViewEmitter { window, id: *id, dispatcher: dispatcher.clone(), mount: Arc::downgrade(&mount) };
                let snapshot_target =
                    (*slot == NativeSlot::Overlay && factory.airspace_policy() == AirspacePolicy::SnapshotOnOcclude).then_some(false);
                let instance = factory.create(surface, *id, *slot, payload, emitter)?;
                self.views.insert(
                    *id,
                    MountedView { slot: *slot, instance, _mount: mount, placed: None, snapshot_target, snapshot_hidden: false },
                );
                Ok(true)
            }
            NativeChange::UpdateTyped { id, props } => {
                if let Some(view) = self.views.get_mut(id) {
                    let payload = props.boxed_payload().ok_or_else(|| ModuleError::InvalidArgs("missing typed native props".into()))?;
                    view.instance.update(payload);
                }
                Ok(true)
            }
            NativeChange::Place { placements } => {
                for placement in placements {
                    self.place(placement);
                }
                Ok(false)
            }
            NativeChange::Destroy { id } => Ok(self.retire(*id).is_some()),
            NativeChange::Create { .. } | NativeChange::Update { .. } => Ok(false),
        }
    }

    pub fn contains(&self, id: NativeId) -> bool {
        self.views.contains_key(&id)
    }

    /// End command, measurement and event delivery now. The host can keep the
    /// instance mounted until presentation and knows whether it received a placement.
    pub fn retire(&mut self, id: NativeId) -> Option<(Box<dyn NativeViewInstance>, bool)> {
        self.views.remove(&id).map(|view| (view.instance, view.placed.is_some()))
    }

    /// Apply a placement to its mounted view during host presentation.
    pub fn place(&mut self, placement: &NativePlacement) {
        if let Some(view) = self.views.get_mut(&placement.id) {
            view.instance.place(placement);
            view.placed = Some(placement.visible);
        }
    }

    pub fn has_underlay(&self) -> bool {
        self.views.values().any(|view| view.slot == NativeSlot::Underlay)
    }

    pub fn measure(&mut self, request: &NativeMeasureRequest) -> Option<Size> {
        self.views.get_mut(&request.ticket.id())?.instance.measure(request.viewport)
    }

    pub fn command(&mut self, command: TypedNativeCommand, dispatcher: &ModuleDispatcher) -> bool {
        let Some(view) = self.views.get_mut(&command.id) else { return false };
        view.instance.call(command.ticket, command.payload, dispatcher.clone());
        true
    }

    pub fn snapshot(&mut self, request: NativeSnapshotRequest, dispatcher: &ModuleDispatcher) -> bool {
        let Some(view) = self.views.get_mut(&request.id) else { return false };
        view.instance.snapshot(request, dispatcher.clone());
        true
    }

    /// Airspace captures require an opted-in overlay with a visible committed placement.
    pub fn can_snapshot_on_occlude(&self, id: NativeId) -> bool {
        self.views.get(&id).is_some_and(|view| view.snapshot_target == Some(false) && !view.snapshot_hidden && view.placed == Some(true))
    }

    /// Stage visibility deltas against the last synchronized frame. The caller
    /// applies them to the corresponding instances when the frame is presented.
    pub fn snapshot_visibility_changes<'a>(
        &'a mut self,
        contains_snapshot: impl Fn(NativeId) -> bool + 'a,
    ) -> impl Iterator<Item = (NativeId, bool)> + 'a {
        self.views.iter_mut().filter_map(move |(&id, view)| {
            let target = view.snapshot_target.as_mut()?;
            let hidden = contains_snapshot(id);
            (std::mem::replace(target, hidden) != hidden).then_some((id, hidden))
        })
    }

    pub fn set_occluded_by_snapshot(&mut self, id: NativeId, hidden: bool) {
        if let Some(view) = self.views.get_mut(&id)
            && view.snapshot_target.is_some()
            && view.snapshot_hidden != hidden
        {
            view.snapshot_hidden = hidden;
            view.instance.set_occluded_by_snapshot(hidden);
        }
    }

    pub fn busy_reason(&self) -> Option<String> {
        self.views.values().find_map(|view| view.instance.busy_reason())
    }

    pub fn clear(&mut self) {
        self.views.clear();
    }
}
