//! Publish the committed native semantic tree as UIKit VoiceOver elements.

use crate::os::WindowAccessibility;
use crate::os::{PlatformEvent, PlatformSender};
use accesskit::{Action, Affine, Live, NodeId, Rect, Role, Toggled, TreeUpdate};
use accesskit_consumer::{FullNodeId, NodeRef, Tree, TreeChangeHandler};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSArray, NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSObjectUIAccessibilityContainer, UIAccessibilityAnnouncementNotification, UIAccessibilityElement,
    UIAccessibilityIsSpeakScreenEnabled, UIAccessibilityIsSwitchControlRunning, UIAccessibilityIsVoiceOverRunning,
    UIAccessibilityLayoutChangedNotification, UIAccessibilityPostNotification, UIAccessibilityPriorityHigh, UIAccessibilityPriorityLow,
    UIAccessibilitySpeechAttributeAnnouncementPriority, UIAccessibilityTraitAdjustable, UIAccessibilityTraitButton,
    UIAccessibilityTraitHeader, UIAccessibilityTraitImage, UIAccessibilityTraitLink, UIAccessibilityTraitNone,
    UIAccessibilityTraitNotEnabled, UIAccessibilityTraitSelected, UIAccessibilityTraitStaticText, UIAccessibilityTraitToggleButton,
    UIAccessibilityTraitUpdatesFrequently, UIAccessibilityTraits, UISceneActivationState, UIView,
};
use octara::runtime::WindowId;
use octara_model::identity::TargetGuard;
use octara_model::semantics::A11yAction;
use octara_pipeline::{AccessibilityContext, NativePublication};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(super) fn platform_accessibility_active() -> bool {
    UIAccessibilityIsVoiceOverRunning() || UIAccessibilityIsSwitchControlRunning() || UIAccessibilityIsSpeakScreenEnabled()
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Actions {
    press: bool,
    focus: bool,
    increment: bool,
    decrement: bool,
}

impl Actions {
    fn read(node: NodeRef<'_>) -> Self {
        let data = node.data();
        if data.is_disabled() {
            return Self::default();
        }
        Self {
            press: data.supports_action(Action::Click),
            focus: data.supports_action(Action::Focus),
            increment: data.supports_action(Action::Increment),
            decrement: data.supports_action(Action::Decrement),
        }
    }
}

struct Route {
    view: Weak<UIView>,
    window: WindowId,
    proxy: PlatformSender,
    focused: Cell<Option<TargetGuard>>,
}

impl Route {
    fn accepts_actions(&self) -> bool {
        platform_accessibility_active()
            && self.view.load().and_then(|view| view.window()).is_some_and(|window| {
                window.isKeyWindow()
                    && window.windowScene().is_some_and(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
            })
    }
}

struct ElementState {
    target: TargetGuard,
    route: Rc<Route>,
    actions: Cell<Option<Actions>>,
}

impl ElementState {
    fn send(&self, action: A11yAction) -> bool {
        self.route.proxy.send_event(PlatformEvent::PlatformAccessibilityAction { window: self.route.window, target: self.target, action });
        true
    }
}

define_class!(
    #[unsafe(super(UIAccessibilityElement))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraVoiceOverElement"]
    #[ivars = ElementState]
    struct Element;

    unsafe impl NSObjectProtocol for Element {}

    impl Element {
        #[unsafe(method(accessibilityActivate))]
        fn activate(&self) -> bool {
            let state = self.ivars();
            state.actions.get().is_some_and(|actions| actions.press)
                && state.route.accepts_actions()
                && state.send(A11yAction::Press)
        }

        #[unsafe(method(accessibilityIncrement))]
        fn increment(&self) {
            let state = self.ivars();
            if state.actions.get().is_some_and(|actions| actions.increment) && state.route.accepts_actions() {
                state.send(A11yAction::Increment);
            }
        }

        #[unsafe(method(accessibilityDecrement))]
        fn decrement(&self) {
            let state = self.ivars();
            if state.actions.get().is_some_and(|actions| actions.decrement) && state.route.accepts_actions() {
                state.send(A11yAction::Decrement);
            }
        }

        #[unsafe(method(accessibilityElementDidBecomeFocused))]
        fn did_focus(&self) {
            let state = self.ivars();
            if let Some(actions) = state.actions.get() && state.route.accepts_actions() {
                state.route.focused.set(Some(state.target));
                if actions.focus {
                    state.send(A11yAction::Focus);
                }
            }
        }

        #[unsafe(method(accessibilityElementDidLoseFocus))]
        fn did_blur(&self) {
            let state = self.ivars();
            if state.actions.get().is_some() && state.route.focused.get() == Some(state.target) {
                state.route.focused.set(None);
            }
        }
    }
);

impl Element {
    fn new(view: &UIView, target: TargetGuard, route: Rc<Route>) -> Retained<Self> {
        let state = ElementState { target, route, actions: Cell::new(None) };
        let mtm = MainThreadMarker::from(view);
        // SAFETY: A UIView is a valid accessibility container. UIKit holds it weakly.
        let element: Retained<Self> =
            unsafe { msg_send![super(mtm.alloc::<Self>().set_ivars(state)), initWithAccessibilityContainer: view] };
        element.setIsAccessibilityElement(true);
        element
    }

    fn update(&self, previous: Option<&ElementData>, data: &ElementData) {
        self.ivars().actions.set(Some(data.actions));
        if previous.is_none_or(|previous| previous.bounds != data.bounds) {
            self.setAccessibilityFrameInContainerSpace(data.bounds);
        }
        if previous.is_none_or(|previous| previous.traits != data.traits) {
            self.setAccessibilityTraits(data.traits);
        }
        if previous.is_none_or(|previous| previous.label != data.label) {
            self.setAccessibilityLabel(data.label.as_deref().map(NSString::from_str).as_deref());
        }
        if previous.is_none_or(|previous| previous.value != data.value) {
            self.setAccessibilityValue(data.value.as_deref().map(NSString::from_str).as_deref());
        }
        if previous.is_none_or(|previous| previous.hint != data.hint) {
            self.setAccessibilityHint(data.hint.as_deref().map(NSString::from_str).as_deref());
        }
    }
}

#[derive(PartialEq)]
struct ElementData {
    bounds: NSRect,
    label: Option<String>,
    value: Option<String>,
    hint: Option<String>,
    traits: UIAccessibilityTraits,
    actions: Actions,
}

impl ElementData {
    fn read(node: NodeRef<'_>, bounds: NSRect) -> Option<Self> {
        let data = node.data();
        let actions = Actions::read(node);
        let label = name(node).or_else(|| data.placeholder().and_then(nonempty).map(str::to_owned));
        let value = joined([
            explicit_value(node, label.as_deref()),
            data.numeric_value().filter(|_| data.value().is_none()).map(|value| value.to_string()),
            data.toggled()
                .map(|checked| match checked {
                    Toggled::True => "Checked",
                    Toggled::False => "Unchecked",
                    Toggled::Mixed => "Mixed",
                })
                .map(str::to_owned),
            data.is_expanded().map(|expanded| if expanded { "Expanded" } else { "Collapsed" }.to_owned()),
        ]);
        let hint = joined([
            node.description(),
            joined(
                data.described_by().iter().filter_map(|&id| node.tree_state.node_by_tree_local_id(id, accesskit::TreeId::ROOT).map(name)),
            ),
            data.tooltip().map(str::to_owned),
        ]);
        let control = node.is_text_input() || actions != Actions::default();
        (label.is_some() || value.is_some() || hint.is_some() || control).then(|| Self {
            bounds,
            label,
            value,
            hint,
            traits: traits(node, actions),
            actions,
        })
    }
}

fn traits(node: NodeRef<'_>, actions: Actions) -> UIAccessibilityTraits {
    // SAFETY: These are immutable UIKit trait constants.
    unsafe {
        let mut traits = match node.role() {
            Role::Button | Role::RadioButton | Role::MenuItem | Role::MenuItemRadio | Role::ComboBox | Role::Tab | Role::TreeItem => {
                UIAccessibilityTraitButton
            }
            Role::CheckBox | Role::Switch | Role::MenuItemCheckBox => UIAccessibilityTraitToggleButton,
            Role::Heading | Role::ColumnHeader => UIAccessibilityTraitHeader,
            Role::Label | Role::Paragraph | Role::Blockquote | Role::Code => UIAccessibilityTraitStaticText,
            Role::Link => UIAccessibilityTraitLink,
            Role::Image => UIAccessibilityTraitImage,
            Role::ProgressIndicator => UIAccessibilityTraitUpdatesFrequently,
            _ if actions.press => UIAccessibilityTraitButton,
            _ => UIAccessibilityTraitNone,
        };
        if actions.increment || actions.decrement {
            traits |= UIAccessibilityTraitAdjustable;
        }
        if node.data().is_disabled() {
            traits |= UIAccessibilityTraitNotEnabled;
        }
        if node.data().is_selected() == Some(true) {
            traits |= UIAccessibilityTraitSelected;
        }
        traits
    }
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn name(node: NodeRef<'_>) -> Option<String> {
    node.label().or_else(|| (node.role() == Role::Label).then(|| node.value()).flatten()).filter(|name| nonempty(name).is_some())
}

fn explicit_value(node: NodeRef<'_>, label: Option<&str>) -> Option<String> {
    node.data().value()?;
    node.value().filter(|value| Some(value.as_str()) != label)
}

fn joined(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    let mut result = String::new();
    for value in values.into_iter().flatten() {
        if let Some(value) = nonempty(&value) {
            if !result.is_empty() {
                result.push_str(", ");
            }
            result.push_str(value);
        }
    }
    (!result.is_empty()).then_some(result)
}

fn walk(tree: &Tree, initial: Affine, mut visit: impl FnMut(NodeRef<'_>, Affine, Option<Rect>)) {
    let mut pending = vec![(tree.state().root(), initial, None)];
    while let Some((node, parent, clip)) = pending.pop() {
        if node.is_hidden() || node.role() == Role::TextRun {
            continue;
        }
        let transform = parent * node.direct_transform();
        visit(node, transform, clip);
        let data = node.data();
        // Projector scroll offsets already keep each scroller's own bounds fixed.
        let clip = if node.is_root() || data.scroll_x().is_some() || data.scroll_y().is_some() {
            transformed_bounds(node, transform, clip).or(clip)
        } else {
            clip
        };
        pending.extend(node.children().rev().map(|child| (child, transform, clip)));
    }
}

fn transformed_bounds(node: NodeRef<'_>, transform: Affine, clip: Option<Rect>) -> Option<Rect> {
    let bounds = transform.transform_rect_bbox(node.raw_bounds()?);
    Some(clip.map_or(bounds, |clip| bounds.intersect(clip)))
}

fn native_bounds(bounds: Rect) -> Option<NSRect> {
    if bounds.is_empty() || ![bounds.x0, bounds.y0, bounds.x1, bounds.y1].into_iter().all(f64::is_finite) {
        return None;
    }
    Some(NSRect::new(NSPoint::new(bounds.x0, bounds.y0), NSSize::new(bounds.width(), bounds.height())))
}

fn caption_of_control(node: NodeRef<'_>) -> bool {
    if node.role() != Role::Label || node.data().supports_action(Action::Focus) || node.data().supports_action(Action::Click) {
        return false;
    }
    let Some(parent) = node.parent() else { return false };
    parent.data().supports_action(Action::Click) && name(node).is_some_and(|name| parent.label().as_ref() == Some(&name))
}

#[derive(PartialEq, Eq)]
struct Announcement {
    live: Live,
    text: String,
}

fn announcements(tree: &Tree) -> Vec<(FullNodeId, Announcement)> {
    let mut regions = Vec::new();
    walk(tree, Affine::IDENTITY, |node, _, _| {
        if let Some(live @ (Live::Polite | Live::Assertive)) = node.data().live() {
            let mut text = Vec::new();
            let mut pending = vec![node];
            while let Some(descendant) = pending.pop() {
                if descendant.is_hidden() || descendant.role() == Role::TextRun {
                    continue;
                }
                if descendant.id() != node.id() && matches!(descendant.data().live(), Some(Live::Polite | Live::Assertive)) {
                    continue;
                }
                if !descendant.is_text_input() && !caption_of_control(descendant) {
                    let label = name(descendant);
                    let value = explicit_value(descendant, label.as_deref());
                    text.extend([label, value]);
                    pending.extend(descendant.children().rev());
                }
            }
            if let Some(text) = joined(text) {
                regions.push((node.id(), Announcement { live, text }));
            }
        }
    });
    regions
}

impl Announcement {
    fn post(&self) {
        let text = NSString::from_str(&self.text);
        // SAFETY: The attribute takes a UIKit announcement priority and the notification
        // accepts an attributed string.
        unsafe {
            let priority = if self.live == Live::Assertive { UIAccessibilityPriorityHigh } else { UIAccessibilityPriorityLow };
            let attributes = NSDictionary::from_slices(&[UIAccessibilitySpeechAttributeAnnouncementPriority], &[priority.as_ref()]);
            let message = NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &text, Some(&attributes));
            UIAccessibilityPostNotification(UIAccessibilityAnnouncementNotification, Some(&message));
        }
    }
}

struct IgnoreChanges;

impl TreeChangeHandler for IgnoreChanges {
    fn node_added(&mut self, _: &NodeRef<'_>) {}
    fn node_updated(&mut self, _: &NodeRef<'_>, _: &NodeRef<'_>) {}
    fn focus_moved(&mut self, _: Option<&NodeRef<'_>>, _: Option<&NodeRef<'_>>) {}
    fn node_removed(&mut self, _: &NodeRef<'_>) {}
}

struct CommittedTree {
    tree: Tree,
    context: AccessibilityContext,
    live: HashMap<FullNodeId, Announcement>,
}

impl CommittedTree {
    fn new(snapshot: TreeUpdate, context: AccessibilityContext) -> Self {
        let tree = Tree::new(snapshot, true);
        let live = announcements(&tree).into_iter().collect();
        Self { tree, context, live }
    }

    fn update(&mut self, update: TreeUpdate, context: AccessibilityContext, pending: &mut Vec<Announcement>) {
        if self.context.tree != context.tree {
            *self = Self::new(update, context);
            return;
        }
        self.tree.update_and_process_changes(update, &mut IgnoreChanges);
        self.context = context;
        let mut live = HashMap::new();
        for (id, announcement) in announcements(&self.tree) {
            if self.live.get(&id) != Some(&announcement) {
                pending.push(Announcement { live: announcement.live, text: announcement.text.clone() });
            }
            live.insert(id, announcement);
        }
        self.live = live;
    }

    fn focus(&self) -> NodeId {
        let tree = self.tree.state();
        let focused = tree.focus_in_tree();
        let target = focused.active_descendant().unwrap_or(focused).id();
        tree.locate_node(target).expect("focused node exists in the committed tree").0
    }
}

pub(super) struct Bridge {
    view: Retained<UIView>,
    route: Rc<Route>,
    tree: Option<CommittedTree>,
    elements: HashMap<TargetGuard, (Retained<Element>, ElementData)>,
    order: Vec<TargetGuard>,
}

impl Bridge {
    pub(super) fn new(view: Retained<UIView>, window: WindowId, proxy: PlatformSender) -> Self {
        let route = Rc::new(Route { view: Weak::new(&*view), window, proxy, focused: Cell::new(None) });
        Self { view, route, tree: None, elements: HashMap::new(), order: Vec::new() }
    }
}

impl WindowAccessibility for Bridge {
    fn synchronize(&mut self, publication: &NativePublication) {
        let commits = publication.take_accessibility_commits();
        let before = self.tree.as_ref().map(|tree| (tree.context, tree.focus()));
        let mut announcements = Vec::new();
        if let Some(tree) = &mut self.tree {
            if commits.is_empty() {
                return;
            }
            for commit in commits {
                tree.update(commit.update.as_ref().clone(), commit.context, &mut announcements);
            }
        } else if let Some(context) = publication.accessibility_context()
            && let Some(snapshot) = publication.snapshot_full()
        {
            self.tree = Some(CommittedTree::new(snapshot, context));
        }
        let Some(tree) = &self.tree else { return };
        let origin = self.view.bounds().origin;
        // AccessKit starts in physical pixels. UIKit frames use the view's points.
        let transform = Affine::translate((origin.x, origin.y)) * Affine::scale(1.0 / f64::from(tree.context.scale));
        let mut order = Vec::new();
        let mut exposed = HashSet::new();
        walk(&tree.tree, transform, |node, transform, clip| {
            if node.is_root() || caption_of_control(node) {
                return;
            }
            let Some((id, _)) = tree.tree.state().locate_node(node.id()) else { return };
            let Some(target) = publication.accessibility_target(id) else { return };
            let Some(data) =
                transformed_bounds(node, transform, clip).and_then(native_bounds).and_then(|bounds| ElementData::read(node, bounds))
            else {
                return;
            };
            exposed.insert(target);
            order.push(target);
            match self.elements.entry(target) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let (element, previous) = entry.get_mut();
                    element.update(Some(previous), &data);
                    *previous = data;
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let element = Element::new(&self.view, target, self.route.clone());
                    element.update(None, &data);
                    entry.insert((element, data));
                }
            }
        });
        self.elements.retain(|target, (element, _)| {
            let keep = exposed.contains(target);
            if !keep {
                element.ivars().actions.set(None);
            }
            keep
        });
        let order_changed = before.is_none() || order != self.order;
        if order_changed {
            let elements: Vec<&Element> = order.iter().map(|target| &*self.elements[target].0).collect();
            let array = NSArray::from_slice(&elements);
            // SAFETY: The array contains UIAccessibilityElements for this UIView.
            unsafe { self.view.setAccessibilityElements(Some(array.cast_unchecked()), self.view.mtm()) };
            self.order = order;
        }

        let modal_changed = before.is_none_or(|(previous, _)| previous.tree != tree.context.tree || previous.modal != tree.context.modal);
        if modal_changed {
            self.view.setAccessibilityViewIsModal(tree.context.modal.is_some(), self.view.mtm());
        }

        let focus = tree.focus();
        let target = publication.accessibility_target(focus).filter(|target| exposed.contains(target));
        let old_focus = self.route.focused.get();
        let lost_focus = old_focus.is_some_and(|target| !exposed.contains(&target));
        if lost_focus {
            self.route.focused.set(None);
        }
        let focus_changed = before.is_none_or(|(previous, id)| previous.tree != tree.context.tree || id != focus);
        if self.route.accepts_actions() {
            let should_move_focus = modal_changed || lost_focus || (focus_changed && target != old_focus);
            if should_move_focus || order_changed {
                let destination = if modal_changed || lost_focus {
                    target.or_else(|| self.order.first().copied())
                } else if should_move_focus {
                    target
                } else {
                    None
                };
                let element = destination.and_then(|target| self.elements.get(&target)).map(|(element, _)| {
                    let element: &AnyObject = element;
                    element
                });
                // SAFETY: A layout notification takes an optional accessible element.
                unsafe { UIAccessibilityPostNotification(UIAccessibilityLayoutChangedNotification, element) };
            }
            for announcement in announcements {
                announcement.post();
            }
        }
    }

    fn clear(&mut self) {
        if self.tree.take().is_none() {
            return;
        }
        self.route.focused.set(None);
        for (element, _) in self.elements.values() {
            element.ivars().actions.set(None);
        }
        // SAFETY: Reset the container to UIKit's default accessibility traversal.
        unsafe { self.view.setAccessibilityElements(None, self.view.mtm()) };
        self.view.setAccessibilityViewIsModal(false, self.view.mtm());
        self.elements.clear();
        self.order.clear();
    }

    fn accepts_action(&self, publication: &NativePublication, target: TargetGuard, action: &A11yAction) -> bool {
        let action = match action {
            A11yAction::Press => Action::Click,
            A11yAction::Focus => Action::Focus,
            A11yAction::Increment => Action::Increment,
            A11yAction::Decrement => Action::Decrement,
            _ => return false,
        };
        self.route.accepts_actions() && self.elements.contains_key(&target) && publication.accessibility_supports_action(target, action)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::{Node, TreeId, TreeInfo};
    use octara_model::identity::TreeId as OctaraTreeId;

    const ROOT: NodeId = NodeId(1);
    const REGION: NodeId = NodeId(2);
    const LABEL: NodeId = NodeId(3);
    const CONTROL: NodeId = NodeId(4);

    fn label(value: &str) -> Node {
        let mut label = Node::new(Role::Label);
        label.set_value(value);
        label.set_transform(Affine::translate((5.0, 9.0)));
        label.set_bounds(Rect::new(0.0, 0.0, 35.0, 16.0));
        label
    }

    fn fixture() -> CommittedTree {
        let mut root = Node::new(Role::Window);
        root.set_children(vec![REGION, CONTROL]);
        root.set_transform(Affine::scale(3.0));
        let mut region = Node::new(Role::GenericContainer);
        region.set_live(Live::Polite);
        region.set_children(vec![LABEL]);
        // A scroll offset is part of the committed ancestor transform.
        region.set_transform(Affine::translate((12.0, -4.0)));
        let mut control = Node::new(Role::Slider);
        control.set_label("Volume");
        control.set_numeric_value(20.0);
        control.set_toggled(Toggled::Mixed);
        control.set_expanded(false);
        control.add_action(Action::Focus);
        control.add_action(Action::Increment);
        control.add_action(Action::Decrement);
        let snapshot = TreeUpdate {
            nodes: vec![(CONTROL, control), (LABEL, label("Waiting")), (REGION, region), (ROOT, root)],
            tree: Some(TreeInfo::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus: CONTROL,
        };
        CommittedTree::new(snapshot, AccessibilityContext { tree: OctaraTreeId::new([1, 2], 1), modal: None, scale: 3.0 })
    }

    #[test]
    fn traversal_composes_ancestor_transforms_in_view_points() {
        let fixture = fixture();
        let mut order = Vec::new();
        walk(&fixture.tree, Affine::scale(1.0 / f64::from(fixture.context.scale)), |node, transform, clip| {
            let (id, _) = node.tree_state.locate_node(node.id()).unwrap();
            order.push(id);
            if id == LABEL {
                assert_eq!(
                    transformed_bounds(node, transform, clip).and_then(native_bounds),
                    Some(NSRect::new(NSPoint::new(17.0, 5.0), NSSize::new(35.0, 16.0)))
                );
            }
        });
        assert_eq!(order, [ROOT, REGION, LABEL, CONTROL]);
        assert_eq!(fixture.focus(), CONTROL);
    }

    #[test]
    fn scrollport_clips_descendants_without_clipping_the_footer() {
        let mut fixture = fixture();
        let state = fixture.tree.state();
        let mut region = state.node_by_tree_local_id(REGION, TreeId::ROOT).unwrap().data().clone();
        region.set_scroll_y(4.0);
        region.set_bounds(Rect::new(0.0, 4.0, 25.0, 14.0));
        let mut footer = state.node_by_tree_local_id(CONTROL, TreeId::ROOT).unwrap().data().clone();
        footer.set_bounds(Rect::new(0.0, 14.0, 80.0, 34.0));
        let update = |nodes| TreeUpdate { nodes, tree: None, tree_id: TreeId::ROOT, focus: CONTROL };
        fixture.update(update(vec![(REGION, region), (CONTROL, footer)]), fixture.context, &mut Vec::new());
        let bounds_of = |tree: &Tree, id| {
            let mut bounds = None;
            walk(tree, Affine::scale(1.0 / 3.0), |node, transform, clip| {
                if node.tree_state.locate_node(node.id()).unwrap().0 == id {
                    bounds = transformed_bounds(node, transform, clip).and_then(native_bounds);
                }
            });
            bounds
        };
        assert_eq!(bounds_of(&fixture.tree, LABEL), Some(NSRect::new(NSPoint::new(17.0, 5.0), NSSize::new(20.0, 5.0))));
        let mut offscreen = label("Waiting");
        offscreen.set_transform(Affine::translate((5.0, 40.0)));
        fixture.update(update(vec![(LABEL, offscreen)]), fixture.context, &mut Vec::new());
        assert_eq!(bounds_of(&fixture.tree, LABEL), None);
        assert_eq!(bounds_of(&fixture.tree, CONTROL), Some(NSRect::new(NSPoint::new(0.0, 14.0), NSSize::new(80.0, 20.0))));
    }

    #[test]
    fn live_region_announces_each_text_commit_without_repeating_on_geometry_updates() {
        let mut fixture = fixture();
        let mut pending = Vec::new();
        let update = |node| TreeUpdate { nodes: vec![(LABEL, node)], tree: None, tree_id: TreeId::ROOT, focus: CONTROL };
        fixture.update(update(label("Loading")), fixture.context, &mut pending);
        fixture.update(update(label("Ready")), fixture.context, &mut pending);
        let mut moved = label("Ready");
        moved.set_transform(Affine::translate((0.0, 40.0)));
        fixture.update(update(moved), fixture.context, &mut pending);
        assert_eq!(pending.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["Loading", "Ready"]);
    }

    #[test]
    fn adjustable_traits_and_callbacks_follow_committed_state() {
        let mut fixture = fixture();
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(10.0, 10.0));
        let control = |tree: &Tree| ElementData::read(tree.state().node_by_tree_local_id(CONTROL, TreeId::ROOT).unwrap(), bounds).unwrap();
        let enabled = control(&fixture.tree);
        assert_eq!(enabled.label.as_deref(), Some("Volume"));
        assert_eq!(enabled.value.as_deref(), Some("20, Mixed, Collapsed"));
        assert!(enabled.actions.focus && enabled.actions.increment && enabled.actions.decrement);
        assert!(!enabled.actions.press);
        // SAFETY: UIKit exports these immutable trait constants.
        assert_ne!(enabled.traits & unsafe { UIAccessibilityTraitAdjustable }, 0);

        let mut disabled = fixture.tree.state().focus_in_tree().data().clone();
        disabled.set_disabled();
        let update = TreeUpdate { nodes: vec![(CONTROL, disabled)], tree: None, tree_id: TreeId::ROOT, focus: ROOT };
        fixture.update(update, fixture.context, &mut Vec::new());
        let disabled = control(&fixture.tree);
        assert!(disabled.actions == Actions::default());
        // SAFETY: UIKit exports these immutable trait constants.
        assert_ne!(disabled.traits & unsafe { UIAccessibilityTraitNotEnabled }, 0);
    }
}
