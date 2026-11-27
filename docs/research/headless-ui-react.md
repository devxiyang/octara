# Headless UI React: Components, Mechanisms, and Octara's Layers

Headless UI React packages complete control behavior in composable parts and reuses mechanisms for actual focus, collection navigation, overlay arbitration, and semantic relationships. In Octara, Kit can configure the nodes users interact with, and Core and the hosts can coordinate behavior across nodes and system services. Apps compose ordinary Rust functions and can use Core directly or build common interactions with Kit. [Octara layering and criteria for adding primitives](../ARCHITECTURE.md)

This study uses Tailwind Labs' `@headlessui/react` **2.2.10**, released April 7, 2026, at commit `d13526d02a2de92c4ad7b62c15cd980636543fe2`. The public API count comes from the released package's root runtime named exports. Implementation details were checked at the same commit. The findings come from static review of the released package and source. The closing Octara scenarios define acceptance criteria. [npm version](https://www.npmjs.com/package/%40headlessui/react?activeTab=versions) · [Release notes](https://github.com/tailwindlabs/headlessui/releases/tag/%40headlessui%2Freact%40v2.2.10) · [Pinned export list](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/index.ts)

## Public API Inventory

The package root has **65 named runtime exports**. Of these, 53 are current renderable components or parts, 10 are deprecated renderable names, and two are the `FocusTrapFeatures` enum and `useClose` hook. The official catalog's 16 component families account for 46 current exports, with seven more shared form parts and utilities. [Official catalog](https://headlessui.com/) · [Released component source](https://github.com/tailwindlabs/headlessui/tree/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components)

| Component family | Current exports | Behavior |
| --- | --- | --- |
| Menu | `Menu`, `MenuButton`, `MenuItems`, `MenuItem`, `MenuSection`, `MenuHeading`, `MenuSeparator` | Navigate commands, find them with typeahead, run a command, and close the menu. |
| Combobox | `Combobox`, `ComboboxInput`, `ComboboxButton`, `ComboboxOptions`, `ComboboxOption` | Keep focus in the search field and highlight and commit options independently. Supports single selection, multiple selection, and virtual lists. |
| Listbox | `Listbox`, `ListboxButton`, `ListboxOptions`, `ListboxOption`, `ListboxSelectedOption` | Customize a single- or multi-select list and optionally show the selected value in the trigger. |
| Radio Group | `RadioGroup`, `Radio` | Choose one option. Arrow keys move actual focus and select the option. |
| Tabs | `TabGroup`, `TabList`, `Tab`, `TabPanels`, `TabPanel` | Pair tabs and panels with configurable orientation and automatic or manual activation. |
| Dialog | `Dialog`, `DialogPanel`, `DialogTitle`, `DialogBackdrop` | Define modal scope, set initial focus, handle dismissal, and restore focus. |
| Popover | `Popover`, `PopoverButton`, `PopoverPanel`, `PopoverBackdrop`, `PopoverGroup` | Anchor a panel and manage focus across the trigger, panel, and group members. |
| Disclosure | `Disclosure`, `DisclosureButton`, `DisclosurePanel` | Expand and collapse content in document flow. |
| Transition | `Transition`, `TransitionChild` | Coordinate entry, exit, and unmounting after descendant exits complete. |
| Button | `Button` | Use a native button and expose focus, hover, and press feedback. |
| Checkbox | `Checkbox` | Manage a controlled or default-checked value, including an indeterminate state. |
| Switch | `Switch` | Manage an on/off state with a controlled value or an internal default. |
| Fieldset | `Fieldset`, `Legend` | Group form fields, name the group, and propagate disabled state. |
| Input | `Input` | Use a native single-line input with field semantics and interaction state. |
| Select | `Select` | Use a native HTML select control. |
| Textarea | `Textarea` | Use a native multiline input with field semantics and interaction state. |

### Shared Parts, Utilities, and Dismissal

| Public export | Responsibility |
| --- | --- |
| `Field`, `Label`, `Description` | Associate a control with mounted labels and descriptions and propagate disabled state. |
| `DataInteractive` | Expose focus, hover, and press states on custom nodes. |
| `CloseButton`, `useClose` | Invoke the nearest Dialog, Disclosure, or Popover's close operation. Menu supplies `close` through its own render prop. |
| `FocusTrap`, `FocusTrapFeatures` | Configure `InitialFocus`, `TabLock`, `FocusLock`, `RestoreFocus`, and `AutoFocus` independently, or disable them with `None`. |
| `Portal` | Change the actual mount location and help register the scope of nested portals. |

`DataInteractive` supplies interaction feedback. The caller declares button semantics and business actions. Portal mount location is configurable independently of modal policy. [DataInteractive](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/data-interactive/data-interactive.tsx#L29-L56) · [Close context](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/close-provider.tsx) · [FocusTrap](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/focus-trap/focus-trap.tsx#L53-L88) · [Portal](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/portal/portal.tsx#L208-L259)

### Deprecated Exports

The 10 top-level names are `ComboboxLabel`, `DialogDescription`, `ListboxLabel`, `PopoverOverlay`, `RadioGroupOption`, `RadioGroupLabel`, `RadioGroupDescription`, `SwitchGroup`, `SwitchLabel`, and `SwitchDescription`. Seven alias the shared `Label` or `Description` component. `PopoverOverlay`, `RadioGroupOption`, and `SwitchGroup` each have their own legacy implementation. [Released component exports](https://github.com/tailwindlabs/headlessui/tree/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components)

There are 40 legacy renderable dot-notation APIs. Each combines an owning component and a part from the table below. `Portal.Group` is the only renderable part exposed exclusively through dot notation. `Transition.Root` aliases `Transition`, and `FocusTrap.features` separately aliases the `FocusTrapFeatures` enum. [Legacy Portal API](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/portal/portal.tsx#L254-L259) · [Legacy Transition APIs](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/transition/transition.tsx#L637-L648)

| Owning component | Legacy dot-notation parts |
| --- | --- |
| `Combobox` | `Input`, `Button`, `Label`, `Options`, `Option` |
| `Dialog` | `Panel`, `Title`, `Description` |
| `Disclosure` | `Button`, `Panel` |
| `Listbox` | `Button`, `Label`, `Options`, `Option`, `SelectedOption` |
| `Menu` | `Button`, `Items`, `Item`, `Section`, `Heading`, `Separator` |
| `Popover` | `Button`, `Backdrop`, `Overlay`, `Panel`, `Group` |
| `Portal` | `Group` |
| `RadioGroup` | `Option`, `Radio`, `Label`, `Description` |
| `Switch` | `Group`, `Label`, `Description` |
| `Tab` | `Group`, `List`, `Panels`, `Panel` |
| `Transition` | `Child`, `Root` |

## Shared Mechanisms and Their Limits

### Configuring the Interactive Node

`as` selects the element that carries the behavior. The render layer merges refs, semantic attributes, and events into that element and exposes state through render props and `data-*`. Forwarding these props through a Fragment requires a single child that can receive them. Component-owned props win conflicts. Caller event handlers run first and can prevent later handlers from running with `preventDefault()`. When the control is disabled, prop merging disables the relevant interaction handlers. [Node rendering](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/render.ts#L130-L270) · [Prop and event merging](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/render.ts#L339-L394)

Press handling tracks pointer leave, move, up, and cancel. Menu-style triggers choose when to open based on the device. A mouse can open before focus changes. Touch and pen use the click path. Pressing the trigger, dragging to an option, and releasing also has dedicated handling. These paths can share hit testing and cancellation signals. Each control decides whether to open, select, or invoke a command. [Press state](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-active-press.tsx#L38-L97) · [Device type and toggle timing](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-handle-toggle.tsx#L6-L53) · [Drag and release](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-quick-release.ts#L22-L113)

Octara uses `El::behavior` to attach component behavior and app callbacks to the same mounted node. IME routing runs first, followed by component behavior and app callbacks. Kit's `select` and `combobox` attach role, name, expanded state, and input behavior to an app-provided trigger. The app can style and disable the same element, give it a tooltip, and add a press callback. [Octara `El::behavior`](../../crates/octara/src/view/el.rs) · [Behavior layer](../../crates/octara/src/view/behavior.rs) · [Kit selectors](../../crates/octara-kit/src/choices.rs)

### Collection Identity, Focus, Active Items, and Commit

Headless UI updates collection positions from live nodes, preserves the active item on reorder, and clears removed targets. Some composite controls share reducer, subscription, and cleanup machinery while keeping their own state. Combobox, Listbox, and RadioGroup support both controlled and default values and can compare business values with `by`. Dynamic options, the active item, and the committed value can change independently. [Collection reordering](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/menu/menu-machine.ts#L69-L92) · [Active item calculation](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/utils/calculate-active-index.ts#L25-L94) · [Shared state machine](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/machine.ts#L5-L88) · [Controlled values](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-controllable.ts#L5-L45)

| Control | Actual focus | Navigation and commit |
| --- | --- | --- |
| Menu | Menu container | The active item is the current command target. Confirming runs it. |
| Listbox | Options container | The active item and selected value are independent. Committing a single selection closes the list. In multi-select mode, the list can stay open. |
| Combobox | Editor | The editor exposes the active item as a semantic relationship. The app can maintain the query independently and write the selection on confirmation. |
| Manually activated Tabs | Current tab | Arrow keys move focus. Confirming changes the selected tab and panel. |
| RadioGroup | Current radio | Arrow keys move focus and update the selected value. |

Combobox lets the IME handle Enter first during composition and can still pass edits to the app for filtering. Octara's hosts and editor already have a system composition and draft contract. Kit needs to decide for each product when to filter on drafts and when to commit a business value. Controls can share collection ordering, disabled-item filtering, and identity resolution. Each control decides whether focus moves, whether navigation commits, and what it commits. [Menu keyboard handling](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/menu/menu.tsx#L458-L556) · [Listbox commit](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/listbox/listbox-machine.ts#L513-L543) · [Combobox input](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L678-L825) · [Tabs keyboard handling](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/tabs/tabs.tsx#L444-L502) · [RadioGroup keyboard handling](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/radio-group/radio-group.tsx#L223-L275) · [Octara text input](../design/v2/03-tree-microkernel.md)

### Overlay Placement, Logical Scope, and Modal Authority

Positioning updates the panel from its anchor and available space. Portal manages the physical mount location and registers descendants. The overlay stack decides which layer handles Escape or outside presses. Dialog also limits background interaction and restores focus. A Popover's logical scope can include its trigger, panel, nested portals, and grouped overlays. An outside press on a focusable control can leave focus on that new target. Focus isolation, scroll lock, and exit state each end on their own schedule. [Anchor positioning](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/floating.tsx#L197-L248) · [Overlay stack](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-is-top-layer.ts#L29-L65) · [Dialog](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/dialog/dialog.tsx#L191-L269) · [Popover](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/popover/popover.tsx#L175-L240)

In Octara, a Headless controller declares an overlay's anchor and dismissal conditions. The window uses mount relationships and input sessions to determine who responds. Kit's `popover` uses a nonmodal scope. `combobox_input` shows suggestions while the editor retains focus. The editor-in-Dialog example covers the panel's logical ownership and key routing during composition. [Octara overlay contract](../design/v2/02-reactive-runtime.md) · [Editor and overlay examples](../../examples/src/showcase/interaction.rs)

### Virtual Options and Exit Timing

Combobox is the component family with a public virtualization API in this React release. Navigation uses the entire logical option set and can request scrolling to an active option outside the viewport. The accessible active-descendant relationship only references a mounted row for the current active option. Mounted rows also report their position and the total size of the full collection. Octara's `virtual_each` already uses business keys to preserve row identity. [Virtual scrolling](https://headlessui.com/react/combobox#virtual-scrolling) · [Virtual row measurement](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L134-L232) · [Active target checks](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox-machine.ts#L608-L628) · [Octara virtual collections](../../crates/octara/src/view/list.rs)


`WriteCx::reveal_row` asks a mounted virtual list to measure and scroll to a business key. `select` and `combobox` keep focus on the list or search field. Once mounted, the active row can be resolved by `active_descendant`. [Reveal contract](../design/v2/03-tree-microkernel.md) · [Chat conversation list](../../apps/chat/src/ui.rs) · [Kit selectors](../../crates/octara-kit/src/choices.rs)


Rune identifies the active option by business key and reveals it by key while the editor holds actual focus. Chooser key handling is skipped during composition. The overlay body and results container can shrink, so the results viewport handles scrolling when the keyboard reduces available space. Exact positions iOS overlays within the available page area, accounting for the keyboard's top edge, and passes the resulting area to content layout. Expo's SwiftUI TextField distinguishes the Return key label from the `onSubmit` callback. Verify the ordering of system input and choice submission with a real IME. [Rune Combobox](https://github.com/anomalyco/rune/blob/3686e94ae09545fd5f2a61e22aabfbeba1cd896f/crates/rune-kit/src/combobox.rs#L216-L338) · [Exact iOS overlays](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/MenusIOS.swift#L749-L792) · [Expo TextField](https://docs.expo.dev/versions/latest/sdk/ui/swift-ui/textfield/#submit-handling)

Transition keeps exiting content painted, waits for participating descendants to finish, then unmounts. Reopening cancels stale exit completion. Combobox can also retain a visual snapshot of exiting content. Octara should define when input, accessibility objects, painting, and unmounting take effect in each scenario. Once an overlay starts closing, the window should use current interaction authority to evaluate old Agent targets independently of whether the old pixels remain visible. [Nested exits](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/transition/transition.tsx#L176-L274) · [Transition cancellation](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/hooks/use-transition.ts#L255-L284) · [Exit snapshots](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/combobox/combobox.tsx#L1334-L1398)

### Field Semantics and System Editing

`Field`, `Label`, and `Description` compose semantic relationships among mounted elements. `Input`, `Textarea`, and `Select` use native browser controls. Choice components take part in native forms through form ownership, disabled state, submission, and reset. In Octara, Kit can compose labels, descriptions, controls, and model bindings. The window and host own actual edit sessions, selections, composition ranges, candidate-window anchors, and platform accessibility. Label text changes should update the original control through its label relationship and become readable by the Inspector and platform accessibility in the same update cycle. [Field](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/components/field/field.tsx#L28-L74) · [Form participation](https://github.com/tailwindlabs/headlessui/blob/d13526d02a2de92c4ad7b62c15cd980636543fe2/packages/%40headlessui-react/src/internal/form-fields.tsx#L21-L105) · [Octara field composition](../../crates/octara-kit/src/forms.rs) · [Window semantics](../../crates/octara-tree/src/semantics.rs)

## Octara Design Choices and Acceptance Criteria

The **app** owns business IDs, data sources, and commit policy. **Kit** composes ready-to-use behaviors, lets callers configure the actual parts, and can share internal implementations across components. **Core and the host** own mount identity, actual focus, input routing, window-level coordination, and system editing. Consider a more general Core API only if a real caller must duplicate window state, compete for the same input, or bypass a system protocol. [Octara primitives and identity](../ARCHITECTURE.md)

The Chat toolbar model picker configures styling, tooltips, empty-state text, and disabled conditions in the `combobox` trigger factory. It uses the same picker with an empty or populated model list. The window and Agent read semantics and available actions from the actual trigger. Model IDs represent business selections, and the framework assigns mount identities to the trigger and options. [Chat model picker](../../apps/chat/src/ui.rs) · [Kit selectors](../../crates/octara-kit/src/choices.rs)

| Scenario | Acceptance criteria |
| --- | --- |
| Chat model picker | The picker is disabled when the collection is empty and available when models return. Style, tooltip, role, and Agent actions refer to the actual trigger. Models with duplicate names are distinguishable by stable ID. |
| Combobox search and virtual options | During Chinese composition, Enter serves only the current IME operation. Confirming an option after confirming the composed word commits the business selection once. Keyboard navigation to an offscreen option scrolls and mounts it before publishing a resolvable active-descendant relationship. |
| Nonmodal overlay inside a Dialog | Escape dismisses only the top layer authorized to handle it. Focusable nodes outside can receive focus, and dismissal restores focus only to a still-valid target. Nested overlays retain their logical scope when the mount location changes. |
| Closing and reopening an overlay | The old visuals can finish exiting. Input and Agent actions follow the window's current state. A stale exit completion leaves reopened content mounted. |
| Controlled field and manual Tabs | Editing the label preserves the editor's mount identity and reports the new name. Keyboard focus and the selected panel can change independently in manual Tabs. Confirmation switches the panel. |

These scenarios identify what existing composition supports, what Kit APIs need to add, and what requires window arbitration. A headless driver can check timing and state. System IME, platform focus, and accessibility output require observations from the relevant host. [Octara verification plan](../design/architecture-v2.md#9-phased-execution--verification-plan)
