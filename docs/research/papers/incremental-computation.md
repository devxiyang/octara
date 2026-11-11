# Incremental Computation

The theoretical source of fine-grained Signals. The shared problem: after an input changes, recompute only the affected parts,
and guarantee that the result is consistent with computing from scratch.

## Self-adjusting computation

- **Source:** Umut A. Acar et al. Representative works: Selective Memoization (POPL 2003, [arXiv 1106.0447](https://arxiv.org/pdf/1106.0447)),
  An Experimental Analysis of Self-Adjusting Computation (PLDI 2006, [Google Research](https://research.google/pubs/an-experimental-analysis-of-self-adjusting-computation)),
  Traceable Data Types for Self-Adjusting Computation (PLDI 2010, [PDF](https://condor.depaul.edu/dturkogl/research/traceable-pldi10.pdf)),
  A Consistent Semantics of Self-Adjusting Computation (JFP 2013, [PDF](https://www.cambridge.org/core/services/aop-cambridge-core/content/view/441A28C813BDA23B57F1ED2BB1A7E36E/S0956796813000099a.pdf/a-consistent-semantics-of-self-adjusting-computation.pdf)).
- **Key points:**
  - a dynamic dependency graph is used to find and re-execute the parts affected by a change, and memoization to reuse the
    unaffected parts; combining the two gives the change propagation algorithm;
  - consistency result: the nondeterminism introduced by memoization does not change the result of the computation;
  - limitation: for many computations and modifications, change propagation is no faster than recomputing from scratch, and
    asymptotic speedups are not common;
  - Traceable Data Types extend the method to general data types, rather than only single mutable cells.
- **Implications for Octara:**
  - Finer granularity is not always better. Tracking itself has a cost, and granularity should be weighed in measurements (the
    Memo and binding boundaries in the prototype are exactly such a trade-off);
  - Traceable Data Types match our `List` idea: a collection should be a first-class traceable data type, not one big value.

## Adapton

- **Source:** Matthew A. Hammer, Khoo Yit Phang, Michael Hicks, Jeffrey S. Foster.
  *Adapton: Composable, Demand-Driven Incremental Computation*, PLDI 2014, DOI 10.1145/2594291.2594324,
  [PDF](https://people.mpi-sws.org/~hammer/adapton-pldi2014.pdf), [project page](https://cs.umd.edu/~hammer/adapton).
- **Key points:**
  - two problems with earlier incremental systems: recomputation ignores which outputs the observer actually needs; and programs
    can only be incrementalized as a whole, making it hard to reuse partial results;
  - proposes a demand-driven semantics (the core calculus λCDDIC) and a layered "demanded computation graph";
  - clearly separates internal incremental computation from external observers;
  - implemented as an OCaml library; the authors report that it is usually significantly faster than previous approaches on benchmarks.
- **Implications for Octara:**
  - The prototype's "push marking + pull evaluation, Memo computed on demand" is the specialization of this model to UI;
  - "Separating internal computation from external observers" corresponds to the division of labor between property bindings
    (observers) and Memos (internal computation), and can serve as the conceptual basis of Octara's documentation.

## Nominal Adapton (incremental computation with names)

- **Source:** Hammer, Dunfield, Headley, Labich, Foster, Hicks, Van Horn.
  *Incremental Computation with Names*, OOPSLA 2015, [arXiv 1503.07792](https://arxiv.org/abs/1503.07792v5),
  [conference page](https://2015.splashcon.org/details/oopsla2015/1/Incremental-Computation-with-Names). Follow-up work: Typed Adapton.
- **Key points:**
  - argues that first-class **names** are the key language feature for efficient incremental computation: the programmer
    associates a name with a node, and equal names mean the same node;
  - example: inserting an element in the middle of a list does not change the names of the output cells, so map results before
    the insertion point can still be reused;
  - risk: the same name may be associated with different nodes. The system detects this ambiguity and raises an exception;
  - formalized as a core calculus, with a proof that the result is always consistent with computing from scratch;
  - the authors report speedups of up to 10900x over from-scratch computation and up to 21000x over classic Adapton.
- **Implications for Octara:**
  - `state(key, ..)` and list row keys are essentially this kind of naming, and the paper gives a theoretical basis for
    "explicit names as identity";
  - The component identity problem pointed out by Agent evaluation ("component name + index among same-named siblings") can be
    improved accordingly: components support explicit names, and the runtime detects name ambiguity (`DUPLICATE_STATE_KEY` and
    `DUPLICATE_KEY` already exist).

## Salsa

- **Source:** the incremental computation engine used by rust-analyzer. [Documentation](https://docs.rs/salsa),
  [Durable Incrementality (rust-analyzer blog, 2023-07)](https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html).
- **Key points:**
  - the "red-green" algorithm; the database tracks only one current revision number, incremented each time an input is set;
  - **early cutoff**: when a recomputed value is unchanged, propagation downstream stops;
  - **durability**: marks how often data changes (for example, inputs from crates.io have high durability). Each durability
    level records the revision of its most recent change; a query that depends only on high-durability inputs can be confirmed
    valid without walking dependency edges when only low-durability inputs have changed.
    Motivation: without durability, editing `src/lib.rs` requires validating all queries related to the standard library (about 300 ms);
  - the durability of a derived query is the lowest among its direct inputs;
  - in the new version (0.28), setters always record the field as changed, without first comparing old and new values.
- **Implications for Octara:**
  - This is an incremental engine proven in large-scale production in Rust, and its internals (revision numbers, validation,
    durability) are worth consulting when implementing the reactive kernel properly;
  - Durability can be borrowed directly: rarely changing state such as theme, configuration and localized text is marked high
    durability, so that large applications can skip validation of whole regions;
  - Early cutoff corresponds to the equality comparison of Memos in the prototype.

## Jane Street Incremental / Bonsai

- **Source:** Jane Street's OCaml libraries. [Bonsai (opam)](https://opam.ocaml.org/packages/bonsai/bonsai.v0.16.0/),
  [Incr_dom tutorial](https://blog.janestreet.com/a-tutorial-for-building-web-applications-with-incrdom/).
- **Key points:**
  - Incremental: an incremental computation library that tracks dependencies automatically;
  - Bonsai: builds composable incremental state machines on top of incremental computation, for web UI; development began in
    2019 because Incr_dom was too hard for newcomers;
  - most of Jane Street's internal web applications use Bonsai.
- **Implications for Octara:**
  - Shows that UI based on incremental computation can be used in production at scale;
  - Bonsai's motivation for evolving from Incr_dom (ease of use) reminds us that beyond the correctness of the incremental
    model, the learnability of the API equally decides success.
