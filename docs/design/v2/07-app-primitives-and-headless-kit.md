# Octara v2 Spec 07: Application Primitives, `octara-headless`, and Redesigned `octara-kit` (`0.0.2`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).  
> Design premise: **Zero historical baggage, zero backwards compatibility with `0.0.1` Kit APIs.**  
> Grounded in [Headless UI React: Components, Mechanisms, and Octara's Layers](../../research/headless-ui-react.md) and Octara v2's `Copy`-handle + capability-context runtime.

---

## 1. Crate Topology & Responsibility Boundaries

Introduce **`octara-headless`** as a first-class Layer 2 crate strictly between `octara` and `octara-kit`:

```text
octara (Layer 2a: Authoring Facade + App Primitives + Window Arbitration)
   ▲
octara-headless (Layer 2b: Pure Unstyled Behavior Parts & Copy State Machines)
   ▲
octara-kit (Layer 2c: Thin Themed Skin, Design Tokens & Turnkey Components)
```

Enforced by `tools/check_layers.py`:
- `octara-headless` production dependencies: **`{"octara"}`** only (zero `octara-model`, `octara-tree`, `octara-runtime`, or `octara-icons` direct imports; included in `AUTHORING_PACKAGES` import lint).
- `octara-kit` production dependencies: **`{"octara", "octara-headless", "octara-icons"}`**.
- **Strict responsibility split**:
  1. **Core (`octara-model`, `octara-tree`, `octara-runtime`, `octara`, hosts)**: owns mounted identity, real focus, input routing, window-level layer/exit/drag arbitration, semantics/IME, navigation/router runtime, and dev hot-restart.
  2. **`octara-headless`**: owns reusable interaction state machines, collection navigation, selection/controllable state, ARIA/AccessKit relation wiring, and `El::behavior` part attachment — **zero visual styles** (no hardcoded colors, padding, borders, radii, or shadows) and **zero wrapper bloat** (parts configure the caller's actual interactive `El`).
  3. **`octara-kit`**: owns visual design tokens (`Size`, `Variant`), theme styling, layout presets, and turnkey widgets composed directly on `octara-headless` — **zero duplicated state machines**.

---

## 2. Wave 1 — Core Application Primitives & Headless Window Contracts

### 2.1 Fine-Grained `FocusTrapFeatures` & Logical Layer Scope (`octara-model` / `octara-tree` / `octara-runtime` / `octara`)
Replace the coarse `LayerPolicy { modal: bool, click_outside: bool }` with explicit, composable layer policies:

```rust
bitflags! {
    pub struct FocusTrapFeatures: u8 {
        const NONE          = 0;
        const INITIAL_FOCUS = 1 << 0; // Move focus into layer on mount (autofocus or first tab stop)
        const TAB_LOCK      = 1 << 1; // Wrap Tab / Shift+Tab inside the layer
        const FOCUS_LOCK    = 1 << 2; // Prevent programmatic/pointer focus escape while active
        const RESTORE_FOCUS = 1 << 3; // Restore previously focused live node on dismiss
        const AUTO_FOCUS    = 1 << 4; // Re-focus container if focused descendant disappears
        const ALL           = Self::INITIAL_FOCUS.bits() | Self::TAB_LOCK.bits()
                            | Self::FOCUS_LOCK.bits() | Self::RESTORE_FOCUS.bits()
                            | Self::AUTO_FOCUS.bits();
    }
}

pub struct LayerPolicy {
    pub inert_outside: bool,            // True for modal Dialog / AlertDialog; false for non-modal Popover / Menu
    pub focus_trap: FocusTrapFeatures,  // Independent focus trap capabilities
    pub dismiss_on_escape: bool,
    pub dismiss_on_outside_press: bool,
    pub dismiss_on_focus_leave: bool,   // Dismiss when focus moves outside the logical layer scope
    pub group: Option<LayerGroupId>,    // Shared logical scope across PopoverGroup / nested Portals
}
```

Key invariants:
- `LayerPolicy::modal()` (`inert_outside: true`, `FocusTrapFeatures::ALL`, escape + outside press) and `LayerPolicy::non_modal()` (`inert_outside: false`, `FocusTrapFeatures::NONE`, escape + outside press + focus leave) provide crisp defaults.
- **Logical Layer Scope**:
  - `Tree::is_in_interaction_scope` respects both portal ownership (`OverlayMount::owner`) and `LayerGroupId`, so triggers, panels, nested portals, and sibling popovers in a `PopoverGroup` form one logical scope.
  - When a non-modal layer is dismissed by clicking an external focusable control, the new external focus target **wins** instead of being overwritten by `RESTORE_FOCUS`.
  - When multiple layers stack (e.g. a non-modal Popover inside a modal Dialog), Escape and outside presses dismiss only the top eligible layer.

### 2.2 Nested Exit Barrier & Stale-Exit Cancellation
- When a parent node enters `Exiting`, it coordinates an **exit barrier** over participating exiting descendants: the parent remains mounted for painting until all participating descendants complete their exit transitions (or immediately under `reduced_motion` / zero duration / invisible).
- Reopening a control (or mounting fresh content for the same logical overlay) cancels any stale pending `ExitFinished` so old exit completions can never unmount newly opened content.
- Input routing, focus restoration, and Agent target eligibility (`TargetGuard`) strictly obey the window's **current logical state**, never lingering exit visuals.

### 2.3 Extended Semantics & Tri-State `Checked`
- In `octara-model::semantics`:
  - Add `Checked` enum (`Unchecked`, `Checked`, `Mixed`) with `From<bool>` conversion so existing boolean callers stay ergonomic while tri-state checkboxes work everywhere.
  - Add missing ARIA/AccessKit `Role` variants: `Tree`, `TreeItem`, `ColumnHeader`, `Navigation`, `Separator`, `Tooltip`, `MenuItemCheckbox`, `MenuItemRadio`, `Toolbar`.
  - Add `described_by: Option<T>` to `Relations<T>`.
- Wire `Checked::Mixed`, new `Role`s, and `described_by` end-to-end through `octara-tree`, `octara-pipeline` (AccessKit `Toggled::Mixed`, `set_described_by`), `octara-host-web` (`aria-checked="mixed"`, `indeterminate`, `aria-describedby`), `octara-inspect`, and `octara::El` (`.described_by(...)`, `.checked(impl IntoProp<Checked>)`).

### 2.4 Unified `CloseHandle` Context (`octara-runtime` / `octara`)
- Provide a `Copy` scope-owned `CloseHandle` registered via `BuildCx::provide_close(impl Fn(&mut WriteCx<'_>) + 'static)` and consumed via `BuildCx::close_handle() -> Option<CloseHandle>` (`handle.close(w)`).
- Enables Headless `CloseButton` and arbitrary nested content inside `Dialog` / `Popover` / `Disclosure` / `Menu` to dismiss their enclosing control without threading close callbacks manually.

### 2.5 Application-Level Navigation & Router (`Navigator<R>`)
- First-class `Copy` handle `Navigator<R>` created in `BuildCx`:
  - `b.navigator(initial: R)` / `b.persisted_navigator("nav", || initial)` (integrates with `RestartState` when `R: Serialize + DeserializeOwned`).
  - Operations on `WriteCx`: `push(w, route)`, `replace(w, route)`, `pop(w) -> bool`, `pop_to_root(w)`, `reset(w, routes)`, `go(w, delta)` (forward/back history).
  - Reactive reads on `ReadCx`: `current(r) -> R`, `stack(r) -> Vec<R>`, `depth(r) -> usize`, `can_go_back(r) -> bool`, `can_go_forward(r) -> bool`.
  - **Leave Guards**: `b.on_before_leave(nav, |r, from, to| bool)` so forms with unsaved drafts can veto or intercept navigation cleanly.
  - **Page Mounting Modes**: helper view `route_stack(nav, options, |b, route| ...)` supporting `KeepAlive` (display-toggled retained subtrees preserving scroll/editor state) or `Unmount` (fresh scope per active entry).
  - **Platform Alignment**:
    - Native iOS interactive back gesture (`BackPhase`) integrates with `Navigator::can_go_back` and triggers `pop` on commit.
    - Web host provides optional URL/History bridge (`RouteCodec<R>`) for syncing `Navigator<R>` with browser `window.history` & `popstate`.

### 2.6 First-Class Element Drag & Drop (`draggable` / `drop_target`)
- Extend `octara-tree`'s `InteractionArbiter` and `octara::El` with first-class in-app drag-and-drop arbitration alongside existing `on_file_drop`:
  - `DragPayload` (typed/tagged `Arc<str>` kind + key/data, plus optional drag preview label/visual state).
  - `El::draggable(DragPayload::new(kind, data))` starts a guarded drag after the pointer or touch threshold. The source exposes `State::DRAGGING`. Escape cancels the drag.
  - `El::drop_target(kinds, axis, on_drop)` accepts matching payloads and divides the live receiver along `DropAxis` into `Before`, `Inside`, and `After`. `on_drop_hover` reports target entry, position changes and departure. `State::DROP_HOVER` marks the current receiver.
  - `El::reorder_keys()` makes a draggable row focusable. Space or Enter starts or confirms the drag, arrows select eligible siblings, and Escape cancels. The source keeps focus while the insertion target changes.

### 2.7 Dev Hot-Restart Runner (`octara-cli`)
- Re-wire `octara dev` / hot-restart runner in `crates/octara-cli` using v2's `RestartState` handoff protocol: watches crate source files, triggers clean rebuild, snapshots `RestartState` over the inspect socket before graceful shutdown, relaunches the rebuilt binary with the state snapshot, and verifies clean process teardown on exit.

---

## 3. Wave 2 — `octara-headless` (Pure Unstyled Behavior Layer)

Create `crates/octara-headless` (`version.workspace = true`, `publish = true`) depending ONLY on `octara`.

### 3.1 Core Architectural Patterns (Rust-Native Headless)
1. **Every controller is `Copy`** (arena-backed signals/memos in `BuildCx` scope — zero `Rc`, zero `RefCell`).
2. **Direct Node Attachment via `El::behavior`**: parts take or return `El` (e.g., `menu.item(el, key)` or `dialog.panel(el)`) and attach ARIA roles, relations (`controls`, `labelled_by`, `described_by`, `active_descendant`), `FocusGroup`, `LayerPolicy`, and event handlers directly onto the caller's interactive element — **zero extra wrapper nodes**.
3. **Shared Collection Engine (`collection`)**:
   - Operates directly on v2 `ReadList<K, T>` (and `virtual_each` + `WriteCx::reveal_row`).
   - Preserves active item identity across reorders/updates, skips disabled items, clears cleanly when removed, supports typeahead prefix search, and only publishes `active_descendant` when the active row is mounted.
   - Unified `Selection<K>` supporting both `Single(Binding<Option<K>>)` and `Multiple(Binding<Vec<K>>)` (or `BTreeSet<K>`).
4. **Controllable or Uncontrolled State**: `Control<T>` accepts either an external `Binding<T>` or an internal default value allocated in `BuildCx`.

### 3.2 Required Headless Component Families (`crates/octara-headless/src/`)
- `collection.rs` — shared collection navigation, active-item reconciliation, typeahead, virtual reveal.
- `close.rs` — `close_button(el)` using Core `CloseHandle`.
- `interactive.rs` — `button(el)`, `data_interactive(el)`.
- `checkbox.rs` — `checkbox(el, Checked binding)` (supports `Checked::Mixed` + `bool`), `switch(el, bool binding)`.
- `field.rs` — `FieldState`, `FieldsetState` (context-propagated `labelled_by`, `described_by`, and inherited `disabled`), `field`, `fieldset`, `label`, `description`, `control`.
- `disclosure.rs` — `DisclosureState`, `button`, `panel` (registers `CloseHandle`).
- `dialog.rs` — `DialogState`, `root` (portal + ExitBarrier + `CloseHandle`), `panel` (configurable `FocusTrapFeatures`), `backdrop`, `title`, `description`.
- `popover.rs` — `PopoverState`, `PopoverGroup`, `button`, `panel` (non-modal logical scope by default, configurable `FocusTrapFeatures`), `backdrop`.
- `menu.rs` — `MenuState`, `button`, `items`, `item`, `checkbox_item`, `radio_item`, `section`, `heading`, `separator`, `submenu`, plus **`ContextMenuState`** (pointer right-click + keyboard Shift+F10 / ContextMenu key anchored at pointer or focused element).
- `listbox.rs` — `ListboxState<K>`, `button`, `options` (standard & virtualized), `option` (custom row builder, single or multi selection).
- `combobox.rs` — `ComboboxState<K>`, `input` (IME composition guard — Enter during composition never commits option), `button`, `options` (standard & virtualized via `virtual_each` + `reveal_row`), `option` (custom row builder, single or multi selection).
- `radio_group.rs` — `RadioGroupState<K>`, `group`, `radio`.
- `tabs.rs` — `TabsState<K>`, `Activation::Automatic | Activation::Manual`, orientation (`Horizontal | Vertical`), `list`, `tab`, `panels`, `panel` (auto-wires `controls` / `labelled_by`).
- Tooltips use `El::tooltip` and the shared window layer in [13 §2.4](13-window-presentation-multimodal-input-and-inline-primitives.md#24-m4--precise-linkevent-modifiers-ordered-spsc-textstreamop-and-layered-hover-tooltip).
- `toast.rs` — `ToastQueue<T>` (`Copy` handle), `push`/`dismiss`/`pause`/`resume`, auto-dismiss timer via `TaskCx::sleep` (virtual-clock DST testable!), `Live::Polite` / `Live::Assertive`.
- `range.rs` — `SliderState`, `NumberFieldState`, `ProgressState`.
- `tree_view.rs` — `TreeViewState<K>`, reactive flattened visible rows over `ReadList`, expand/collapse (Left/Right arrows), roving focus, single/multi selection, virtual-ready.
- `data_table.rs` — `DataTableState<K, C>`, column definitions, multi/single sort state, row selection, virtualized body rows, column resize & drag-reorder hooks.

### 3.3 Mandatory Headless Acceptance Scenarios (from `docs/research/headless-ui-react.md`)
All 5 scenarios MUST have dedicated automated tests in `crates/octara-headless/tests/`:
1. Empty-to-populated picker disables/re-enables trigger cleanly, preserving stable business keys even with duplicate labels.
2. Combobox IME composition Enter does not commit an option; post-composition Enter commits once; keyboard navigation to off-viewport virtual options calls `reveal_row`, mounts the row, and only then publishes `active_descendant`.
3. Non-modal Popover inside modal Dialog: Escape dismisses only the top layer; clicking an external focusable control lets that control keep focus; nested portals stay inside logical scope.
4. Closing & immediately reopening an exiting overlay cancels stale `ExitFinished` without unmounting the reopened content; Agent/input eligibility tracks current logical state.
5. Controlled `Field` label change preserves editor mount identity & reports updated accessible name in the same cycle; manual `Tabs` separates focus from selection until confirmed.

---

## 4. Wave 3 — Redesigned `octara-kit`, Showcase & `0.0.2` Release

### 4.1 Thin Styled `octara-kit`
- Rebuild `octara-kit` cleanly as a thin themed skin over `octara-headless` (re-export `pub use octara_headless as headless;` for zero-friction drill-down).
- Introduce unified design tokens:
  - `Size`: `Sm`, `Md`, `Lg` (with touch-target `44px` preservation under `When::Touch`).
  - `ButtonVariant`: `Primary`, `Secondary`, `Ghost`, `Destructive`.
  - `BadgeVariant` / `CalloutVariant`: semantic theme colors (`Neutral`, `Accent`, `Success`, `Warning`, `Danger`).
- Complete turnkey widget surface:
  - **Core Controls**: `button(label, size, variant)`, `button_part` for custom elements, `icon_button`, `toggle_button` with a boolean binding, `checkbox` (tri-state), `switch`, `text_field`, `text_area`, `search_field`, `password_field`, `number_field`, `slider`, `progress`, `file_dropzone`.
  - **Forms & Choices**: `field`, `fieldset`, `radio_group`, `select` and `combobox` with `Selection`, `PickerOptions` and caller-authored row factories, and `combobox_input` for inline suggestions. Pickers and radio groups use Headless `Choice` data.
  - **Overlays & Feedback**: `dialog`, `alert_dialog`, `popover`, `menu_button`, `context_menu`, `toast_viewport` + `ToastQueue`, `command_palette`.
  - **Navigation & Structure**: `tabs` (auto & manual, integrated `tab_panel`), `segmented`, `breadcrumb`, `collapsible`, `list` / `list_item` / `list_header`, `split` / `splitter`, `separator`.
  - **Data & Rich Content**: `tree_view`, `data_table`, `badge`, `avatar`, `skeleton`, `spinner`, `chip` / `removable_chip`, `callout`, streaming `markdown`.

### 4.2 Showcase, Docs, Skills & `0.0.2` Packaging
- Upgrade `apps/chat` to use the redesigned `octara-kit` + `Navigator` (where natural, e.g. settings/conversation panes or compact navigation) + `context_menu` / `toast` feedback.
- Upgrade `examples/` to showcase `Navigator`, Drag & Drop reorder, `octara-headless` custom-styled widgets, `DataTable`, `TreeView`, `CommandPalette`, `ContextMenu`, `Toast`.
- Update `docs/` and regenerate `skills/octara/` (`octara skill export skills`).
- Bump workspace version to **`0.0.2`** across all **18 publishable crates** (17 existing + `octara-headless`), verify `cargo publish --workspace --dry-run`, all workspace tests, clippy, wasm32 (default & `webgpu`), and Chromium verification.
