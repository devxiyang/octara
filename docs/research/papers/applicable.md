# Applicable Algorithms and Mechanisms

Status: review (2026-11-11). After the kernel rewrite and virtualization were completed, the papers and engineering approaches
collected in this directory were re-examined, and the algorithms and mechanisms usable in Octara are listed with their source,
their use in Octara, current status and priority.

## Already adopted

| Mechanism | Source | In Octara | Verification |
| --- | --- | --- | --- |
| Demand-driven, push marking + pull evaluation, equality-based cutoff | Adapton; Salsa's early cutoff | The basic algorithm of the reactive graph | [Reactive kernel](../../design/reactive-core.md) |
| First-class names determine identity, name ambiguity is detected | Nominal Adapton | Named state, `component_keyed`, `AMBIGUOUS_COMPONENT` | Error injection I7 |
| Collections as traceable data types; changes are first-class values; operators maintained incrementally by change | Traceable Data Types; ILC; DBSP | `List`, `Filter`, change log, `each` | [Reactive kernel](../../design/reactive-core.md) section 8 |
| Effects separated from tracking | Elm's Cmd / Sub; reactive programming without functions | `watch`: the source is tracked, the callback is not | Error injection I4, I8 |
| **Consistency check against computing from scratch** | The consistency result of self-adjusting computation; Nominal Adapton's "from-scratch consistency" | In tests, after every event the whole tree is rebuilt from the current state and compared character by character with the incremental result | Implemented this round; in mutation testing the first erroneous event is located |
| Weighted order-statistic tree (sum tree) | GPUI, Rune (engineering approaches) | Virtualization: mutual lookup between scroll offset and row number in O(log n) | Implemented this round |
| Composable incremental operators: sort, group | DBSP; ILC | Stage chain: `filter`, `sort_by`, `group_by` can be chained; "move" changes preserve row state | [Reactive kernel](../../design/reactive-core.md) |
| Flattening (flat_map) | DBSP | `Groups::flatten`: group headers and members expand into one stage, so grouped lists can be virtualized | [Reactive kernel](../../design/reactive-core.md) |
| Change structure for strings | ILC | `LiveText` edit log, `text(live)`, kernel `append_text`: streaming tokens write only the token itself | [Reactive kernel](../../design/reactive-core.md) |

## Applicable (by priority)

### High priority

**1. Incremental operator library: sort, group, map, join** (DBSP) — sort, group and flatten are done, see [Reactive kernel](../../design/reactive-core.md); map and join remain

- DBSP's conclusion is that most operators have incremental versions whose cost is proportional to the size of the change, and
  the incremental version of a composed query is the composition of the incremental versions of its parts.
- Octara currently has only `filter`. The other most common needs in UI can be implemented in the same way:
  - `sort_by(key)`: maintained by sort key with an order-statistic tree; adding or modifying an item is O(log n);
  - `group_by(key)` plus per-group aggregation (count, sum): for example chat history grouped by date, lists with group headers;
  - `map`: derive a new item type, recomputing only the changed items;
  - `join`: associate two collections by key, for example tasks and tags.
- DBSP's "weighted sets" can be introduced (each change carries a weight of +1 / −1), so that insert, delete and modify unify
  into a single kind of change and operator implementation is simpler.

**2. Change structure for strings** (ILC) — done, see [Reactive kernel](../../design/reactive-core.md); remaining work is incremental per-paragraph layout in a real text stack

- ILC defines a "change" as a first-class structure corresponding to the value type. Currently, for each token Chat writes the
  whole body as a new value: the binding gets the entire string again, and the kernel resets the entire text.
- Define `Append` / `Splice` changes for strings and let them travel along the binding to the kernel: the kernel only appends
  text, and text layout reprocesses only the affected paragraphs (corresponding to the two-level line cache used by Rune).
- The benefit is direct for Agent-type applications, where streaming output is the hot path.

**3. Commands, subscriptions and cancellable resources** (Elm's Cmd / Sub; algebraic effects; Salsa's cancellation)

- Commands: I/O is handed to a replaceable handler as typed commands, and replaced by fakes or recorded results during testing
  and replay (design v2 section 5).
- Subscriptions: external event sources (timers, WebSocket, system notifications) are described declaratively, for example
  "fire once per second while `active` is true", with the runtime responsible for starting and stopping them;
  the clock is controlled by the runtime and the Agent can advance time (consistent with Exact's "the clock belongs to the Agent").
- Resources: when an input changes, in-flight requests are cancelled, the same as Salsa cancelling in-flight queries when an
  input changes, avoiding races.

### Medium priority

**4. Typed actions** (Bonsai's incremental state machines; Elm's messages; Exact's action)

- A component's state changes are described by a finite set of typed actions, not arbitrary closures.
- Benefits: the Inspector can list the actions each component accepts; an Agent can invoke actions directly, not just simulate
  clicks; replay only needs to record the sequence of actions.
- It is the same idea as the Native Action Schema (typed, authorizable system operations), just applied inside the application.

**5. On-demand suspension of hidden UI** (Adapton: only the outputs that are needed are recomputed)

- Virtualization already applies this idea to long lists. It can be generalized to all invisible UI: tabs switched away from,
  collapsed panels.
- These subtrees keep their state and nodes, but the bindings inside them are paused and only record "stale"; they run once when
  visible again. Unlike `show`, switching does not need to destroy and rebuild.

**6. Durability and bounded caches** (Salsa)

- Durability: rarely changing state (theme, configuration, localization) is graded, and when it changes whole regions skip
  validation (design v2 section 6).
- Bounded cache: Salsa applies LRU to memoized results and reclaims values no longer used after a number of revisions. This can
  be used for derived data cached by key in virtualized lists (for example the Markdown render result of each message): the
  cache is kept when a row scrolls out of the window and reclaimed once it exceeds the limit.

**7. Derived results reused by name** (Nominal Adapton)

- Example from the paper: after inserting an element in the middle of a list, the computation results before the insertion
  point can still be reused by name.
- Currently, when a row is filtered out or scrolls out of the window, its scope is destroyed and the Memos inside the row are
  lost. Combined with the cache from item 6, derived results can be stored by "list key + name" and reused directly when the
  row reappears.

**8. State migration on hot reload** (dynamic software updating: Kitsune, Rubah)

- When a state type changes, convert the old value with serde or a user-provided migration function; large state uses lazy
  conversion (design v2 section 7).

**9. Runnable incomplete programs** (Hazel: programs with holes; Livelits)

- Provide a view placeholder, for example `hole("todo list")`: it renders a visible placeholder node and reports a diagnostic,
  instead of a compile failure or panic. A half-written UI from an Agent can also be run and inspected, so the
  Generate → Run → Inspect loop can start earlier.
- Livelits and Rune's live literals are the same direction: adjusting a parameter needs no recompilation.

**10. Formal semantics of the reactive kernel** (Remus's small-step semantics; denotational semantics of UI description languages)

- Write an operational semantics for update order, batching and scope destruction, and implement a slow but obviously correct
  reference interpreter.
- Use it for differential testing of the fast implementation. This is the reactive-layer version of the "from-scratch
  consistency check", and can cover scheduling problems that tree comparison cannot find, such as `watch` callbacks writing.

### Low priority

**11. Ahead-of-time ordering of static subgraphs** (Remus; Solid 2.0's height heap)

- Most bindings have static dependencies. Topological heights can be computed ahead of time, avoiding sorting the queue on every
  flush. This is a performance optimization, and no bottleneck has been measured at present.

**12. Adaptive granularity** (the caution from self-adjusting computation: change propagation is not always faster than recomputation)

- Use runtime counters to find Memos whose recomputation cost is lower than the tracking overhead, and suggest merging or
  removing them. It can be made a performance lint.

**13. Multi-threaded and cross-process propagation** (REScala: thread-safe reactive programming, SID-UP distributed propagation)

- Only needed when a background thread must write reactive state directly, or when adopting a split-process development mode
  with "rendering on the device, logic on the development machine". In that case SID-UP can guarantee no intermediate states
  across processes without centralized knowledge of the dependency graph.

## Suggested next steps

1. Incremental operator library, starting with `sort_by` and `group_by` (item 1);
2. Change structure for strings, connecting "streaming token → kernel append text" (item 2);
3. Commands, subscriptions and cancellable resources (item 3), together with clock control for the Agent.
