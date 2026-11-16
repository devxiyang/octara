# Latest Rune and Exact: collections, regions and retained input

Source review on **2026-11-16**. GitHub branch heads were resolved before fetching individual
files at immutable revisions. Local clones were used to locate older code, not as evidence of
the current upstream implementation.

| Project | Branch | Inspected head | Commit time (UTC) | Older local baseline |
| --- | --- | --- | --- | --- |
| Rune | `dev` | [`631e8a67`](https://github.com/anomalyco/rune/commit/631e8a67cb3c40225efa3f6c4f57b80e7b18f190) | 2026-11-16 00:13:55 | `3686e94` |
| Exact | `main` | [`5491ab29`](https://github.com/expo/exact/commit/5491ab299cc6ed94857cacfd54fee093550a70c7) | 2026-11-16 02:55:34 | `08c889a9` |

The compare endpoints reported 501 and 2,755 commits respectively since those local baselines.
This is a targeted mechanism review, not a review of every intervening commit. Neither project's
tests, benchmarks nor native hosts were executed here. The [Octara proposal](../design/collection-regions.md)
is a design derived from this evidence, not a claim that either reference implements Signals
or that Octara has already implemented the proposed replacement.

## Rune: ordinary values with explicit reuse boundaries

### Tracked reads have a specific owner

Latest Rune has `Tracked<T>`, `region(id, build)` and `region_with(id, inputs, build)`.
`Tracked` owns an ordinary value; shared dereferencing inside a region records its cell/version,
and mutable dereferencing increments the version. A mutable borrow conservatively counts as a
write even if nothing changes. A region reuses its subtree while its recorded state/layout/caret
reads and environment remain valid. Explicit inputs cover values computed outside that region.
[Tracked implementation][r-tracked], [region and memo implementation][r-memo].

The November 12 introduction described tracked reads for memos as well. The subsequent
correction limits automatic state recording to regions: ordinary `memo(id, version, build)`
continues to trust its supplied version, with recorded environmental/layout dependencies and
nested regions. A clock read had otherwise caused rebuilding every frame. Read the current
implementation rather than treating the first changelog as the final contract.
[Introduction][r-intro], [tracking-boundary correction][r-boundary].

**Lesson for Octara:** automatic dependencies must belong to a well-defined computation.
Ordinary Rust struct access remains useful, but importing Rune's manual memo versions beside
Signals would create another freshness protocol. Keep collection derivation in Octara's graph.

**Cost and limitation:** this is subtree reuse within Rune's view-build model, not Octara's
per-property Signals model. Interior mutation needs explicit notification, and a swapped-out
Tracked cell kept alive is documented as a case that does not invalidate until the appropriate
write/drop. Transparent-looking value wrappers do not eliminate identity/lifetime rules.

### Reuse needs evidence, including its blind spots

Debug builds construct reused regions again and compare their printed element descriptions,
reporting the first disagreement. `RUNE_REGION_MISS` reports an invalidating state version or
layout/caret/selection read. The comparison intentionally excludes handlers, so it cannot prove
that a retained callback captures the current payload. The module documentation explicitly warns
that memoized handlers keep their construction-time captures and should act by item ID rather
than row index. [Memo source and debug comparison][r-memo].

**Lesson:** use from-scratch equivalence for collection output and selected-region explanations
for Agents, then test actual callbacks separately. A matching screenshot does not prove that
clicking the row acts on its current item. Octara already has consistency checks; extend those
rather than adding an autonomous improvement engine or a global provenance database.

### Virtualization is a workload boundary with lifecycle costs

`virtual_rows` accepts count, per-index key and builder functions; it builds the mounted range
instead of constructing all children first. Without an explicit key version it still checks
keys across the list; the optional `virtual_rows_version` skips that scan but requires callers
to change the version for reorder/replacement. Rows that leave the built range lose their memo
cache; a focused editor has a special retention path. [Virtual row API][r-rows].

**Lesson:** share row semantics across list modes while keeping viewport/measurement work local.
Generate freshness internally in Octara. Avoid promising that ordinary whole-Vec replacement
can skip inspecting keys, and do not equate virtualization with preserving all row-local state.

Recent fixes show the failure modes behind the optimization:

- Reaching a fill budget used to leave the list blank until another event requested a frame.
  The replacement keeps resumable work with backoff and ignores obsolete wakes.
  [Fill resumption][r-resume].
- After a jump, missing-row counts included the gap between the old and new windows. Moving
  the window then appeared to be progress without mounting the rows it needed.
  [Disjoint-window accounting][r-disjoint].
- End-pinned lists counted rows outside the actual end window and missed needed rows above
  the mounted set. [End-window accounting][r-pinned].

Octara should adopt pending-work ownership and target-relative progress, not copy Rune's exact
frame/backoff/overscan constants. The newest head also fixes revealing a search match in an
offscreen paragraph on the same frame: lazily materialized geometry must still satisfy a real
selection/reveal request. [Latest paragraph fix][r-head].

## Exact: identity, environment and branch state are distinct

### Retained rows receive a current execution environment

`RegionInst::update_item` retains row slots, installs the current item/index in its Frame, and
uses changed fields/position dependencies to update the row. `switch` preserves an unchanged
arm and updates its bound value; changing arms destroys the previous subtree and initializes
the new arm's slots. [Region implementation][e-region].

**Lesson:** matching a key does not authorize keeping an old input capture. Octara needs a
region-owned current row input and an explicit content lifetime boundary. Keep row-level state
on replacement; replace a child scope when its enum branch or captured resource changes.

**Cost and boundary:** Exact can analyze frames and changed fields because it owns a language,
Plan and execution environment. Octara cannot infer arbitrary Rust closure captures or field
dependencies. Use Signals, explicit read closures and keyed content; do not import the VM to
imitate Exact's implementation.

### Optimize against a complete semantic baseline

The rekey fast path can reuse object identity only when key dependencies and any observed index
remain valid. Its tests compare optimized and forced complete recomputation through insertion,
removal, replacement under equal keys and reorder. Changed row content invalidates relevant
measurement while unaffected row heights remain reusable. [Rekey implementation][e-rekey],
[equivalence tests][e-rekey-tests].

**Lesson:** define complete replacement first, then specialize incremental updates. A reference
or key staying equal is not enough to conclude that all inputs, measurements and callbacks are
valid. For Octara, snapshot and incremental input must satisfy the same observable row contract.

### Bounded residency is different from physical node reuse

The virtual Collection is driven by actual scrollport feedback and explicitly avoids a
historical row-state cache. Extra `keep-mounted` keys are bounded; ownership tests check
interaction/focus pins and stale feedback. [Collection owner][e-collection], [keep policy][e-keep],
[ownership tests][e-ownership].

Its optional physical row reuse is a separate mechanism. Rebinding retired nodes to another
item must behave like fresh mounting, including renewed identities/measurement and clearing
host interaction state. Reuse excludes shapes such as editors and native controls, requires host
support, and macOS disables it for VoiceOver/Switch Control. This is substantially more involved
than preserving a row during reorder. [Reuse rules][e-reuse], [macOS renewal][e-renew].

**Lesson:** retain logical identity only for the same entity. Octara does not need a physical
node pool in this redesign. If a measured bottleneck later justifies one, it must preserve fresh
target generations and fully reset host state; reuse is not a reason to broaden key semantics.

### Retirement includes every owned table

Exact's November 10 resource-list audit documented a lifecycle defect: departed members stayed
in resource tables. Alternating two two-item lists 6,000 times retained 12,001 resource rows;
even unrelated commits paid for that history. The issue is closed at the inspected head: retired
member slots without in-flight requests can be safely reused. These are upstream-reported
measurements, not measurements run by us. [Resource-member growth report][e-growth].

A separate closed issue found writes attached to a collection container while refresh completion
occurred on its members. Overlay lookup and write retirement therefore missed each other.
The resolution gives members their own overlay/retirement handling. [Member-write report][e-writes].

**Lesson:** cleanup ownership must agree across row state, requests, subscriptions, cached
measurements and inspection metadata. Test repeated removal/re-addition with a small live set;
memory/work should track current residents and bounded history, not session duration. This does
not justify bringing Exact's resource/write protocol into Octara's Signal primitive.

## Decisions for Octara

| Adopt | Deliberately leave out |
| --- | --- |
| Ordinary Rust model data plus a pure collection derivation | Mandatory Vec-to-writable-list mirroring |
| Stable row identity and replaceable current input | Capturing an upstream item forever merely because its key survives |
| Child content scope for branch/resource replacement | Changing a business ID just to force new payload to render |
| One row coordinator, viewport policy for virtual lists | Parallel reconciliation implementations and unbounded offscreen scopes |
| Complete recomputation as an oracle; focused callback/lifetime tests | Assuming visual equivalence proves handler freshness |
| Direct causal diagnostics and pending-work evidence | A VM, manual memo revisions, physical node pooling or a general history engine |

These choices address observed Octara failures. Exact's and Rune's internal complexity is
evidence of tradeoffs, not a requirement to reproduce their architectures wholesale.

[r-tracked]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/crates/rune/src/tracked.rs
[r-memo]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/crates/rune/src/memo.rs
[r-intro]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/changelog/unreleased/2026-11-12-tracked-region.md
[r-boundary]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/changelog/unreleased/2026-11-12-region-reads.md
[r-rows]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/crates/rune/src/el/virtual_rows.rs
[r-resume]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/changelog/unreleased/2026-11-12-virtual-fill-resumes.md
[r-disjoint]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/changelog/unreleased/2026-11-12-virtual-fill-disjoint-count.md
[r-pinned]: https://github.com/anomalyco/rune/blob/631e8a67cb3c40225efa3f6c4f57b80e7b18f190/changelog/unreleased/2026-11-12-virtual-fill-pinned-count.md
[r-head]: https://github.com/anomalyco/rune/commit/631e8a67cb3c40225efa3f6c4f57b80e7b18f190
[e-region]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/region.rs
[e-rekey]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/rekey.rs
[e-rekey-tests]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/rekey_tests.rs
[e-collection]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/mod.rs
[e-keep]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/keep.rs
[e-ownership]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/ownership_tests.rs
[e-reuse]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/runner/src/instance/collection/reuse.rs
[e-renew]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/host/apple/Sources/ExactKit/Mac/RenewMac.swift
[e-growth]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/issues/closed/20261110-resource-over-a-list-rows-grow-without-bound.md
[e-writes]: https://github.com/expo/exact/blob/5491ab299cc6ed94857cacfd54fee093550a70c7/issues/closed/20261110-writes-over-a-list-never-retire.md
