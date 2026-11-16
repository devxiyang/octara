# React and Solid: smaller structural primitives

Reviewed 2026-11-16 using official documentation and tagged source. This refines the proposed
[collection architecture](../design/collection-regions.md); no runtime implementation or upstream
test execution is claimed.

## Versions and evidence

- React's official latest release resolves to **19.3.1**, released October 14, 2026. Its current
  documentation describes the 19.3 line. [Release][react-release], [versions][react-versions].
- Solid's current coordinated preview is **2.0.0-rc.22**, released November 15. Its tagged package
  manifest agrees with the release, and the v2 site explicitly labels its documentation RC.
  Do not describe these APIs as stable. [Release][solid-release], [manifest][solid-manifest],
  [RC status][solid-overview].
- The current stable-ecosystem Solid docs describe 1.x For/mapArray/store APIs. They are used
  separately below. GitHub's latest-release redirect still selected v1.9.0 during this review;
  that alone does not establish the latest npm 1.x patch. This review makes no such patch claim.

## React: identity does not freeze inputs

State preservation follows the component's place, type and key within its parent. Changing props
does not inherently reset state. At the inspected 19.3.1 source, `updateElement` reuses a compatible
Fiber with the incoming props rather than retaining its old props. [Identity guide][react-identity],
[tagged reconciler][react-reconciler].

For Octara, preserve the useful separation: domain/row identity controls retained state; current
read inputs control values; a changed content kind/resource can require a child replacement.
This does not require Fiber, hook call-order state, component rerenders or a virtual DOM. Rust
closures do not acquire new captures automatically; read accessors and explicit boundaries still
matter.

React's guidance also rejects storing renderable derived data in state and synchronizing it with
an Effect. Derive it directly and memoize expensive work when useful. This supports removing
Chat's parts mirror, without introducing a mandatory public collection abstraction in its place.
[Effect guidance][react-effects].

## React: distinguish calculation, commit and visibility

React separates pure rendering from applying host mutations. Its state variables represent a
render snapshot, so a setter does not change the value already captured by that render's event
handler. [Render/commit][react-commit], [snapshot semantics][react-snapshot].

Adopt the separation of data calculation and host application, not React's snapshot variable
semantics. Octara already promises immediately readable model writes with structure settling
at the outer callback boundary. No speculative Rust execution or rollback is needed.

Activity preserves hidden UI/state but cleans up Effects and recreates them when visible; hidden
children may still render at lower priority. It is an explicit visibility/lifecycle capability,
not a bounded virtualization cache. [Activity][react-activity]. A future Octara tab-preservation
primitive may benefit from this distinction. It is not required for the current row redesign,
and retaining every offscreen row would still be unbounded.

`useEffectEvent` reads latest committed values for logic invoked from Effects. It is constrained
to that use, not a universal event callback or a way to erase required dependencies. [Reference][react-effect-event].
Octara's ordinary handlers can read Signals when invoked; importing another public hook solely
to repair stale captures is unnecessary.

## Solid: explicit key plus current item accessor

In the 1.x documentation, For/mapArray cache by item value identity. Replacing an object with a
new object of the same `id` is not itself a guarantee of preserving that row. Store `reconcile`
is a separate mechanism that can match incoming items by a field. [For][solid1-for],
[mapArray][solid1-map], [store reconcile][solid1-reconcile].

The inspected **2.0 RC** provides a directly relevant third mode:

| For keying mode | Item supplied to row | Identity |
| --- | --- | --- |
| default / true | Raw item | Item identity |
| false | Item accessor | Position |
| key extraction function | Item and index accessors | Extracted business key |

The third mode keeps the row while exposing replacement values through the accessor. For
delegates to `mapArray`; its tagged implementation uses an owned computation, row owners and
internal row Signals. Applications can pass an ordinary array directly to For. [V2 For][solid2-for],
[tagged component][solid2-flow], [tagged mapping implementation][solid2-map].

This supports a smaller Octara API: let `each` accept an ordinary reactive Vec and an explicit
key selector, supplying a read-only current item to the row. A private snapshot/index cache is
reasonable; a public ReadList producer is not mandatory for this route. StateList/ReadList can
remain an opt-in incremental route to the same mounted-row machinery.

Do not copy all three modes automatically. Rust has no universal JavaScript object identity;
explicit business keys remain the default design. Positional mapping needs a separate concrete
use case, and is wrong for reorderable messages.

## Solid: ownership and dependency tracking are different responsibilities

For's mapping owns row lifetimes; nested computations subscribe to values. Show independently
illustrates preservation versus replacement: the non-keyed function child receives a narrowed
accessor, while the keyed form receives a value and replaces content when identity changes.
Reading a narrowed value after its valid branch has ended is an error, not a retained default.
[Stable Show contract][solid1-show], [RC implementation][solid2-flow].

Octara can use its existing scope tree for lifetime and reactive graph for dependencies. Normal
bindings, keyed branches and list rows should use those common mechanisms. There is no need for
an extra public owner, item-input object or general-purpose Region API for each conceptual box.
Region remains a useful internal word for bindings that own child scopes.

The RC mapper is not trivial internally: it stages new mappings and defers removals so a pending
async read cannot corrupt its previous mapping. Its source documents recovery from an aborted
mapping pass. [Mapping implementation][solid2-map]. Learn the need to define failure boundaries;
do not import async speculative mapping or claim rollback for Octara's arbitrary Rust callbacks.

## Solid 2 scheduling is a deliberate non-adoption

Solid 2 stages ordinary writes and commits them on a microtask; reading immediately after a
setter returns the last committed value. Its migration guide introduces flush for explicit
imperative boundaries and warns against replacing every former batch with flush. This is a
semantic change from 1.x, not just renamed APIs. [Migration guide][solid2-migration].

Keep Octara's immediate `set/update` value semantics. Staging arbitrary in-place Rust mutation
would require additional ownership/copying or a different public mutation contract. Native
input/IME and callback reads would also need requalification. These costs are not justified by
the list identity problem. Borrow Solid 2's keyed-accessor API separately from its scheduling.

The new createEffect separates a tracked compute from an untracked effect callback owned by
the surrounding scope. [Effect reference][solid2-effect]. Octara already has this separation
in watch's source/callback. Keep it; ordinary derived values should still use pure computations.

## Recommended smaller architecture

One dependency graph answers **what must update**. One scope tree answers **what must end**.
View bindings consume ordinary read inputs:

- Property bindings update an existing node.
- `show`/`keyed` select owned child content.
- `each` matches keyed child scopes and exposes current item reads.
- `virtual_each` adds bounded residency and measurement to the same keyed implementation.

Ordinary arrays/Vecs do not require a reusable collection object or a second writable state.
Shared expensive projections can use ordinary memoized data. Incremental lists and TextBuffer
remain specialized storage/update paths with measurable benefits, rather than mandatory steps
for all application data.

This is an Octara recommendation inferred from the sources, not a claim that React and Solid
have identical semantics. The six review gates from the previous proposal still apply, especially
write-after-source-switch actions, same-handler reads, retirement and invalid input. Simplifying
the public model does not make those obligations disappear.

[react-release]: https://github.com/react/react/releases/tag/v19.3.1
[react-versions]: https://react.dev/versions
[react-identity]: https://react.dev/learn/preserving-and-resetting-state
[react-reconciler]: https://github.com/react/react/blob/v19.3.1/packages/react-reconciler/src/ReactChildFiber.js#L580
[react-effects]: https://react.dev/learn/you-might-not-need-an-effect
[react-commit]: https://react.dev/learn/render-and-commit
[react-snapshot]: https://react.dev/learn/state-as-a-snapshot
[react-activity]: https://react.dev/reference/react/Activity
[react-effect-event]: https://react.dev/reference/react/useEffectEvent
[solid-release]: https://github.com/solidjs/solid/releases/tag/solid-js%402.0.0-rc.22
[solid-manifest]: https://github.com/solidjs/solid/blob/solid-js%402.0.0-rc.22/packages/solid/package.json
[solid-overview]: https://v2.solidjs.com/
[solid1-for]: https://docs.solidjs.com/reference/components/for
[solid1-map]: https://docs.solidjs.com/reference/reactive-utilities/map-array
[solid1-reconcile]: https://docs.solidjs.com/reference/store-utilities/reconcile
[solid1-show]: https://docs.solidjs.com/reference/components/show
[solid2-for]: https://v2.solidjs.com/reference/solid-js/components-jsx/for/
[solid2-flow]: https://github.com/solidjs/solid/blob/solid-js%402.0.0-rc.22/packages/solid/src/client/flow.ts
[solid2-map]: https://github.com/solidjs/solid/blob/solid-js%402.0.0-rc.22/packages/signals/src/map.ts
[solid2-migration]: https://v2.solidjs.com/migration/from-solid-1/
[solid2-effect]: https://v2.solidjs.com/reference/solid-js/reactivity/create-effect/
