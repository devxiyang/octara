//! Admission order for work sharing one device queue.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub(super) const MAX_PENDING_SUBMISSIONS: usize = 2;
const DRAIN_AFTER: Duration = Duration::from_millis(33);
const AGE_AFTER: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(super) enum Lane {
    Draw,
    Drain,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Resource,
    Capture,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Canvas,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Profile,
}

const LANES: &[Lane] = &[
    Lane::Draw,
    Lane::Drain,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Lane::Resource,
    Lane::Capture,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Lane::Canvas,
    #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    Lane::Profile,
];

pub(super) struct Schedule<T> {
    queues: [VecDeque<(Instant, T)>; LANES.len()],
}

impl<T> Default for Schedule<T> {
    fn default() -> Self {
        Self { queues: std::array::from_fn(|_| VecDeque::new()) }
    }
}

impl<T> Schedule<T> {
    pub(super) fn push(&mut self, lane: Lane, at: Instant, job: T) {
        self.queues[lane as usize].push_back((at, job));
    }

    pub(super) fn remove(&mut self, mut matches: impl FnMut(&T) -> bool) -> Option<T> {
        self.queues.iter_mut().find_map(|queue| {
            let index = queue.iter().position(|(_, job)| matches(job))?;
            queue.remove(index).map(|(_, job)| job)
        })
    }

    pub(super) fn take(&mut self, now: Instant, pending: usize, _resource_running: bool) -> Option<T> {
        let at = |lane: Lane| self.queues[lane as usize].front().map(|(at, _)| *at);
        // Once a drain is due, stop feeding the queue even when both slots are occupied.
        if at(Lane::Drain).is_some_and(|at_drain| at(Lane::Draw).is_none() || now.saturating_duration_since(at_drain) >= DRAIN_AFTER) {
            return self.queues[Lane::Drain as usize].pop_front().map(|(_, job)| job);
        }
        if pending >= MAX_PENDING_SUBMISSIONS {
            return None;
        }
        let aged = |lane: Lane| at(lane).is_some_and(|at| now.saturating_duration_since(at) >= AGE_AFTER);
        let ready = |lane: Lane| match lane {
            Lane::Draw => true,
            Lane::Drain => false,
            #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
            Lane::Resource => !_resource_running,
            // Keep the other submission slot free for an interactive frame.
            Lane::Capture => pending == 0 || aged(lane),
            #[cfg(any(test, feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
            Lane::Canvas | Lane::Profile => pending == 0 || aged(lane),
        };
        let candidates = LANES.iter().copied().filter(|&lane| ready(lane)).filter_map(|lane| at(lane).map(|at| (lane, at)));
        let lane = candidates
            .clone()
            .filter(|(lane, _)| !matches!(lane, Lane::Draw) && aged(*lane))
            .min_by_key(|(_, at)| *at)
            .or_else(|| candidates.into_iter().next())
            .map(|(lane, _)| lane)?;
        self.queues[lane as usize].pop_front().map(|(_, job)| job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_orders_jobs_and_ages_background_work() {
        let now = Instant::now();
        let mut queue = Schedule::default();
        queue.push(Lane::Profile, now, "profile");
        queue.push(Lane::Canvas, now, "canvas");
        queue.push(Lane::Capture, now, "capture");
        queue.push(Lane::Resource, now, "resource");
        queue.push(Lane::Draw, now, "first draw");
        queue.push(Lane::Draw, now, "second draw");
        assert_eq!(queue.take(now, 0, false), Some("first draw"));
        assert_eq!(queue.take(now, 0, false), Some("second draw"));
        assert_eq!(queue.take(now, 0, false), Some("resource"));
        assert_eq!(queue.take(now, 0, false), Some("capture"));
        queue.push(Lane::Draw, now, "third draw");
        let later = now + AGE_AFTER;
        assert_eq!(queue.take(later, 1, false), Some("canvas"));
        assert_eq!(queue.take(later, 0, false), Some("profile"));
        assert_eq!(queue.take(later, 0, false), Some("third draw"));
    }

    #[test]
    fn drain_preempts_a_full_queue_after_its_grace_period() {
        let now = Instant::now();
        let mut queue = Schedule::default();
        queue.push(Lane::Draw, now, "draw");
        queue.push(Lane::Drain, now, "drain");
        assert_eq!(queue.take(now, MAX_PENDING_SUBMISSIONS, false), None);
        assert_eq!(queue.take(now + DRAIN_AFTER, MAX_PENDING_SUBMISSIONS, false), Some("drain"));
        assert_eq!(queue.take(now, 1, false), Some("draw"));
    }
}
