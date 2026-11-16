# Architecture review: collection regions

Reviewed 2026-11-16 against the proposed [collection architecture](collection-regions.md),
`reactive/graph.rs`, `reactive/list.rs`, `view/list.rs`, Chat callers and the
[runtime contract](runtime-contract.md). This is a design/code inspection, not implementation
validation. No runtime code was changed or tests run for this review.

Verdict: the ownership direction addresses the demonstrated faults, but the original proposal
overstated its completeness and prescribed a scheduling rewrite without sufficient evidence.
The corrections below are incorporated in the proposal. Prototype gates remain necessary.

The later [React/Solid comparison](../research/react-solid-boundaries.md) further removes the
requirement to turn every ordinary Vec into a public ReadList. Direct keyed structural bindings
still have to satisfy all correctness gates below; reducing API layers does not resolve them
automatically.

## Findings and corrections

### P1: row reads could conflict with immediate read-after-write

**Original ambiguity:** a coordinator updates the row's stable input during reconciliation, yet
the runtime says model writes are readable immediately and structural work waits until callback
exit. A handler selecting list B and reading its same-key row could get list A's payload. Making
the read flush UI would instead execute disposal/cleanup inside the handler.

**Correction:** a row input purely resolves the currently selected collection and key. Source
selection and item/membership dependencies participate in that read. It does not mount or dispose
anything. Missing/invalid membership fails the read; it never silently returns old data. Mounted
identity remains governed by coordinator-accepted membership. This is an explicit distinction
between data freshness and structural publication, not two competing models.

**Required proof:** inside one callback, select B, read the current item, and verify no cleanup
or row construction ran. Then verify old-source writes no longer affect it. Cover removal and
invalid membership, normal row retirement, and a source switch whose values initially compare
equal. The public API must support the selected bounds without a new proxy/lens system.

### P1: rejected data did not revoke old UI authority

**Original ambiguity:** duplicate keys reject a new snapshot but keep the prior presentation.
An old Delete button still has a valid mounted target and can address ambiguous current business
data. Stale-target checking alone validates node identity, not the current key-to-item mapping.

**Correction:** invalid computed input is unreadable. On reconciliation, retire that region's
rows and interactions, expose its diagnostic and keep only the coordinator waiting for valid
input. Recovery mounts fresh rows/targets. An insertion rejected before mutating a valid
incremental source does not invalidate the existing region. Before later dispatch, reconcile
pending ancestor structure/validity; do not invoke old handlers first.

**Required proof:** duplicate a displayed key, attempt semantic/Agent actions using its former
target, and verify no callback ran. Correct the data and verify fresh targets. Exercise focused
input, composition/drag retirement, and unrelated regions continuing to work. This is deterministic
handling of an invalid collection, not rollback of arbitrary Rust mutation.

Validation failure must remain an observable internal result with active dependency tracking;
throwing from the producer would retire it under the current graph failure path and prevent the
promised recovery. User callback panics remain distinct from duplicate-key validation.

### P2: removing Derived did not specify who owns computation

**Original ambiguity:** returning Rc-backed ReadList removes the public wrapper but does not
replace its scope owner. A retained handle could keep evaluating closures after their captured
scope died, or a graph/cache strong cycle could retain everything. A promised frozen result is
also ambiguous for lazy unevaluated data and live resources such as TextBuffer.

**Correction:** the construction scope owns execution. Retirement disconnects dependencies and
disables the producer before captured state is disposed; clones do not extend that lifetime.
Create intentionally shared producers in the App root. Retired reads/composition reject; cached
storage can remain until final release. Do not promise a generically frozen result. Release must
still use the creating runtime even from outside App entry or inside another App.

**Required proof:** keep an upstream source alive in the App root, create a derived producer in
a window, retain a read handle, close the window and mutate the upstream. No retired computation
may run. Read/composition rejects, final release restores node counts, and dropping the App still
releases graph values. Specify acyclic ownership in the prototype: the scope owns the producer
registration, cache handles own retained storage, and backreferences to runtime/registration must
not create strong cycles. Existing final-drop tests cover only part of this contract.

### P2: mandatory Observer replacement exceeded the evidence

**Original prescription:** rewrite all filter/sort/group/join propagation into the graph as part
of fixing source-switch and content-capture faults. The current Emitter propagates synchronously;
it is not another delayed UI event loop. The demonstrated failures first occur at the view input
and content boundaries. A new engine would introduce cursor, before-value, invalidation and
ownership work across operators whose algorithms are already useful.

**Correction:** require one freshness, purity, retirement and publication contract. Snapshot
derivation belongs in the graph. Whether existing incremental evaluation should be replaced is
a private implementation choice to settle with a mixed-chain/join prototype. Do not preserve
an old engine for compatibility, but do not remove a correct specialized evaluator for visual
uniformity either. Never add the withdrawn watch mirror as an interim public API.

**Required proof:** compare consistent reads through two join operands, long-chain propagation,
scope retirement, and changed-item work. Verify ancestor structural work precedes descendant
properties, including structural work queued during an ongoing batch. A single sort of the
existing Binding queue is not proof of correct ordering. Choose the smallest implementation
that satisfies these tests without O(n) work for an ordinary incremental edit.

### P2: content replacement could reset unrelated local state

**Original ambiguity:** keying a whole part subtree by its TextBuffer identity also destroys a
Reasoning/Tool disclosure's local open state when only its buffer changes.

**Correction:** put disclosure state outside the buffer-specific subtree. A different part
variant can reset variant-local state; replacing its buffer should only replace the resource
consumer. Persistence across virtual-row eviction is a separate model-state decision.

**Required proof:** open a tool/reasoning part, replace its buffer under the same part/type,
verify it stays open and displays new content, then write the old buffer and verify no change.
Switch variants and verify obsolete handlers/tasks retire. Appending to the same buffer must
neither rebuild the content scope nor rederive the parts collection.

### P1: fresh input does not repair a captured write destination

**Original overclaim:** the same-key source-switch table promised that every event would use the
new input. Read rebinding only fixes reads. A callback that captured writable list A can still
remove from A when the retained row now displays B. Similarly, key `1` in one conversation may
be a different entity from key `1` in another; equal keys alone cannot decide reuse semantics.

**Correction:** keys identify entities within a region/domain. Change the enclosing keyed
boundary when changing domains, or use composite business keys. Retained handlers resolve their
action owner from current application state instead of capturing a replaceable source. No
automatic closure rewriting or writable row proxy is promised.

**Required proof:** switch between sources, invoke the row's actual action and assert which
model changed. Also switch between conversations that both contain the same local key and
verify state/targets do not leak between them. A text assertion or screenshot is insufficient.

## What is necessary, and what is not

| Necessary contract | Keep implementation small |
| --- | --- |
| Ordinary Rust values can feed keyed views | No mandatory writable mirror, Store DSL or field proxy |
| Same key preserves row identity while input changes | One private current-input computation, not public generation management |
| Different content/resources retire their consumers | Reuse keyed child scopes and ordinary bindings |
| Row ownership is shared by normal/virtual lists | Keep viewport and measurement policy separate from reconciliation |
| Current reads and correct structural ordering | Use existing dependency/scope concepts before introducing scheduler types |
| Bounded work/storage and correct cleanup | Bound actual caches/logs; do not start a general physical-node pooling project |
| Agents receive authoritative results and failure reasons | Extend existing inspection, not a second state/history engine |

The prototype is the next deliverable: a small executable contract for source switching, missing
keys, invalid input, ownership and a mixed collection chain. It is not an app-specific patch or a
promise to rewrite unrelated core modules. After that evidence, finalize public signatures,
implement the shared coordinator and migrate Chat; remove superseded code in the same slices.
