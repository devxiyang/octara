# References for Architectural Decisions

These sources explain Octara's design choices. The choices below are Octara's own, and platform coverage follows what this repository implements and verifies.

| Source | Mechanism | Octara's design choice |
| --- | --- | --- |
| Rune | Ordinary Rust functions compose elements. The retained tree, region caching, platform frame ordering, and GPU paint-order slots each have their own conditions. | Keep the authoring API direct and use the known scope of a change to select layout and paint work. Give author extensions access to the normal paint order and lifecycle. [Rune architecture and source](rune-architecture.md) |
| Exact | The compiler, runner, hosts, data permissions, and build receipts work together around a verifiable application program. | Use permissions, versions, and receipts to decide which window work can proceed independently. Give the Agent evidence from the running application. [Exact architecture overview](exact-full-architecture.md) · [Runtime and presentation timing](exact-architecture.md) |
| SwiftUI | View descriptions, identity, lifetime, and dependencies have distinct responsibilities. | Determine state ownership by asking which object survives a reorder, then design the API. [Demystify SwiftUI](https://developer.apple.com/videos/play/wwdc2021/10022/) |
| Headless UI React | Behavior and semantics attach to replaceable control parts. Composite controls share collection, focus, and overlay mechanisms. | Let Kit configure the actual interactive node. Centralize identity, input routing, and cross-node coordination in Core. [Public exports and implementation](headless-ui-react.md) |
| W3C ARIA APG | Keyboard navigation, keyboard focus, and selection inside composite controls each have their own conventions. | Let the application own selection and the window own focus. Each component decides when to synchronize them. [Keyboard interaction guidance](https://www.w3.org/WAI/ARIA/apg/practices/keyboard-interface/) |
| Jetpack Compose | Nested scrolling allocates scroll deltas in phases. Fling phases also allocate velocity. | Verify handoffs against inputs and the deltas actually consumed before deciding which phases Octara should expose. [Nested scrolling](https://developer.android.com/develop/ui/compose/touch-input/scroll/nested-scroll-modifiers) |
| Elliott and Hudak | Time-varying values and discrete events are modeled separately. | Track visible progress separately from commit and cancellation times. Explicit state and event ordering can express these distinctions independently of the paper's runtime. [Functional Reactive Animation, ICFP 1997](https://conal.net/papers/icfp97/) |

Test proposed primitives against interactions from independent products. Desktop reordering and mobile dragging each provide a case where failures can be recalculated or observed. [Interaction timing](../design/v2/06-inspect-dst-release-migration.md)

Exact actions read state at the start and apply writes together. Rune handlers can mutate Rust state directly. Octara uses Rust's read and write semantics and defines the order of layout feedback and window publication. [Target architecture](../design/architecture-v2.md)
