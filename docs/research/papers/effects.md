# Modeling Effects

The problem: React's `useEffect` is criticized for doing too much and for dependencies that are hard to get right; the Signals
version of Effect in the prototype has the opposite problem, namely that implicit tracking leads to multiple subscriptions
(error injection I8), which existing diagnostics cannot detect.

## Elm: from asynchronous FRP to Cmd / Sub

- **Source:** Evan Czaplicki, Stephen Chong. *Asynchronous Functional Reactive Programming for GUIs*, PLDI 2013,
  DOI 10.1145/2462156.2462161, [BibTeX](https://bibtex.github.io/PLDI-2013-CzaplickiC.html);
  preceded by Czaplicki's paper *Elm: Concurrent FRP for Functional GUIs* (2012, [PDF](https://elm-lang.org:443/assets/papers/concurrent-frp.pdf)).
- **Key points:**
  - the paper presents Elm: declarative asynchronous FRP and purely functional graphical layout, whose core innovation is
    expressing asynchronous computation in FRP;
  - Elm later abandoned the "functional reactive" signal design in favor of Subscriptions; asynchronous work moved to Task, and
    the `async` construct from the paper was not implemented in later runtimes (secondhand: from a commentary article);
  - final form: effects are described as data (Cmd) and executed by the runtime; external events enter as subscriptions (Sub).
- **Implications for Octara:**
  - A mainstream language gave up "embedding effects in the reactive graph" in practice, and moved to "effects are data";
  - Handing effects to the runtime as typed commands lets them be recorded or replaced in headless tests and deterministic
    replay, and listed in full in the Inspector; this matches two goals in `ARCHITECTURE.md`: "typed, authorizable system
    operations" and "deterministic replay".

## Algebraic effects and handlers

- **Source:** Plotkin, Pretnar, *Handlers of Algebraic Effects* (ESOP 2009);
  Bauer, Pretnar, *Programming with Algebraic Effects and Handlers* ([arXiv 1203.1539](https://arxiv.org/pdf/1203.1539), the Eff language);
  an ICFP paper modeling asynchronous reactive programs and event wiring on top of algebraic effects ([PDF](https://namin.seas.harvard.edu/pubs/icfp-corrl.pdf)).
- **Key points:**
  - effects are algebraic operations, and a handler is a homomorphism out of the free algebra, i.e. operations that can be
    intercepted and reinterpreted;
  - the ICFP work above models asynchronous reactive programs and event wiring in direct style, with handlers acting as modular
    interpreters of event notification;
  - this round found no paper specifically about using algebraic effects in UI frameworks; robrix/ui-effects is an experimental
    code project, not a paper.
- **Implications for Octara:**
  - Rust has no native algebraic effects, but "effects are operations that a handler can reinterpret" can be implemented with
    trait objects: test, replay and real environments each provide a different handler;
  - This suggests an approach for the design of Native Modules: system capabilities are a set of operations whose
    implementations can be replaced, rather than direct calls.

## Reactive programming without functions

- **Source:** Bjarno Oeyen, Joeri De Koster, Wolfgang De Meuter. *Reactive Programming without Functions*,
  The Art, Science, and Engineering of Programming 8(3), 2024, DOI 10.22152/programming-journal.org/2024/8/11,
  [arXiv 2403.02296](https://arxiv.org/pdf/2403.02296).
- **Key points:**
  - calling ordinary host-language functions from reactive code (lifting) has two problems: host code may not terminate, making
    the program unresponsive; and the two paradigms do not match, which produces wrong behavior;
  - proposes a language with only reactive constructs and no functions, adding in steps stateful reactors, conditional dynamic
    dependencies, recursive reactors, and anonymous reactors with lexical scope;
  - the authors argue that pure reactive programming can express the programs that reactive languages relying on ordinary
    functions can express.
- **Implications for Octara:**
  - This is exactly the root of I8: a helper function reads a Signal incidentally inside an Effect, and the boundary between
    host functions and reactive tracking is unclear;
  - Functions cannot be removed in Rust, but the tracked code can be narrowed: provide `watch(source, callback)`, which tracks
    only the "source" and never tracks the callback, and keep the bare `effect` as an advanced API.

## Suggested validation experiment

Implement `watch(source, callback)` in the prototype, rewrite error injections I4 and I8 to use `watch`, and verify whether
these two classes of error structurally no longer occur.
