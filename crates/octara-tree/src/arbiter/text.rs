use super::{InteractionArbiter, InteractionDecision, InteractionIntent, distance, guarded_node};
use crate::{NodeId, Tree};
use octara_model::{
    geometry::Point,
    identity::TargetGuard,
    input::{Modifiers, PointerKind},
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPointerTarget {
    Editor(TargetGuard),
    Region(TargetGuard),
}

impl TextPointerTarget {
    pub(super) fn at(tree: &Tree, hit: NodeId, action: Option<NodeId>) -> Option<Self> {
        if let Some(editor) = action.filter(|&node| tree.editor(node).is_some()) {
            return Some(Self::Editor(tree.guard(editor)));
        }
        tree.region_of(hit).map(|region| Self::Region(tree.guard(region)))
    }

    pub fn guard(self) -> TargetGuard {
        match self {
            Self::Editor(guard) | Self::Region(guard) => guard,
        }
    }

    pub fn resolve(self, tree: &Tree) -> Option<NodeId> {
        let node = guarded_node(tree, self.guard(), tree.front_modal())?;
        match self {
            Self::Editor(_) => tree.editor(node).is_some(),
            Self::Region(_) => tree.region_of(node) == Some(node),
        }
        .then_some(node)
    }
}

/// A guarded text gesture. Runtime resolves byte positions using the settled text projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextPointerIntent {
    Down { target: TextPointerTarget, pos: Point, count: u8, extend: bool },
    Move { target: TextPointerTarget, pos: Point },
    Clear,
}

#[derive(Clone, Copy)]
pub(super) struct TextContact {
    pub target: TextPointerTarget,
    count: u8,
    extend: bool,
}

impl TextContact {
    pub(super) fn down(self, pos: Point) -> TextPointerIntent {
        TextPointerIntent::Down { target: self.target, pos, count: self.count, extend: self.extend }
    }

    pub(super) fn moved(self, decision: &mut InteractionDecision, pos: Point) {
        decision.intents.push(InteractionIntent::TextPointer(TextPointerIntent::Move { target: self.target, pos }));
    }
}

#[derive(Clone, Copy)]
pub(super) struct TextPress {
    target: TextPointerTarget,
    kind: PointerKind,
    pos: Point,
    at: Duration,
    count: u8,
}

impl InteractionArbiter {
    pub(super) fn text_press(
        &mut self,
        target: Option<TextPointerTarget>,
        kind: PointerKind,
        pos: Point,
        mods: Modifiers,
        at: Duration,
    ) -> Option<TextContact> {
        let last = self.last_text_press.take();
        let target = target?;
        let extend = mods.contains(Modifiers::SHIFT);
        let (gap, slop) = if kind == PointerKind::Touch {
            (self.tuning.double_tap, self.tuning.double_tap_slop)
        } else {
            (self.tuning.double_click, self.tuning.drag_threshold)
        };
        let count = last
            .filter(|last| {
                !extend
                    && last.target == target
                    && last.kind == kind
                    && last.count < 3
                    && at.checked_sub(last.at).is_some_and(|elapsed| elapsed <= gap)
                    && distance(last.pos, pos) <= slop
            })
            .map_or(1, |last| last.count + 1);
        if !extend {
            self.last_text_press = Some(TextPress { target, kind, pos, at, count });
        }
        Some(TextContact { target, count, extend })
    }
}
