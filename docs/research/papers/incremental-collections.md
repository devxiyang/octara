# Incremental Maintenance of Collections

This addresses a measured bottleneck in the prototype: after introducing `List<K, T>`, updating a single item dropped to O(1), but
aggregate Memos (`count_where`, `any`) and filtered views (`keys_where`) still traverse all entries every time (for the problem
and results see sections 5 and 8 of the [reactive kernel](../../design/reactive-core.md)).

## DBSP

- **Source:** Mihai Budiu, Tej Chajed, Frank McSherry, Leonid Ryzhyk, Val Tannen.
  *DBSP: Automatic Incremental View Maintenance for Rich Query Languages*, PVLDB 16(7):1601–1614, 2023,
  DOI 10.14778/3587136.3587137, [PDF](https://www.vldb.org/pvldb/vol16/p1601-budiu.pdf), [arXiv 2203.16684](https://arxiv.org/abs/2203.16684).
  Code: github.com/vmware/database-stream-processor.
- **Key points:**
  - proposes DBSP, a language for dataflow computation, and an algorithm that incrementalizes any DBSP program;
  - incrementalizing a composed query equals composing the incremental versions of its parts;
  - most basic operators have incremental versions whose cost is proportional to the **size of the change**;
  - complex query features such as grouping, aggregation and recursion can all be modeled in it, and both SQL and Datalog can be
    implemented with it.
- **Implications for Octara:**
  - `List` should emit **incremental changes** (insert, delete, an item going from old value to new value), and derived
    collections should compute over the change stream instead of traversing again;
  - Several operations in the prototype can be incrementalized this way:
    - `count_where(pred)`: maintain the count, and on each change add or subtract according to "did the old value satisfy it, does the new value satisfy it", O(1);
    - `keys_where(pred)`: maintain the filtered ordered key set, handling only the changed item;
    - `any(pred)`: equivalent to the count being greater than 0;
  - "The incremental version of a composition is the composition of incremental versions" means we can provide a set of
    incremental operators (filter, map, count, group) for users to compose, with no need to hand-write incremental logic for each application.

## Incremental λ-calculus (ILC)

- **Source:** Yufei Cai, Paolo G. Giarrusso, Tillmann Rendel, Klaus Ostermann.
  *A Theory of Changes for Higher-Order Languages — Incrementalizing λ-Calculi by Static Differentiation*, PLDI 2014,
  [arXiv 1312.0658](https://arxiv.org/pdf/1312.0658), [project page](https://www.informatik.uni-marburg.de/~pgiarrusso/ILC).
- **Key points:**
  - incrementalizes through the program's **derivative**: the derivative maps changes in the input directly to changes in the
    output, without rerunning the original program;
  - the transformation is fully static and automatic, supports first-class functions, and the generated derivatives can be
    optimized by ordinary means;
  - changes are first-class entities, distinct from base values, and are also defined for functions;
  - correctness is machine-checked in Agda; the authors call this the first source-to-source incrementalization method for the
    pure λ-calculus that needs no runtime support;
  - a Scala prototype achieves order-of-magnitude speedups on a non-trivial program.
- **Implications for Octara:**
  - Provides a theoretical framework for "incremental operators": every collection operator should have a corresponding
    "change → change" function;
  - Static differentiation needs compiler support, which is unrealistic in Rust; but the idea that "changes are first-class
    values" can be used directly in designing the change type of `List`.

## Suggested validation experiment

In the prototype, add a change stream to `List` and incremental `count_where` and `keys_where`, and re-measure Todo toggle, add
and filter with the existing benchmarks. The goal is for the cost of these to be independent of list size at 1,000 items.
