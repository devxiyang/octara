# Octara v2 Spec 05: Delta-First Web/Wasm Host (`octara-host-web`)

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).

---

## 1. Zero-Bloat Wasm Dependency Contract

**CI-enforced guarantee**: `cargo tree --target wasm32-unknown-unknown -p octara-host-web` (default features) MUST contain **zero** instances of:
- `taffy`
- `parley`
- `accesskit`
- `wgpu`
- `swash`
- `zeno`
- `png`
- `roxmltree`

Only when an app explicitly opts into `octara-host-web/webgpu` does `octara-render` (with `wgpu`) enter the Wasm dependency graph — and even then, **zero** `std::thread`, `taffy`, `parley`, `accesskit`, or `png`.

---

## 2. Module Structure

```text
crates/octara-host-web/src/
  lib.rs          # WebApp public entrypoint, MessageChannel wake loop, ResizeObserver, event listeners
  engine.rs       # DomEngine implementing octara_runtime::HostEngine
  engine/         # Structure, semantic and visual patching, editors, assets, exits and geometry
  dom.rs          # DomStore (FxHashMap<NodeId, DomEntry>, WeakMap<Element, u64>), Delta patcher
  undo.rs         # Lightweight DomUndoLog for atomic speculative rollback + moveBefore editor parking
  measure.rs      # Rust binding to measure.js batched Float32Array bridge & demand-driven target selection
  measure.js      # ~40-line JS shim bundled via #[wasm_bindgen(inline_js = ...)]
  css.rs          # octara_model::{LayoutStyle, Visual} -> CSS declaration serializer
  edit.rs         # Controlled <input>/<textarea> -> EditorEvent bridge (CJK IME, reverse selection, KeyIntercept)
  image.rs        # Raster image projection using canvas ImageData
  services.rs     # Browser file picker & async clipboard permission bridge
  gpu.rs          # #[cfg(feature = "webgpu")] local WebGPU Canvas executor using octara-render
```

---

## 3. Pure `TreeDelta` DOM Patcher & Reversible `DomUndoLog`

### 3.1 Why v1's Dual-Snapshot Approach is Deleted
v1 built a full `Vec<DocumentNode>` (cloning every `String` in the tree) on every mutation and retained two full `Stage` snapshots to diff and restore on rollback.

Each `DomEngine::project(&Tree, &TreeDelta, viewport, now)` round patches structure, then semantics and styles, then editor and media content. The engine reads the affected geometry and prepares canvas updates before reporting feedback.

1. **Before mutating DOM**: If an active editor element (`document.activeElement`) is going to be moved, hidden, disabled, or retired during this speculative `project()` round, park it using `Element.moveBefore` (if supported by browser) into an off-screen connected parking container so Chromium preserves native IME composition and reverse selection. (On WKWebView without `moveBefore`, record its selection/composition state for explicit restoration on rollback.)
2. **Apply Delta & Record Undo-Log (`undo.rs`)**:
   Every DOM mutation appends the exact inverse to `self.undo_log: Vec<DomUndoOp>`:
   ```rust
   pub(crate) enum DomUndoOp {
       RemoveCreated { id: NodeId, element: HtmlElement },
       ReinsertRemoved { id: NodeId, parent: HtmlElement, next_sibling: Option<Node>, element: HtmlElement },
       MoveBack { element: HtmlElement, old_parent: HtmlElement, old_next_sibling: Option<Node> },
       RestoreAttr { element: HtmlElement, name: &'static str, prev: Option<String> },
       RestoreStyle { element: HtmlElement, prev: String },
       RestoreText { element: HtmlElement, prev: String },
       RestoreScroll { element: HtmlElement, prev: [f32; 2] },
       RestoreInputValue { element: HtmlElement, prev: String },
       RestoreAnchorPlacement { element: HtmlElement, prev: Option<Placed> },
   }
   ```
   - `delta.created`: create `HtmlElement` by `(NodeKind, Role, activatable)`, register `WeakMap.set(el, node_id.to_ffi())`, record `RemoveCreated`.
   - `delta.topology_dirty`: reorder only changed parent containers, recording `MoveBack` (using `moveBefore` when subtree contains active editor).
   - `delta.layout_dirty | delta.paint_dirty`: serialize CSS only for dirty nodes, recording `RestoreStyle`.
   - `delta.semantics_dirty`: patch only changed ARIA/tabindex/inert/modal/relation attributes, recording `RestoreAttr`.
   - `delta.scroll_dirty`: apply instant scroll targets, recording `RestoreScroll`.
   - `delta.retired`: stage detached nodes and retain visible exiting elements with their settled geometry. Suppress interaction with the exiting DOM while it fades.
3. **On `commit()`**:
   - Clear `self.undo_log` (`O(undo_len)` drop, zero DOM work!).
   - Permanently retire removed elements, unobserve from `ResizeObserver`, revoke retired image `ObjectURL`s, clear editor parking.
   - Start staged opacity animations through the Web Animations API after commit. Give independently fading descendants separate opacity contexts. Send `HostReply::ExitFinished(ticket)` for each completed participant and settle invisible, reduced-motion, or zero-duration exits immediately.
   - Publish staged WebGPU canvas plans (if `feature = "webgpu"`).
4. **On `rollback()`**:
   - Replay `self.undo_log` in **reverse order** (`for op in self.undo_log.drain(..).rev()`), restoring the exact committed DOM state in `O(|delta|)` without touching unchanged parts of the page!
   - Unpark active editor back to its exact prior position.

---

## 4. Shared-Memory JS Batched Geometry Bridge (`measure.js`)

### 4.1 Eliminating `10 * N` Wasm↔JS Crossings
Instead of calling several `web_sys` getters per node from Rust, `measure.rs` passes a JS `Array` of DOM elements and a `Float32Array` with `18 * count` floats across the boundary in a batch.

#### Flat ABI Layout per Node (`STRIDE = 18` floats = 72 bytes)

| Float Offset | Field | Source |
| --- | --- | --- |
| `0..4` | `bounds: [x, y, w, h]` | `getBoundingClientRect()` relative to `surface` origin |
| `4..6` | `client: [w, h]` | `el.clientWidth`, `el.clientHeight` |
| `6..8` | `content: [w, h]` | `el.scrollWidth`, `el.scrollHeight` |
| `8..10` | `scroll: [l, t]` | `el.scrollLeft`, `el.scrollTop` |
| `10..14` | `padding: [t, r, b, l]` | parsed `px` from cached `getComputedStyle(el)` |
| `14..18` | `border: [t, r, b, l]` | parsed `px` from cached `getComputedStyle(el)` |

```js
// Geometry bridge in measure.js
export function octara_measure_batch(surface, elements, out) {
  const origin = surface.getBoundingClientRect();
  const ox = origin.x, oy = origin.y;
  for (let i = 0, o = 0; i < elements.length; i++, o += 18) {
    const el = elements[i];
    const r = el.getBoundingClientRect();
    const hasBox = r.width !== 0 || r.height !== 0 || r.x !== 0 || r.y !== 0 || el.getClientRects().length !== 0;
    if (hasBox) {
      out[o]     = r.x - ox; out[o + 1] = r.y - oy;
      out[o + 2] = r.width;  out[o + 3] = r.height;
    } else {
      out[o] = out[o + 1] = out[o + 2] = out[o + 3] = 0;
    }
    out[o + 4] = el.clientWidth;  out[o + 5] = el.clientHeight;
    out[o + 6] = el.scrollWidth;  out[o + 7] = el.scrollHeight;
    out[o + 8] = el.scrollLeft;   out[o + 9] = el.scrollTop;
    const cs = el.ownerDocument.defaultView.getComputedStyle(el);
    out[o + 10] = parseFloat(cs.paddingTop) || 0;
    out[o + 11] = parseFloat(cs.paddingRight) || 0;
    out[o + 12] = parseFloat(cs.paddingBottom) || 0;
    out[o + 13] = parseFloat(cs.paddingLeft) || 0;
    out[o + 14] = parseFloat(cs.borderTopWidth) || 0;
    out[o + 15] = parseFloat(cs.borderRightWidth) || 0;
    out[o + 16] = parseFloat(cs.borderBottomWidth) || 0;
    out[o + 17] = parseFloat(cs.borderLeftWidth) || 0;
  }
}
```

### 4.2 Demand-Driven vs Full-Verify Measurement Modes
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeasureMode {
    /// Production default: O(dirty + observed). Measures only:
    /// - changed elements and affected scroll ancestors
    /// - demanded bounds, relevant anchors, and the active editor
    /// - nodes flagged by ResizeObserver
    #[default]
    DemandDriven,
    /// Diagnostic / test verification mode: measures all mounted DOM nodes in one JS batch
    /// to detect arbitrary external CSSOM mutations.
    VerifyFullCssom,
}
```
Even in `VerifyFullCssom` mode, 1,000 nodes cost **1 Wasm↔JS call** instead of 10,000!

---

## 5. Zero-PNG/Base64 Image Rendering (`image.rs`)

Delete `Picture::encode_png()` + `base64` from `octara-host-web` completely:
- **Primary path (`<canvas>`)**: `NodeKind::Image` projects to `<canvas role="img" aria-label="...">` with CSS `object-fit` semantics; when `(image_id, frame_version)` changes, copy RGBA pixels directly via `ImageData::new_with_u8_clamped_array_and_sh(Clamped(&frame.rgba), w, h)` → `ctx2d.put_image_data(&data, 0.0, 0.0)`.
  - Zero PNG compression CPU cost;
  - Zero Base64 encoding & 33% string bloat;
  - Zero `png` and `base64` crate dependencies in Wasm!
- **Alternative for static asset URLs**: If an `Image` carries a browser URL source in the future, project directly to `<img src="...">`.

---

## 6. Unified Pointer & Native Editing Bridge (`lib.rs`, `edit.rs`)

- **Pointer events (`O(1)` lookup)**: `pointerdown`/`pointermove`/`pointerup`/`pointercancel` map `event.target` via `WeakMap<Element, u64>` directly to `ContactTarget { hit: TargetGuard, action: Option<TargetGuard> }` and emit standard `InteractionEvent::Pointer*` — **zero Web-only `HostedPress`/`HostedClick` variants in core!**
- **Controlled Editing (`edit.rs`)**: Translates browser `beforeinput`/`input`/`compositionstart`/`compositionupdate`/`compositionend`/`selectionchange`/`keydown` into unified `EditorEvent` carrying `EditorSessionId` and `expected_rev`. Navigation keys needed by menus/comboboxes fire `EditorEvent::KeyIntercept` first; if unhandled, browser executes native cursor/editing behavior.
