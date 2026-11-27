//! Keep the retired DOM in its paint slot while the browser fades it out.

use super::DomEngine;
use octara::runtime::engine::ProjectFeedback;
use octara::tree::{ExitTicket, NodeId, RetiredNode, Tree, TreeDelta};
use octara_model::input::HostReply;
use octara_model::motion::{Curve, PropClass, Track};
use rustc_hash::FxHashMap;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use web_sys::HtmlElement;

#[wasm_bindgen(module = "/src/exit.js")]
extern "C" {
    fn octara_exit_style(element: &HtmlElement, visible_only: bool) -> Option<String>;
    fn octara_exit_opacity(element: &HtmlElement) -> f64;
    fn octara_exit_reduced_motion(element: &HtmlElement) -> bool;
    #[wasm_bindgen(catch)]
    fn octara_exit_start(element: &HtmlElement, opacity: &[f64], duration_ms: f64) -> Result<JsValue, JsValue>;
    fn octara_exit_remaining(animation: &JsValue, element: &HtmlElement) -> f64;
    fn octara_exit_cancel(animation: &JsValue);
}

pub(super) struct ExitPlan {
    ticket: ExitTicket,
    style: String,
    opacity: Vec<f64>,
    duration_ms: f64,
    ends: Duration,
}

impl ExitPlan {
    fn capture(element: &HtmlElement, tree: &Tree, ticket: ExitTicket, visible_only: bool, now: f64, inherited: f64) -> Option<Self> {
        let curve = tree.visual(ticket.node).motion.opacity;
        if matches!(curve, Curve::Instant) || octara_exit_reduced_motion(element) {
            return None;
        }
        let mut track = Track::new(octara_exit_opacity(element) * inherited);
        track.retarget(0.0, 0.0, curve, PropClass::Opacity);
        let duration = track.rest_at(0.0)?;
        let ends = Duration::try_from_secs_f64(now + duration).ok()?;
        let style = octara_exit_style(element, visible_only)?;
        let segment = match curve {
            Curve::Spring(spring) => f64::from(spring.duration) / 24.0,
            _ => duration / 48.0,
        };
        let steps = (duration / segment).ceil().clamp(1.0, 1024.0) as u32;
        let opacity = (0..=steps).map(|step| track.value(duration * f64::from(step) / f64::from(steps)).clamp(0.0, 1.0)).collect();
        Some(Self { ticket, style, opacity, duration_ms: duration * 1000.0, ends })
    }
}

pub(super) struct ExitAnimation {
    ticket: ExitTicket,
    element: HtmlElement,
    animation: JsValue,
    ends: Duration,
}

impl Drop for ExitAnimation {
    fn drop(&mut self) {
        octara_exit_cancel(&self.animation);
    }
}

impl DomEngine {
    pub(super) fn capture_exits(&self, tree: &Tree, delta: &TreeDelta, now: f64) -> FxHashMap<NodeId, ExitPlan> {
        delta
            .retired
            .iter()
            .filter_map(|retired| {
                let RetiredNode::Exiting { ticket, visible } = *retired else { return None };
                if !tree.accepts_exit(ticket) {
                    return None;
                }
                let element = self.projected_element(ticket.node)?;
                let inherited = std::iter::successors(tree.physical_parent(ticket.node), |&parent| tree.physical_parent(parent))
                    .take_while(|&parent| !tree.is_live(parent))
                    .filter_map(|parent| self.projected_element(parent))
                    .map(|element| octara_exit_opacity(&element))
                    .product();
                Some((ticket.node, ExitPlan::capture(&element, tree, ticket, visible, now, inherited)?))
            })
            .collect()
    }

    pub(super) fn stage_exit(&mut self, node: NodeId, element: &HtmlElement, plan: Option<ExitPlan>) -> Result<(), JsValue> {
        let undo = &mut self.pending().undo;
        undo.attr(element, "inert", Some(""))?;
        undo.attr(element, "aria-hidden", Some("true"))?;
        let style = plan.as_ref().map_or("display:none", |plan| plan.style.as_str());
        undo.attr(element, "style", Some(style))?;
        if let Some(plan) = plan {
            self.pending().exits.insert(node, plan);
        }
        Ok(())
    }

    pub(super) fn publish_exits(&mut self, exits: FxHashMap<NodeId, ExitPlan>) {
        for (node, plan) in exits {
            if let Some(element) = self.dom.element(node) {
                let animation = octara_exit_start(&element, &plan.opacity, plan.duration_ms).unwrap_or(JsValue::NULL);
                self.exits.insert(node, ExitAnimation { ticket: plan.ticket, element, animation, ends: plan.ends });
            }
        }
    }

    pub(super) fn report_exits(&self, tree: &Tree, now: f64, feedback: &mut ProjectFeedback) {
        let pending = self.pending.as_ref().expect("an open DOM projection");
        let now = Duration::try_from_secs_f64(now).unwrap_or_default();
        let mut finished = Vec::new();
        for (&node, exit) in &self.exits {
            if !tree.accepts_exit(exit.ticket) || pending.retired.contains(&node) {
                continue;
            }
            let remaining = octara_exit_remaining(&exit.animation, &exit.element);
            if remaining <= 0.0 {
                finished.push(exit.ticket);
                continue;
            }
            // The predicted deadline wakes the host. WAAPI may start on a later frame.
            let deadline = if exit.ends > now {
                exit.ends
            } else {
                now.saturating_add(Duration::try_from_secs_f64((remaining / 1000.0).max(1.0 / 60.0)).unwrap_or(Duration::MAX))
            };
            feedback.next_deadline = Some(feedback.next_deadline.map_or(deadline, |previous| previous.min(deadline)));
        }
        finished.sort_unstable_by_key(|ticket| ticket.node);
        feedback.replies.extend(finished.into_iter().map(HostReply::ExitFinished));
        for exit in pending.exits.values().filter(|exit| tree.accepts_exit(exit.ticket)) {
            feedback.next_deadline = Some(feedback.next_deadline.map_or(exit.ends, |deadline| deadline.min(exit.ends)));
        }
    }
}
