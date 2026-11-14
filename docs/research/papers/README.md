# Papers and Frontier Approaches: Reference

Status: reference material (compiled 2026-11-11). This directory collects papers and engineering approaches related to
Octara's state model, one file per topic. Each entry gives the source, the key points, and what it means for Octara.

**Note on sources:** this round is based on paper abstracts, conference pages and project documentation; **the full
papers were not read one by one**. Check the original before citing a specific conclusion. Items marked "secondhand"
come from third-party summaries. Paper PDFs are not stored in the repository; only links are kept.

## Topics

| File | Topic | Octara problem addressed |
| --- | --- | --- |
| [exact-runtime-simplicity.md](../exact-runtime-simplicity.md) | Exact's task-oriented Agent interface, local identity, transactions and native boundaries | Which parts of the proposed observation/convergence architecture to simplify or defer |
| [mature-runtime-identities.md](../mature-runtime-identities.md) | Fixed-source comparison of Chromium, Flutter/Dart, Compose and GPUI identities and completion stages | Which identity fields need public guarantees; limits of app-wide revision and layout convergence |
| [runtime-foundations.md](../runtime-foundations.md) | Original sources on causal order, operation boundaries, FRP, superdense time, and Unix/platform failure modes | Baseline for the proposed runtime and Agent context contracts; includes its own source boundary |
| [incremental-computation.md](incremental-computation.md) | Incremental computation: Self-adjusting computation, Adapton, Nominal Adapton, Salsa, Incremental / Bonsai | Theoretical basis of the reactive kernel; node identity and naming; durability |
| [incremental-collections.md](incremental-collections.md) | Incremental maintenance of collections: DBSP, the incremental λ-calculus | Aggregation and filtering over `List` are still O(n) |
| [effects.md](effects.md) | Modeling effects: the evolution of Elm, algebraic effects, reactive languages without functions | `useEffect`-style problems; I8 (reading under the wrong observer) |
| [runtime-update.md](runtime-update.md) | Updating at runtime: dynamic software updating, Hazel live programming | State migration during hot reload |
| [concurrency-and-semantics.md](concurrency-and-semantics.md) | Concurrency and formalization: REScala, Remus, UI description language semantics | Multi-threaded updates, determinism, verifiability |
| [applicable.md](applicable.md) | **Review: applicable algorithms and mechanisms**, ordered by priority, marked adopted or to be adopted | Roadmap |

## Summary of conclusions

- Related incremental and reactive models motivate Octara's choices; they neither establish that Signals have no
  alternatives nor prove the correctness of Octara's implementation.
- Three improvements can be layered directly on top of Signals:
  1. **Incremental collections** (DBSP, ILC): `List` emits incremental changes, so the cost of derived collections and
     aggregates is proportional to the size of the change;
  2. **Explicitly named identity** (Nominal Adapton): the identity of components and state is determined by names, and
     name ambiguity is checked;
  3. **Effects as data** (Elm's Cmd / Sub, algebraic effects): effects are handed to the runtime as typed commands, which
     serves deterministic replay and the Inspector.
- Engineering approaches worth borrowing: Salsa's durability levels (validated at scale in Rust), and state transformation
  functions from dynamic software updating.

The design derived from this: [reactive kernel](../../design/reactive-core.md).

Related documents: [Signals implementation survey](../signals.md), [UI primitives and component layering](../../design/ui-primitives.md).
