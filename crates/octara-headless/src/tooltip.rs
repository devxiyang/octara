//! Delayed, anchored descriptions for hover and keyboard focus.

use crate::id::Id;
use octara::prelude::*;
use std::time::Duration;

/// Delays are measured by the application's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooltipOptions {
    pub open_delay: Duration,
    /// Time to travel between the anchor and the tooltip with the pointer.
    pub close_delay: Duration,
    /// Time after closing during which another tooltip can open immediately.
    pub cooldown: Duration,
}

impl Default for TooltipOptions {
    fn default() -> Self {
        Self { open_delay: Duration::from_millis(700), close_delay: Duration::from_millis(100), cooldown: Duration::from_millis(300) }
    }
}

#[derive(Clone, Copy, Default)]
struct Warmth {
    open: Option<(Id, ScopeId)>,
    cools_at: Duration,
}

/// Shares warmup between tooltip triggers. At most one tooltip in a group is visible.
#[derive(Clone, Copy)]
pub struct TooltipGroup(Signal<Warmth>);

impl TooltipGroup {
    /// Creates a group in a build scope shared by all its triggers.
    pub fn new(build: &mut BuildCx<'_>) -> Self {
        Self(build.signal(Warmth::default()))
    }

    fn is_warm(self, write: &mut WriteCx<'_>) -> bool {
        let warmth = write.get(self.0);
        write.now() < warmth.cools_at || warmth.open.is_some_and(|(_, owner)| write.is_scope_alive(owner))
    }
}

#[derive(Clone, Copy, Default)]
struct Activity {
    trigger_hovered: bool,
    panel_hovered: bool,
    focused: bool,
    dismissed: bool,
}

impl Activity {
    fn engaged(self) -> bool {
        self.trigger_hovered || self.panel_hovered || self.focused
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Trigger,
    Panel,
    Focus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transition {
    Open,
    Close,
}

/// A scope-owned tooltip that can be attached directly to a caller's element.
#[derive(Clone, Copy)]
pub struct TooltipState {
    id: Id,
    owner: ScopeId,
    group: TooltipGroup,
    options: TooltipOptions,
    activity: Signal<Activity>,
    timer: Signal<Option<(Transition, TaskId)>>,
}

impl TooltipState {
    /// Creates a tooltip in the build scope that owns its trigger.
    pub fn new(build: &mut BuildCx<'_>, options: TooltipOptions) -> Self {
        let group = TooltipGroup::new(build);
        Self::in_group(build, group, options)
    }

    pub fn in_group(build: &mut BuildCx<'_>, group: TooltipGroup, options: TooltipOptions) -> Self {
        Self { id: Id::new(), owner: build.scope(), group, options, activity: build.signal(Activity::default()), timer: build.signal(None) }
    }

    pub fn is_open(self, read: &mut ReadCx<'_>) -> bool {
        read.with(self.group.0, |warmth| warmth.open.is_some_and(|(id, _)| id == self.id))
    }

    /// Closes visible content or cancels its pending opening until the trigger is left.
    pub fn dismiss(self, write: &mut WriteCx<'_>) -> bool {
        let open = write.read(|read| self.is_open(read));
        let opening = write.with(self.timer, |timer| matches!(timer, Some((Transition::Open, _))));
        if !open && !opening {
            return false;
        }

        self.cancel_timer(write);
        write.update(self.activity, |activity| {
            activity.panel_hovered = false;
            activity.dismissed = activity.engaged();
        });
        if open {
            self.finish(write, Transition::Close);
        }
        true
    }

    /// Describes the focusable trigger with the portaled tooltip while it is open.
    pub fn trigger<C>(
        self,
        element: El<C>,
        placement: Placement,
        target: Option<AnchorTarget>,
        mut content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> El<C> {
        element
            .focusable()
            .described_by_fallback(move |read| self.is_open(read).then(|| self.id.part("content")))
            .behavior(|behavior| {
                behavior.on_hover_change(move |write, hovered| self.activity(write, Source::Trigger, hovered)).on_key(move |write, key| {
                    if key.key == Key::Escape && key.modifiers.is_empty() && !write.read(|read| self.is_open(read)) {
                        self.dismiss(write);
                    }
                    false
                })
            })
            .on_focus(move |write| self.activity(write, Source::Focus, true))
            .on_blur(move |write| self.activity(write, Source::Focus, false))
            .overlay_target(placement, target, move |read| self.is_open(read), move |build| self.panel(content(build)))
    }

    fn panel<C>(self, element: El<C>) -> El<C> {
        let policy = LayerPolicy { dismiss_on_outside_press: false, dismiss_on_focus_leave: false, ..LayerPolicy::non_modal() };
        element
            .id(self.id.part("content"))
            .role(Role::Tooltip)
            .behavior(|behavior| behavior.on_hover_change(move |write, hovered| self.activity(write, Source::Panel, hovered)))
            .layer(policy, move |write| {
                self.dismiss(write);
            })
    }

    fn activity(self, write: &mut WriteCx<'_>, source: Source, active: bool) {
        let open = write.read(|read| self.is_open(read));
        let mut activity = write.get(self.activity);
        match source {
            Source::Trigger => activity.trigger_hovered = active,
            Source::Panel => activity.panel_hovered = active,
            Source::Focus => activity.focused = active,
        }
        activity.panel_hovered &= open;
        let engaged = activity.engaged();
        activity.dismissed &= engaged;
        write.set(self.activity, activity);

        let should_open = engaged && !activity.dismissed;
        if should_open == open {
            self.cancel_timer(write);
            return;
        }
        if should_open && !active {
            return;
        }

        let (transition, delay) = if should_open {
            (Transition::Open, if self.group.is_warm(write) { Duration::ZERO } else { self.options.open_delay })
        } else {
            (Transition::Close, if source == Source::Focus { Duration::ZERO } else { self.options.close_delay })
        };
        self.schedule(write, transition, delay);
    }

    fn schedule(self, write: &mut WriteCx<'_>, transition: Transition, delay: Duration) {
        if !delay.is_zero() && write.with(self.timer, |timer| timer.is_some_and(|(pending, _)| pending == transition)) {
            return;
        }
        self.cancel_timer(write);
        if delay.is_zero() {
            self.finish(write, transition);
            return;
        }

        let deadline = write.now().saturating_add(delay);
        let timer = write.spawn_in(self.owner, "tooltip delay", move |task| async move {
            task.sleep(deadline.saturating_sub(task.now())).await;
            task.write(|write| {
                write.set(self.timer, None);
                self.finish(write, transition);
            });
        });
        write.set(self.timer, Some((transition, timer)));
    }

    fn cancel_timer(self, write: &mut WriteCx<'_>) {
        if let Some((_, task)) = write.get(self.timer) {
            write.cancel_task(task);
            write.set(self.timer, None);
        }
    }

    fn finish(self, write: &mut WriteCx<'_>, transition: Transition) {
        match transition {
            Transition::Open => write.update(self.group.0, |warmth| warmth.open = Some((self.id, self.owner))),
            Transition::Close if write.read(|read| self.is_open(read)) => {
                let cools_at = write.now().saturating_add(self.options.cooldown);
                write.set(self.group.0, Warmth { open: None, cools_at });
            }
            Transition::Close => {}
        }
    }
}
