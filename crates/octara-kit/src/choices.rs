//! Pure choice calculations. Components own filtering, focus and commitment policies.

use std::collections::HashSet;

pub(super) fn unique_ids<'a>(mut ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    ids.all(|id| seen.insert(id))
}

/// Wrap within already enabled IDs. An absent/stale active ID starts at the corresponding
/// edge; an empty list has no destination. This never changes selection or component state.
pub(super) fn adjacent_id<'a>(ids: &'a [String], active: Option<&str>, backward: bool) -> Option<&'a str> {
    if ids.is_empty() {
        return None;
    }
    let at = active.and_then(|id| ids.iter().position(|item| item == id));
    let next = match (backward, at) {
        (true, Some(0) | None) => ids.len() - 1,
        (true, Some(at)) => at - 1,
        (false, Some(at)) => (at + 1) % ids.len(),
        (false, None) => 0,
    };
    Some(ids[next].as_str())
}
