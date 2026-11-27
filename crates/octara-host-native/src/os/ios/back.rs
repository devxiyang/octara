//! The UIKit navigation controller settles its own interactive pop. Only the settled
//! result can retire the Octara page that was visible when the transition started.

use super::scene;
use crate::ShellEvent;
use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::NSArray;
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIColor, UIGestureRecognizer, UIGestureRecognizerDelegate, UINavigationController,
    UINavigationControllerDelegate, UISceneActivationState, UIView, UIViewController, UIViewControllerTransitionCoordinator,
    UIViewControllerTransitionCoordinatorContext, UIWindow,
};
use octara::runtime::Input;
use octara_model::identity::TargetGuard;
use octara_model::input::{BackPhase, InteractionEvent};
use std::cell::{Cell, RefCell};
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

#[derive(Clone, Copy)]
struct Pop {
    id: u64,
    owner: Option<TargetGuard>,
    coordinated: bool,
    cancelled: Option<bool>,
}

struct PopEvents {
    proxy: EventLoopProxy<ShellEvent>,
    pending_events: RefCell<Vec<(u64, TargetGuard, BackPhase)>>,
    drawn_owner: Cell<Option<TargetGuard>>,
    pop: Cell<Option<Pop>>,
}

impl PopEvents {
    fn send(&self, pop: Pop, phase: BackPhase) {
        if let Some(owner) = pop.owner {
            self.pending_events.borrow_mut().push((pop.id, owner, phase));
        }
        let _ = self.proxy.send_event(ShellEvent::Wake);
    }

    fn pending(&self, id: u64) -> Option<Pop> {
        self.pop.get().filter(|pop| pop.id == id && pop.cancelled.is_none())
    }

    fn finish(&self, id: u64, cancelled: bool) {
        let Some(mut pop) = self.pending(id) else { return };
        pop.cancelled = Some(cancelled);
        self.pop.set(Some(pop));
        self.send(pop, if cancelled { BackPhase::Cancelled } else { BackPhase::Completed });
    }
}

struct ObserverState {
    window: Weak<UIWindow>,
    navigation: Weak<UINavigationController>,
    content: Weak<UIViewController>,
    preview: Retained<UIViewController>,
    events: Rc<PopEvents>,
}

type TransitionContext = ProtocolObject<dyn UIViewControllerTransitionCoordinatorContext>;

impl ObserverState {
    fn window_accepts_gestures(&self) -> bool {
        self.window.load().is_some_and(|window| {
            window.isKeyWindow()
                && window.windowScene().is_some_and(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
        })
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraBackNavigationObserver"]
    #[ivars = ObserverState]
    struct NavigationObserver;

    unsafe impl NSObjectProtocol for NavigationObserver {}

    unsafe impl UIGestureRecognizerDelegate for NavigationObserver {
        #[unsafe(method(gestureRecognizerShouldBegin:))]
        fn should_begin(&self, _: &UIGestureRecognizer) -> bool {
            let state = self.ivars();
            state.events.drawn_owner.get().is_some()
                && state.events.pop.get().is_none()
                && state.window_accepts_gestures()
                && state.navigation.load().is_some_and(|navigation| {
                    navigation.transitionCoordinator().is_none()
                        && navigation.topViewController().zip(state.content.load()).is_some_and(|(top, content)| ptr::eq(&*top, &*content))
                })
        }
    }

    unsafe impl UINavigationControllerDelegate for NavigationObserver {
        #[unsafe(method(navigationController:willShowViewController:animated:))]
        fn will_show(&self, navigation: &UINavigationController, page: &UIViewController, _: bool) {
            let state = self.ivars();
            if !ptr::eq(&*state.preview, page) || state.events.pop.get().is_some() {
                return;
            }
            let coordinator = navigation.transitionCoordinator().or_else(|| page.transitionCoordinator());
            let interactive = coordinator.as_ref().is_some_and(|coordinator| coordinator.initiallyInteractive());
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let owner = state.events.drawn_owner.get().filter(|_| state.window_accepts_gestures());
            let pop = Pop { id, owner, coordinated: coordinator.is_some(), cancelled: None };
            state.events.pop.set(Some(pop));
            state.events.send(pop, BackPhase::Started);
            let Some(coordinator) = coordinator else { return };
            if interactive {
                let events = state.events.clone();
                let progress = RcBlock::new(move |context: NonNull<TransitionContext>| {
                    // SAFETY: UIKit supplies a live context for the duration of this callback.
                    let context = unsafe { context.as_ref() };
                    if context.isInteractive() {
                        return;
                    }
                    let Some(pop) = events.pending(id) else { return };
                    let progress = context.percentComplete() as f32;
                    if progress.is_finite() {
                        events.send(pop, BackPhase::Progress(progress.clamp(0.0, 1.0)));
                    }
                });
                coordinator.notifyWhenInteractionChangesUsingBlock(&progress);
            }
            let events = state.events.clone();
            let completion = RcBlock::new(move |context: NonNull<TransitionContext>| {
                // SAFETY: UIKit supplies a live context for the duration of this callback.
                events.finish(id, unsafe { context.as_ref() }.isCancelled());
            });
            if !coordinator.animateAlongsideTransition_completion(None, Some(&completion))
                && let Some(mut pop) = state.events.pending(id)
            {
                // didShow can settle the pop if the registration fails.
                pop.coordinated = false;
                state.events.pop.set(Some(pop));
            }
        }

        #[unsafe(method(navigationController:didShowViewController:animated:))]
        fn did_show(&self, _: &UINavigationController, page: &UIViewController, _: bool) {
            let state = self.ivars();
            if let Some(pop) = state.events.pop.get().filter(|pop| !pop.coordinated) {
                state.events.finish(pop.id, !ptr::eq(&*state.preview, page));
            }
        }
    }
);

struct Stack {
    navigation: Retained<UINavigationController>,
    observer: Retained<NavigationObserver>,
    recognizers: Vec<StockRecognizer>,
}

struct StockRecognizer {
    gesture: Retained<UIGestureRecognizer>,
    delegate: Option<Retained<ProtocolObject<dyn UIGestureRecognizerDelegate>>>,
}

impl Stack {
    fn preview(&self) -> &UIViewController {
        &self.observer.ivars().preview
    }

    fn detach(&self) {
        for recognizer in &self.recognizers {
            recognizer.gesture.setDelegate(recognizer.delegate.as_deref());
        }
        // SAFETY: Stop observing before changing the stack or releasing its delegate.
        unsafe { self.navigation.setDelegate(None) };
    }
}

/// The only live rendering view and its controller come from winit. A snapshot of
/// the previously drawn page is the other controller's view during the transition.
pub(super) struct BackNavigation {
    window: Retained<UIWindow>,
    content: Retained<UIViewController>,
    view: Retained<UIView>,
    events: Rc<PopEvents>,
    stack: Option<Stack>,
    previewable: bool,
    owner: Option<TargetGuard>,
    active: bool,
}

impl BackNavigation {
    pub(super) fn new(window: &Window, proxy: EventLoopProxy<ShellEvent>) -> Result<Self, String> {
        let view = scene::view(window)?;
        let content = scene::controller(window)?;
        let native = view.window().ok_or("the UIKit view has no window")?;
        let events = Rc::new(PopEvents { proxy, pending_events: RefCell::default(), drawn_owner: Cell::new(None), pop: Cell::new(None) });
        Ok(Self { window: native, content, view, events, stack: None, previewable: false, owner: None, active: true })
    }

    fn arm(&mut self) -> Result<(), String> {
        let mtm = MainThreadMarker::from(&*self.window);
        let snapshot = self.previewable.then(|| self.view.snapshotViewAfterScreenUpdates(false)).flatten().unwrap_or_else(|| {
            let view = UIView::initWithFrame(mtm.alloc(), self.view.bounds());
            let background = self.view.backgroundColor().unwrap_or_else(UIColor::whiteColor);
            view.setBackgroundColor(Some(&background));
            view
        });
        self.previewable = false;
        snapshot.setUserInteractionEnabled(false);
        snapshot.setAccessibilityElementsHidden(true, mtm);
        let preview = UIViewController::new(mtm);
        preview.setView(Some(&snapshot));
        let navigation = UINavigationController::initWithRootViewController(mtm.alloc(), &preview);
        navigation.setNavigationBarHidden(true);
        // Install the full stack before the navigation controller enters the window.
        // UIKit must release the previous root before the same controller can be parented.
        self.window.setRootViewController(None);
        navigation.pushViewController_animated(&self.content, false);
        navigation.loadViewIfNeeded();
        let state = ObserverState {
            window: Weak::new(&*self.window),
            navigation: Weak::new(&*navigation),
            content: Weak::new(&*self.content),
            preview,
            events: self.events.clone(),
        };
        // SAFETY: Initialize the observer's NSObject superclass on the UIKit thread.
        let observer: Retained<NavigationObserver> = unsafe { msg_send![super(mtm.alloc::<NavigationObserver>().set_ivars(state)), init] };
        // SAFETY: Keep the weak delegate alive while the navigation stack is installed.
        unsafe { navigation.setDelegate(Some(ProtocolObject::from_ref(&*observer))) };
        let mut recognizers = Vec::<StockRecognizer>::new();
        // This stack hides the UIKit bar. Admit its stock gestures only when the live
        // controller owns a drawn back handler. UIKit still runs and settles the pop.
        for gesture in [navigation.interactivePopGestureRecognizer(), content_pop(&navigation)].into_iter().flatten() {
            if recognizers.iter().any(|recognizer| ptr::eq(&*recognizer.gesture, &*gesture)) {
                continue;
            }
            let delegate = gesture.delegate();
            gesture.setDelegate(Some(ProtocolObject::from_ref(&*observer)));
            gesture.setEnabled(true);
            recognizers.push(StockRecognizer { gesture, delegate });
        }
        self.window.setRootViewController(Some(&navigation));
        let installed = recognizers.iter().any(|recognizer| recognizer.gesture.isEnabled() && recognizer.gesture.view().is_some());
        self.stack = Some(Stack { navigation, observer, recognizers });
        if !installed {
            self.restore();
            return Err("UIKit did not attach its interactive pop recognizer to the navigation stack".into());
        }
        Ok(())
    }

    pub(super) fn sync(&mut self, owner: Option<TargetGuard>) {
        if self.owner != owner {
            self.interrupt();
            self.owner = owner;
        }
        if owner.is_none() && self.events.pop.get().is_none() {
            self.restore();
        }
        if self.active && owner.is_some() && self.stack.is_none() {
            let _ = self.arm();
        }
    }

    pub(super) fn did_present(&mut self) {
        self.previewable = self.owner.is_none() && self.active;
        self.events.drawn_owner.set(self.owner.filter(|_| self.active && self.stack.is_some()));
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.interrupt();
        }
    }

    fn interrupt(&self) {
        self.events.drawn_owner.set(None);
        if let Some(mut pop) = self.events.pop.get() {
            // The native transition still owns the compositor until its completion callback.
            pop.owner = None;
            self.events.pop.set(Some(pop));
        }
        self.events.pending_events.borrow_mut().clear();
    }

    pub(super) fn drain(&mut self, out: &mut Vec<Input>) {
        for (id, owner, phase) in self.events.pending_events.take() {
            if self.active && self.owner == Some(owner) {
                out.push(Input::Interaction(InteractionEvent::BackGesture { id, phase }));
            }
        }
        if let Some(pop) = self.events.pop.get()
            && let Some(cancelled) = pop.cancelled
        {
            self.events.pop.set(None);
            if !cancelled || self.owner.is_none() {
                self.restore();
            }
        }
    }

    fn restore(&mut self) {
        let Some(stack) = self.stack.take() else { return };
        self.events.drawn_owner.set(None);
        stack.detach();
        stack.navigation.setViewControllers_animated(&NSArray::from_slice(&[stack.preview()]), false);
        self.window.setRootViewController(Some(&self.content));
    }
}

fn content_pop(navigation: &UINavigationController) -> Option<Retained<UIGestureRecognizer>> {
    if navigation.respondsToSelector(sel!(interactiveContentPopGestureRecognizer)) {
        navigation.interactiveContentPopGestureRecognizer()
    } else {
        None
    }
}

impl Drop for BackNavigation {
    fn drop(&mut self) {
        self.restore();
        self.events.pop.set(None);
    }
}
