# Primitive reduction audit

Status: base implementation committed as `af6fa20`, 2026-11-16. The four deletions
in the first table and explicit prelude exports are implemented through the
[shared collection boundary](collection-regions.md). Specialized grouping types remain in
`reactive::list`; macro/representation support types remain accessible through their explicit
modules, outside the prelude. This audit records the rationale and remaining encapsulation
options, not a claim that every option below was implemented.

The criterion is fewer independent mechanisms and clearer application code, not fewer exported
function names. Separate removing an abstraction, removing a default import, and specializing a
capability. A scope responsibility cannot disappear merely because its wrapper is deleted.

## Remove with a concrete replacement

| Candidate | Evidence | Replacement / gate |
| --- | --- | --- |
| Public `Derived<K, T>` wrapper | `reactive/list.rs`: a scoped holder/version plus forwarding read/composition methods | Operators return ReadList; producer registration remains scope-owned. Prove retirement and cross-runtime final release before removal |
| Chat parts mirror | `apps/chat/src/ui.rs::answer`: Vec -> watch -> StateList | Direct keyed Vec input; preserve streaming TextBuffer path and fix content replacement |
| Kit option mirror | `octara-kit/src/navigation.rs::retained_options`: memo -> watch -> StateList -> sort by synthetic order | Direct keyed Vec input for Tabs/Segmented; retain current order, target, focus and duplicate-key semantics or explicitly migrate them |
| Separate normal/virtual row reconciliation | `view/list.rs`: both manage row scopes and source-switch reuse | One private keyed-row implementation; virtualization keeps residency/measurement policy |

The two mirror helpers are application/component code, not public primitives. Their deletion is
nevertheless a larger simplification than deleting a convenience constructor. These removals
must land with the replacement semantics; deleting them first would break working UI.

## Reduce the default surface, retain useful capabilities

- `Groups`, `GroupInfo`, `FlatKey`, `FlatItem` are specialized collection data, not part of the
  small Signals vocabulary. Keep them in their explicit collection module rather than making
  every one a default prelude import. Chat's grouped thread sidebar and the grouped virtual
  contacts example use them; deleting the capability would push equivalent logic into apps.
- `StateList`/`ReadList` remain explicit incremental tools. Ordinary Vec rendering must not
  require them. Do not replace large-list incremental updates with full Vec scans for uniformity.
- `Resource`/`Load` provide keyed asynchronous loading, cancellation and refetch, retaining the
  previous value during reload/error. They are a higher-level task utility, not another Signal
  primitive. Keep the behavior; removing it would distribute the lifecycle protocol to callers.
- `paced`, `clock` and `interval` are presentation/time conveniences. They do not require another
  reactive engine. In particular, paced integrates pending work, motion settling and reduced
  motion; its implementation cannot just be moved to the kit without preserving that contract.

Changing prelude membership is an import/API decision, not deleting the underlying feature or
proof that runtime complexity decreased. Do not create a new crate solely to move these names.

## Hide implementation hooks without breaking Rust API boundaries

The prelude currently glob-reexports `view::*`. This includes publicly reachable implementation
names as well as intentional application APIs. Prefer an explicit curated list.

- `Building`/`enter_component` are already doc-hidden macro plumbing. Generated code in another
  crate must still be able to call its support entry. Move it behind a clearly internal public
  macro-support path if useful; simply making it crate-private would break the macro.
- `Mount` is opaque but appears in public `View::Dyn`. Hiding it requires encapsulating View's
  representation and migrating callers; it is not a one-line visibility deletion.
- `Prop` is already doc-hidden, but appears in the public IntoProp conversion signature.
  `TextContent` similarly participates in IntoText. Audit custom conversions before changing
  their visibility. Remove unnecessary default imports without claiming the types disappeared.
- Retain `IntoElement`, `IntoProp` and `IntoText` where they permit ergonomic public conversion
  and kit composition. Replacing their functionality with repeated per-component adapters is
  not a simplification.

## Do not merge different identity mechanisms by name

`#[component]` records source/component attribution; its macro explicitly creates no state
scope. `component` and `component_keyed` create mounted scopes; `keyed` replaces a child scope
when a reactive key changes. They are different operations despite similar names.

Their naming/documentation can be made clearer. Do not delete an explicit scope constructor
until an equally small replacement preserves initialization, cleanup, named-state reload and
kit state isolation. Automatically turning every source-attribution macro into a new scope
would change lifecycle and identity, not merely reduce syntax.

## Keep the small semantic core

- `Signal` / `ReadSignal`: write authority versus read authority.
- `state`, `memo`, `watch`: owned state, pure derivation and effects.
- `untrack`: intentional sampling independent of write permission; it must not be used to
  conceal live inputs that should be bound.
- Scope cleanup and scoped tasks; `provide` / `use_context` for scoped shared dependencies.
- `show`, `keyed`, `each` and virtual residency, with common internal lifecycle handling.
- `TextBuffer` and text input/IME/focus semantics where ordinary string snapshots are insufficient.
- Component `Behavior` versus application event callbacks: the composition mechanism prevents
  application callbacks from replacing built-in control behavior.

`show_else` already shares the private switch implementation with `show` and `keyed`. Removing
the spelling saves no engine and can make branches less clear. Likewise row/col/spacer and
element conversion conveniences should be judged by application clarity rather than name count.

The kit's `Model<T>` is already outside the core. It accepts both a Signal and a getter/setter
pair, allowing application validation or transformation. Requiring a writable Signal everywhere
would remove this control, so this audit does not recommend deleting Model.

Implementation evidence: [collection primitive validation](collection-primitives-validation.md).

## Order of work

First prove the direct Vec/keyed-row replacement. Then delete Derived, the identified mirrors
and duplicate row reconciliation together with their migrated callers. Curate prelude exports
and macro plumbing separately. Keep incremental collection and async capabilities unless a
concrete replacement reduces both their implementation and caller burden.

The proposed universal collection-constructor layer and new public coordinator/input/Region
types were already rejected in the design. They are not implemented code to count as deletions.
