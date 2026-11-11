# Concurrency and Formal Semantics

Octara currently chooses a single-threaded reactive runtime plus a render thread, and does not yet need concurrent reactive
updates. This file records approaches that may be useful later, and the state of the art in formalization.

## REScala: thread-safe reactive programming

- **Source:**
  - Drechsler, Mogk, Salvaneschi, Mezini, *Thread-Safe Reactive Programming*, Proc. ACM Program. Lang. 2(OOPSLA), Article 107, 2018,
    [preprint](https://www.st.informatik.tu-darmstadt.de/preprints/PREPRINT%20Thread-Safe%20Reactive%20Programming%20Camera-Ready.pdf);
  - Same authors, *Distributed REScala: An Update Algorithm for Distributed Reactive Programming*, OOPSLA 2014,
    [conference page](https://2014.splashcon.org/details/oopsla2014/6/Distributed-REScala-An-Update-Algorithm-for-Distributed-Reactive-Programming).
- **Key points:**
  - 2018: existing reactive languages either avoid multithreading or give up expressiveness or consistency to support it;
    the scheduler in this work provides strict serializability without aborts, and the scheduling algorithm is formally proved;
    implemented in REScala, its scalability is comparable to hand-written application-specific locking and better than
    schedulers based on off-the-shelf software transactional memory;
  - 2014: proposes the SID-UP change propagation algorithm, which needs no centralized knowledge of the dependency graph in a
    distributed setting while guaranteeing no intermediate states (glitch-free).
- **Implications for Octara:**
  - If reactive state ever needs to be updated from multiple threads (for example background data written directly), this is a
    ready-made approach with a correctness proof;
  - For now we keep a single-threaded runtime, and background results are sent back to the UI thread through the scheduler, to
    avoid introducing this layer of complexity.

## Remus: a low-level reactive virtual machine

- **Source:** Oeyen, De Koster, De Meuter, *Reactive Programming on the Bare Metal: A Formal Model for a Low-Level Reactive Virtual Machine*,
  REBLS 2022 (SPLASH), [conference page](https://2022.splashcon.org/details/rebls-2022-papers/4/Reactive-Programming-on-the-Bare-Metal-A-Formal-Model-for-a-Low-Level-Reactive-Virtu),
  [technical report](https://soft.vub.ac.be/Publications/2022/vub-tr-soft-22-15.pdf).
- **Key points:**
  - gives a small-step operational semantics for Remus, a virtual machine for running reactive programs on constrained devices;
  - combines ahead-of-time topological sorting of the dependency graph with support for dynamic graphs (which usually needs a
    priority queue).
- **Implications for Octara:**
  - Ahead-of-time topological sorting plus dynamic graphs is a cheaper scheduling approach than sorting at runtime, and can be
    used to optimize large static subgraphs;
  - A reference for how to write a formal semantics for a reactive runtime.

## Formalization of UI description languages and Signals

- **Source:**
  - *Reactive Semantics for User Interface Description Languages*, arXiv 2508.13610 (2025), [HTML](https://arxiv.org/html/2508.13610);
  - *Signal-First Architectures: Rethinking Front-End Reactivity*, arXiv 2506.13815 (2025), [PDF](https://arxiv.org/pdf/2506.13815).
- **Key points:**
  - the former gives a denotational semantics for a minimal declarative UI description language, and notes that the behavior of
    React programs is usually described with a small-step operational semantics;
  - the latter takes Signals as the basic unit of reactivity and quantitatively compares the memory behavior of several Angular
    reactivity patterns (an empirical study, not a formal semantics);
  - this round found no 2024–2025 paper on the formal semantics of Signals-based UI frameworks.
- **Implications for Octara:**
  - Octara emphasizes determinism, replayability and verifiability; writing a formal semantics for the reactive kernel (update
    order, batching, scope destruction) is a gap nobody has yet filled, and it can also serve as the basis for differential
    testing and for Agent reasoning.
