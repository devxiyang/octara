//! UI-thread requests and their scope-bound completions.

use std::any::{Any, TypeId};
use std::collections::{BTreeSet, VecDeque, btree_map::Entry};
use std::sync::{Arc, Weak};

use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::identity::WindowId;
use octara_model::input::HostReply;

use super::dispatch::Message;
use super::*;
use crate::cycle::{AppError, CycleReceipt};
use crate::task::{TaskCx, channel};
use crate::{AppRuntime, ReactiveWorld, ScopeId};

struct Request {
    ticket: CallTicket,
    pending: PendingCall,
    payload: Box<dyn Any>,
}

type Completion = Box<dyn FnOnce(&mut WriteCx<'_>, ErasedResult)>;
type EventCallback = Box<dyn Fn(&mut WriteCx<'_>, &dyn Any)>;

pub(super) struct PendingCall {
    call: CallMetadata,
    scope: ScopeId,
    window: Option<WindowId>,
    lifetime: Option<Weak<()>>,
    completion: Completion,
}

impl PendingCall {
    fn is_live(&self, world: &ReactiveWorld) -> bool {
        world.is_scope_alive(self.scope) && self.lifetime.as_ref().is_none_or(|token| token.strong_count() != 0)
    }
}

struct Subscription {
    scope: ScopeId,
    window: Option<WindowId>,
    event: TypeId,
    callback: EventCallback,
}

enum LocalMessage {
    Event { metadata: EventMetadata, payload: Box<dyn Any> },
    Lifecycle(HostLifecycleEvent),
}

#[derive(Default)]
pub(crate) struct ModuleWork {
    ticket_seq: u64,
    subscription_seq: u64,
    requests: VecDeque<Request>,
    local_messages: VecDeque<LocalMessage>,
    subscriptions: Vec<Subscription>,
    pub(crate) dispatcher: ModuleDispatcher,
}

impl ModuleWork {
    pub(crate) fn next_ticket(&mut self) -> CallTicket {
        self.ticket_seq = self.ticket_seq.checked_add(1).expect("module call tickets exhausted");
        CallTicket(self.ticket_seq)
    }
}

impl Drop for ModuleWork {
    fn drop(&mut self) {
        self.dispatcher.close();
    }
}

impl WriteCx<'_> {
    /// Queue a typed call owned by this scope. The callback runs on the UI thread.
    pub fn call_module<C: ModuleCall>(
        &mut self,
        request: C,
        complete: impl FnOnce(&mut WriteCx<'_>, Result<C::Output, ModuleError>) + 'static,
    ) -> CallTicket {
        self.queue_module_call(request, None, complete)
    }

    /// Queue a call whose completion does not update application state.
    pub fn notify_module<C: ModuleCall<Output = ()>>(&mut self, request: C) {
        self.call_module(request, |_, _| {});
    }

    fn queue_module_call<C: ModuleCall>(
        &mut self,
        request: C,
        lifetime: Option<Weak<()>>,
        complete: impl FnOnce(&mut WriteCx<'_>, Result<C::Output, ModuleError>) + 'static,
    ) -> CallTicket {
        let work = &mut self.world.module_work;
        let ticket = work.next_ticket();
        let completion = Box::new(move |write: &mut WriteCx<'_>, result: ErasedResult| {
            complete(write, result.and_then(|value| value.downcast::<C::Output>().map(|value| *value).map_err(|_| type_mismatch())));
        });
        work.requests.push_back(Request {
            ticket,
            pending: PendingCall { call: CallMetadata::of::<C>(), scope: self.scope, window: self.window, lifetime, completion },
            payload: Box::new(request),
        });
        ticket
    }
}

struct PendingFuture {
    token: Option<Arc<()>>,
    dispatcher: ModuleDispatcher,
}

impl Drop for PendingFuture {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            drop(token);
            self.dispatcher.wake();
        }
    }
}

impl TaskCx {
    /// Start a module call from a scope-owned task. Dropping the future cancels
    /// the ticket if the module has received the call and still owes a reply.
    pub async fn call_module<C: ModuleCall>(&self, request: C) -> Result<C::Output, ModuleError> {
        let (sender, mut receiver) = channel();
        let token = Arc::new(());
        let dispatcher = self
            .write(|write| {
                write.queue_module_call(request, Some(Arc::downgrade(&token)), move |_, result| {
                    let _ = sender.send(result);
                });
                write.world.module_work.dispatcher.clone()
            })
            .ok_or(ModuleError::Cancelled)?;
        let mut lifetime = PendingFuture { token: Some(token), dispatcher };
        let result = receiver.recv().await.unwrap_or(Err(ModuleError::Cancelled));
        drop(lifetime.token.take());
        result
    }
}

impl BuildCx<'_> {
    /// Subscribe in the current scope. Retiring the scope removes the subscription.
    pub fn on_module_event<E: ModuleEvent>(&mut self, callback: impl Fn(&mut WriteCx<'_>, &E) + 'static) -> SubscriptionId {
        self.subscribe_module(callback)
    }

    pub fn on_lifecycle(&mut self, callback: impl Fn(&mut WriteCx<'_>, &HostLifecycleEvent) + 'static) -> SubscriptionId {
        self.subscribe_module(callback)
    }

    fn subscribe_module<E: 'static>(&mut self, callback: impl Fn(&mut WriteCx<'_>, &E) + 'static) -> SubscriptionId {
        let work = &mut self.world.module_work;
        work.subscription_seq = work.subscription_seq.checked_add(1).expect("module subscription IDs exhausted");
        work.subscriptions.push(Subscription {
            scope: self.scope,
            window: self.window,
            event: TypeId::of::<E>(),
            callback: Box::new(move |write, payload| {
                if let Some(event) = payload.downcast_ref::<E>() {
                    callback(write, event);
                }
            }),
        });
        SubscriptionId(work.subscription_seq)
    }
}

impl AppRuntime {
    pub fn set_module_host_kind(&mut self, kind: HostSurfaceKind) {
        self.modules.host = Some(kind);
    }

    pub fn module_dispatcher(&self) -> ModuleDispatcher {
        self.world.module_work.dispatcher.clone()
    }

    pub fn native_views_mut(&mut self) -> &mut NativeViewRegistry {
        &mut self.modules.views
    }

    /// Receive optional inspection summaries after runtime privacy filtering.
    pub fn set_module_observer(&mut self, observer: Option<Observe>) -> Option<Observe> {
        std::mem::replace(&mut self.modules.observer, observer)
    }

    pub(crate) fn observes_modules(&self) -> bool {
        self.modules.observer.is_some()
    }

    pub(crate) fn observe_module(&mut self, observation: ModuleObservation) {
        if let Some(observer) = &mut self.modules.observer {
            observer(observation);
        }
    }

    pub fn install_modules(&mut self, modules: Vec<Box<dyn OctaraModule>>) -> Result<(), AppError> {
        self.require_active()?;
        let mut seen = BTreeSet::new();
        let modules = modules.into_iter().map(|module| (module.id(), module)).collect::<Vec<_>>();
        for (id, _) in &modules {
            if self.modules.modules.contains_key(id) || !seen.insert(*id) {
                self.diagnostics.push(Diagnostic::new(&codes::DUPLICATE_MODULE, format!("Duplicate module {}", id.0)));
                return Err(AppError::DuplicateModule(*id));
            }
        }

        self.guarded(move |app| {
            let host = app.modules.host_kind();
            let dispatcher = app.module_dispatcher();
            let scope = app.world.root_scope();
            for (id, mut module) in modules {
                let build = app
                    .world
                    .build(scope, None)
                    .with_tasks(app.tasks.spawner())
                    .with_system_preferences(app.system_preferences.read_only());
                let mut cx = ModuleInstallCx { build, native_views: &mut app.modules.views, host, dispatcher: dispatcher.clone() };
                if let Err(error) = module.install(&mut cx) {
                    module.shutdown();
                    app.diagnostics.push(Diagnostic::new(&codes::MODULE_INSTALL_FAILED, format!("Could not install module {}", id.0)));
                    app.shutdown();
                    return Err(AppError::ModuleInstall { id, error });
                }
                app.modules.modules.insert(id, module);
            }
            Ok(())
        })
    }

    /// Replace a call handler by Rust type for deterministic headless runs.
    pub fn stub_module_call<C: ModuleCall>(&mut self, mut handle: impl FnMut(C) -> Result<C::Output, ModuleError> + 'static) {
        self.modules.stubs.insert(
            TypeId::of::<C>(),
            Box::new(move |call| {
                let request = call.downcast::<C>().map_err(|_| type_mismatch())?;
                handle(*request).map(|value| Box::new(value) as Box<dyn Any>)
            }),
        );
    }

    pub fn dispatch_lifecycle(&mut self, event: HostLifecycleEvent) -> Result<CycleReceipt, AppError> {
        self.require_active()?;
        // A host on the UI thread can drive ingress directly, even when the
        // bounded inbox used by background callbacks is full.
        self.world.module_work.local_messages.push_back(LocalMessage::Lifecycle(event));
        self.run_cycle(None).map(|receipt| CycleReceipt { delivered: true, ..receipt })
    }

    /// Inject a local typed event without imposing a cross-thread Send requirement.
    pub fn emit_module_event<E: ModuleEvent>(&mut self, event: E) -> Result<CycleReceipt, AppError> {
        self.require_active()?;
        self.world
            .module_work
            .local_messages
            .push_back(LocalMessage::Event { metadata: EventMetadata::of::<E>(), payload: Box::new(event) });
        self.run_cycle(None).map(|receipt| CycleReceipt { delivered: true, ..receipt })
    }

    pub(crate) fn has_module_requests(&self) -> bool {
        !self.world.module_work.requests.is_empty()
    }

    pub(crate) fn dispatch_module_requests(&mut self) {
        let requests = std::mem::take(&mut self.world.module_work.requests);
        for Request { ticket, pending, payload } in requests {
            if !pending.is_live(&self.world) {
                continue;
            }
            let call = pending.call;
            let window = pending.window;
            if self.observes_modules() {
                self.observe_call(ModuleObservationKind::Call, call, ticket, || (call.args)(&*payload));
            }
            let dispatcher = self.module_dispatcher();
            let host = self.modules.host_kind();
            let result = if let Some(stub) = self.modules.stubs.get_mut(&call.id) {
                Some(stub(payload))
            } else if let Some(module) = self.modules.modules.get_mut(&call.module) {
                let mut result = None;
                let mut cx = ModuleCallCx { call, reply: &mut result, dispatcher, host, window };
                module.call(ticket, payload, &mut cx);
                result
            } else {
                Some(Err(ModuleError::Unavailable))
            };

            if let Some(result) = result {
                self.finish_module_call(ticket, pending, result);
            } else {
                self.modules.pending.insert(ticket, pending);
            }
        }
    }

    pub(crate) fn drain_module_messages(&mut self) {
        self.cancel_retired_module_work();
        let incoming = self.module_dispatcher().drain();
        for message in incoming {
            match message {
                Message::Reply { ticket, call, result } => {
                    let Entry::Occupied(entry) = self.modules.pending.entry(ticket) else { continue };
                    let expected = entry.get().call;
                    if call.is_some_and(|call| expected.id != call)
                        || result.as_ref().is_ok_and(|value| (**value).type_id() != expected.output)
                    {
                        continue;
                    }
                    let pending = entry.remove();
                    if !pending.is_live(&self.world) {
                        self.cancel_module(ticket, pending.call.module);
                        continue;
                    }
                    self.finish_module_call(ticket, pending, result.map(|value| value as Box<dyn Any>));
                }
                Message::Event { metadata, payload } => self.deliver_module_event(metadata, &*payload),
                Message::Lifecycle(event) => self.deliver_lifecycle(&event),
                Message::NativeEvent { window, id, event, mount } => {
                    if mount.is_none_or(|mount| mount.strong_count() != 0) {
                        self.route_typed_native_event(window, id, event);
                    }
                }
                Message::NativeSnapshot { request, snapshot } => {
                    if let Some(window) = self.windows.values_mut().find(|window| window.tree.id() == request.tree) {
                        window.engine.native_reply(&HostReply::NativeSnapshot { request, snapshot: Ok(snapshot) });
                        window.force_project = true;
                    }
                }
            }
        }
        for message in std::mem::take(&mut self.world.module_work.local_messages) {
            match message {
                LocalMessage::Event { metadata, payload } => self.deliver_module_event(metadata, &*payload),
                LocalMessage::Lifecycle(event) => self.deliver_lifecycle(&event),
            }
        }
    }

    pub(crate) fn cancel_retired_module_work(&mut self) {
        let world = &self.world;
        for (ticket, pending) in self.modules.pending.extract_if(.., |_, pending| !pending.is_live(world)) {
            if let Some(module) = self.modules.modules.get_mut(&pending.call.module) {
                module.cancel(ticket);
            }
        }
        let mut subscriptions = std::mem::take(&mut self.world.module_work.subscriptions);
        subscriptions.retain(|subscription| self.world.is_scope_alive(subscription.scope));
        self.world.module_work.subscriptions = subscriptions;
    }

    pub(crate) fn shutdown_modules(&mut self) {
        self.module_dispatcher().close();
        self.cancel_retired_module_work();
        self.world.module_work.requests.clear();
        self.world.module_work.local_messages.clear();
        for mut module in std::mem::take(&mut self.modules.modules).into_values() {
            module.shutdown();
        }
        self.modules.stubs.clear();
        self.modules.views = NativeViewRegistry::default();
    }

    pub(crate) fn discard_module_requests(&mut self) {
        self.world.module_work.requests.clear();
    }

    fn cancel_module(&mut self, ticket: CallTicket, module: ModuleId) {
        if let Some(module) = self.modules.modules.get_mut(&module) {
            module.cancel(ticket);
        }
    }

    fn finish_module_call(&mut self, ticket: CallTicket, pending: PendingCall, mut result: ErasedResult) {
        let call = pending.call;
        if result.as_ref().is_ok_and(|value| (**value).type_id() != call.output) {
            result = Err(type_mismatch());
        }
        if self.observes_modules() {
            self.observe_call(ModuleObservationKind::Reply, call, ticket, || match &result {
                Ok(value) => (call.inspect_output)(&**value),
                Err(error) => serde_json::to_value(error).ok(),
            });
        }
        (pending.completion)(&mut self.module_write(pending.scope, pending.window), result);
    }

    fn observe_call(
        &mut self,
        kind: ModuleObservationKind,
        call: CallMetadata,
        ticket: CallTicket,
        inspect: impl FnOnce() -> Option<Value>,
    ) {
        self.observe_module(ModuleObservation {
            at: self.now(),
            kind,
            module: call.module.0.into(),
            name: call.name.into(),
            ticket: Some(ticket),
            payload: if call.sensitive { Some(Value::from("<redacted>")) } else { inspect() },
        });
    }

    fn deliver_module_event(&mut self, metadata: EventMetadata, payload: &dyn Any) {
        if self.observes_modules() {
            self.observe_module(ModuleObservation {
                at: self.now(),
                kind: ModuleObservationKind::Event,
                module: metadata.module.0.into(),
                name: metadata.name.into(),
                ticket: None,
                payload: if metadata.sensitive { Some(Value::from("<redacted>")) } else { (metadata.inspect)(payload) },
            });
        }
        self.broadcast(metadata.id, payload);
    }

    fn deliver_lifecycle(&mut self, event: &HostLifecycleEvent) {
        let window = match event {
            HostLifecycleEvent::WindowAttached { window } | HostLifecycleEvent::WindowDetached { window } => {
                self.windows.contains_key(window).then_some(*window)
            }
            _ => None,
        };
        let scope = window.map_or_else(|| self.world.root_scope(), |window| self.windows[&window].scope);
        let dispatcher = self.module_dispatcher();
        let host = self.modules.host_kind();
        let write = self.world.write(scope, window).with_tasks(self.tasks.spawner()).with_commands(&mut self.commands);
        let mut cx = ModuleLifecycleCx { write, dispatcher, host };
        for module in self.modules.modules.values_mut() {
            module.on_lifecycle(event, &mut cx);
        }
        self.broadcast(TypeId::of::<HostLifecycleEvent>(), event);
    }

    fn broadcast(&mut self, event: TypeId, payload: &dyn Any) {
        let subscriptions = std::mem::take(&mut self.world.module_work.subscriptions);
        for subscription in &subscriptions {
            if subscription.event == event && self.world.is_scope_alive(subscription.scope) {
                (subscription.callback)(&mut self.module_write(subscription.scope, subscription.window), payload);
            }
        }
        self.world.module_work.subscriptions = subscriptions;
    }

    fn module_write(&mut self, scope: ScopeId, window: Option<WindowId>) -> WriteCx<'_> {
        let mut cx = self.world.write(scope, window).with_tasks(self.tasks.spawner()).with_commands(&mut self.commands);
        cx.tree = window.and_then(|id| self.windows.get(&id)).map(|window| &window.tree);
        cx
    }
}
