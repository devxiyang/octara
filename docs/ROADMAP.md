# Platform verification roadmap

The architecture specification defines the current crate boundaries and engineering checks. The release roadmap covers milestones and platform priorities. [v2 architecture specification](design/architecture-v2.md) · [Release roadmap](design/roadmap.md)

| Area | Verification |
| --- | --- |
| Editing, reordering, modals, and touch scrolling | [Headless regression tests](../crates/octara-inspect/tests/headless.rs) |
| Multiple windows, the virtual clock, and replay | [Deterministic simulation](../crates/octara-inspect/src/dst.rs) |
| GPU queues, color, and readback | [Rendering tests](../crates/octara-render/tests/integration/) · [Device executor](../crates/octara-gpu/src/executor.rs) |
| First frame in a native macOS window | [Native host smoke example](../crates/octara-host-native/examples/smoke.rs) |
| Live DOM, selection, and rollback in Chromium | [Browser examples](../crates/octara-host-web/examples/) |

Platform verification prioritizes macOS 15.4+, iOS 18.4+, Web, and Linux. Windows and Android are deferred. Record live-window and device verification in the relevant tests and build receipts. [Follow-up checklist](TODO.md)
