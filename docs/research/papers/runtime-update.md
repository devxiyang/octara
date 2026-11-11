# Updating at Runtime: Hot Reload and State Migration

The problem: in the prototype, hot reload preserves state by keeping it in an external store keyed by scope path + key; when a
state type changes it is simply reset and `STATE_TYPE_CHANGED` is reported, with no means of migration.

## Dynamic software updating (DSU)

- **Source:**
  - Kitsune: Hayden, Smith, Denchev, Hicks, Foster, a DSU system for C (OOPSLA 2012, [DOI](https://dx.doi.org/10.1145/2384616.2384635); a journal extension followed);
  - Rubah: Pina, Hicks, Veiga, OOPSLA 2014, [PDF](https://sites.fct.unl.pt/sites/default/files/synergy-vm/files/2014-oopsla-pina-core-a.pdf);
  - State migration: Hayden et al., HotSWUp 2011 ([page](https://www.cs.umd.edu/~mwh/papers/hayden2011xfer.html)).
- **Key points:**
  - DSU lets a program be updated while running; state must be turned into the representation the new version expects by a
    **state transformation function**;
  - Kitsune was tested on five open-source programs, needing only small program changes and incurring almost no performance overhead;
  - Rubah proposes two transformation algorithms: a one-shot multi-threaded eager transformation, and a lazy transformation that
    converts objects when they are accessed. Lazy transformation has constant update time but adds overhead to object access in
    steady state (secondhand: from an encyclopedic summary);
  - Another approach by Hayden et al.: start a process running the new version, then migrate the state over from the old process.
- **Implications for Octara:**
  - When a state type changes, a migration function (old value → new value) should be allowed instead of only resetting; this can
    be combined with serde, migrating field by field and using defaults for new fields (Rune takes the same approach);
  - "Start a new process and then migrate state" corresponds to the split-process development mode proposed by Rune (renderer on
    the device, application logic on the development machine);
  - The trade-off between lazy and eager transformation directly affects hot-reload latency for large state (long lists, documents).

## Hazel: live functional programming

- **Source:** Omar, Voysey, Chugh, Hammer, *Live Functional Programming with Typed Holes*, POPL 2019,
  [arXiv 1805.00155](https://arxiv.org/pdf/1805.00155); *Hazel: Live and Direct Functional Programming with Holes*, LIVE 2018;
  papers on Livelits (live literals) ([NSF PAR](https://par.nsf.gov/servlets/purl/10312229)).
- **Key points:**
  - provides a dynamic semantics for incomplete programs (programs with "holes"), so the editor always has types, can run, and
    gives feedback at any moment;
  - a live context inspector shows the values in hole closures;
  - Livelits allow user-defined GUIs to be embedded persistently in code to fill holes of a particular type, and they are composable;
  - this round found no specific discussion in Hazel of state preservation or hot reload.
- **Implications for Octara:**
  - "Incomplete programs can run and give feedback" is relevant to the Agent development loop: a half-generated UI can still be inspected;
  - Livelits, Rune's live literals and Exact's fast edit loop belong to the same direction: adjusting a parameter should not
    require a full recompile.
