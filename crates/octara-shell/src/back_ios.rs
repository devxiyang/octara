//! The UIKit navigation controller settles its own interactive pop. Only the settled
//! result can retire the Octara page that was visible when the transition started.

use super::{ShellEvent, scene_ios};
use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::NSArray;
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIGestureRecognizer, UIGestureRecognizerDelegate, UINavigationController, UINavigationControllerDelegate,
    UIView, UIViewAutoresizing, UIViewController, UIViewControllerTransitionCoordinator, UIViewControllerTransitionCoordinatorContext,
    UIWindow,
};
use octara::app::BackPhase;
use octara::kernel::HandlerId;
use std::cell::Cell;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use winit::event_loop::EventLoopProxy;
use winit::window::{Window, WindowId};

#[derive(Clone, Copy)]
struct Pop {
    id: u64,
    owner: Option<HandlerId>,
    interactive: bool,
    coordinated: bool,
    cancelled: Option<bool>,
}

struct PopEvents {
    proxy: EventLoopProxy<ShellEvent>,
    window: WindowId,
    drawn_owner: Cell<Option<HandlerId>>,
    pop: Cell<Option<Pop>>,
}

impl PopEvents {
    fn send(&self, pop: Pop, phase: BackPhase) {
        let _ = self.proxy.send_event(ShellEvent::IosBack {
            window: self.window,
            id: pop.id,
            owner: pop.owner,
            phase,
            interactive: pop.interactive,
        });
    }

    fn finish(&self, id: u64, cancelled: bool) {
        let Some(mut pop) = self.pop.get().filter(|pop| pop.id == id && pop.cancelled.is_none()) else { return };
        pop.cancelled = Some(cancelled);
        self.pop.set(Some(pop));
        self.send(pop, if cancelled { BackPhase::Cancelled } else { BackPhase::Completed });
    }
}

struct ObserverState {
    navigation: Weak<UINavigationController>,
    content: Weak<UIViewController>,
    preview: Retained<UIViewController>,
    events: Rc<PopEvents>,
}

type TransitionContext = ProtocolObject<dyn UIViewControllerTransitionCoordinatorContext>;

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
            let pop = Pop { id, owner: state.events.drawn_owner.get(), interactive, coordinated: coordinator.is_some(), cancelled: None };
            state.events.pop.set(Some(pop));
            state.events.send(pop, BackPhase::Started);
            let Some(coordinator) = coordinator else { return };
            if interactive {
                let events = state.events.clone();
                let progress = RcBlock::new(move |context: NonNull<TransitionContext>| {
                    // SAFETY: UIKit supplies a live context for the duration of this callback.
                    let context = unsafe { context.as_ref() };
                    if context.isInteractive() || !events.pop.get().is_some_and(|current| current.id == id && current.cancelled.is_none()) {
                        return;
                    }
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
            coordinator.animateAlongsideTransition_completion(None, Some(&completion));
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
    snapshot: Retained<UIView>,
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
pub(crate) struct BackNavigation {
    window: Retained<UIWindow>,
    content: Retained<UIViewController>,
    view: Retained<UIView>,
    events: Rc<PopEvents>,
    stack: Option<Stack>,
    handoff: Option<Retained<UIView>>,
}

impl BackNavigation {
    pub(crate) fn new(window: &Window, proxy: EventLoopProxy<ShellEvent>) -> Result<Self, String> {
        let view = scene_ios::ui_view(window)?;
        let content = scene_ios::ui_view_controller(window)?;
        let native = view.window().ok_or("the UIKit view has no window")?;
        let events = Rc::new(PopEvents { proxy, window: window.id(), drawn_owner: Cell::new(None), pop: Cell::new(None) });
        Ok(Self { window: native, content, view, events, stack: None, handoff: None })
    }

    pub(crate) fn armed(&self) -> bool {
        self.stack.is_some()
    }

    pub(crate) fn arm(&mut self) -> Result<(), String> {
        let mtm = MainThreadMarker::from(&*self.window);
        let snapshot = self.view.snapshotViewAfterScreenUpdates(false).ok_or("UIKit could not snapshot the last presented page")?;
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
        let state =
            ObserverState { navigation: Weak::new(&*navigation), content: Weak::new(&*self.content), preview, events: self.events.clone() };
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
        self.stack = Some(Stack { navigation, observer, snapshot, recognizers });
        if !installed {
            self.restore(false);
            return Err("UIKit did not attach its interactive pop recognizer to the navigation stack".into());
        }
        Ok(())
    }

    pub(crate) fn owns(&self, id: u64) -> bool {
        self.events.pop.get().is_some_and(|pop| pop.id == id)
    }

    pub(crate) fn disarm(&mut self) {
        if self.events.pop.get().is_none() {
            self.restore(false);
        }
    }

    pub(crate) fn settle(&mut self, id: u64, accepted: bool, eligible: bool) -> bool {
        let Some(stack) = &self.stack else { return false };
        let Some(cancelled) = self.events.pop.get().filter(|pop| pop.id == id).and_then(|pop| pop.cancelled) else { return false };
        self.events.pop.set(None);
        if !eligible || (!cancelled && accepted) {
            self.restore(!cancelled && accepted);
            return true;
        }
        if !cancelled {
            // UIKit popped the controller, but the originating app handler has since retired.
            // Bring the live view back without changing the current Octara page.
            stack.navigation.setViewControllers_animated(&NSArray::from_slice(&[stack.preview(), &*self.content]), false);
            return true;
        }
        false
    }

    fn restore(&mut self, retain_preview_for_draw: bool) {
        let Some(stack) = self.stack.take() else { return };
        self.events.drawn_owner.set(None);
        stack.detach();
        stack.navigation.setViewControllers_animated(&NSArray::from_slice(&[stack.preview()]), false);
        self.window.setRootViewController(Some(&self.content));
        if retain_preview_for_draw {
            stack.preview().setView(None);
            stack.snapshot.setFrame(self.view.bounds());
            stack.snapshot.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
            self.view.addSubview(&stack.snapshot);
            self.handoff = Some(stack.snapshot);
        }
    }

    pub(crate) fn did_draw(&mut self, owner: Option<HandlerId>) {
        if self.stack.is_some() {
            self.events.drawn_owner.set(owner);
        }
        if let Some(snapshot) = self.handoff.take() {
            snapshot.removeFromSuperview();
        }
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
        if let Some(stack) = &self.stack {
            stack.detach();
        }
        self.events.pop.set(None);
    }
}
