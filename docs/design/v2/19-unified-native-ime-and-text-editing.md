# 19 — Unified Native IME, Text Editing & Clipboard Hygiene (`0.0.10`)

> **Design premise**: Zero historical `winit` baggage, zero backwards compatibility with `winit::event::Ime`-era event variants or duplicated per-platform text shadow state machines.  
> **North star**: Now that every Tier-1 native host (macOS `15.4+` AppKit, iOS `18.4+` UIKit, Linux Wayland `zwp_text_input_v3` / X11) and Web (`beforeinput` / `composition*`) speaks its native text protocol directly, unify the editor event protocol, host speculative text bridge, semantic editing commands, key-routing priority, and clipboard hygiene into one minimal, cross-platform architecture.

---

## 1. Remaining `winit`-Era Debts in Text Editing & Host Services

Even after removing `winit` from `Cargo.toml`, five structural artifacts of the old `winit` layer remain:

1. **Redundant `EditorEvent` Variants (`octara-model/src/input.rs`)**:
   - `EditorEvent` carries overlapping variants created solely for `winit::event::Ime::{Preedit, Commit, Disabled}` (`Composition`, `Commit`, `CancelComposition`, `KeyFallback`) alongside the richer native variants (`ProposeEdit { edit: TextEdit::{Mark, Insert, Commit, Unmark, Cancel} }`).
2. **Duplicated Speculative Shadow Mirrors Across Platforms**:
   - `os/macos/editor.rs` (~765 LoC) implements `Session { document: EditorMirror, predicted: VecDeque<(u64, EditorMirror)> }`.
   - `os/ios/editor.rs` + `os/ios/draft.rs` (~1,266 LoC) implements an entirely separate `Draft { displayed, expected, committed } + PendingInputs + inflight` state machine.
   - `os/linux/wayland/` needs a third bridge for `zwp_text_input_v3` (`surrounding_text`, `delete_surrounding_text`, `preedit_string`, `commit_string`, `content_type`).
   - UTF-8 ↔ UTF-16 conversion and `TextGeometry` rect/hit queries are copy-pasted across backends.
3. **Fragmented Editing Command Execution**:
   - `os/macos/editor.rs` manually executes `word_edge`, `grapheme_edge`, `adjacent_atom` (rich-text pills), and clipboard commands inside AppKit selector dispatch, duplicating `octara-runtime/src/cycle/text.rs` and diverging from iOS/Linux/Web.
4. **4-Hop Key Ping-Pong Loop (`shell.rs`)**:
   - `KeyDown → KeyIntercept → on_unhandled_key → KeyFallback → fallback_edit` existed because `winit` intercepted `NSView::keyDown:` before Octara. Without `winit`, direct hosts can enforce one deterministic 4-tier priority law.
5. **Dead `arboard` Clipboard Dependency**:
   - With direct `NSPasteboard` (macOS), `UIPasteboard` (iOS), `wl_data_device` (Wayland), and `x11rb` ICCCM (X11), `arboard` is completely redundant and should be deleted from `Cargo.toml`, `crates/octara-host-native/Cargo.toml`, and `Cargo.lock`.

---

## 2. Clean-Sheet Orthogonal Protocol (`octara-model` + `octara-tree` + `octara-runtime`)

### 2.1 Minimal `EditorEvent` & Unified `EditorCommand`

Replace the overlapping `winit`-era variants in `octara-model/src/input.rs` with four orthogonal variants:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorCommand {
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordLeft,
    MoveWordRight,
    MoveLineStart,
    MoveLineEnd,
    MoveDocumentStart,
    MoveDocumentEnd,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
    SelectDocumentStart,
    SelectDocumentEnd,
    SelectAll,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    InsertNewline,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorEvent {
    /// Atomic logical text/composition proposal (`Insert`, `Mark`, `Unmark`, `Cancel`, `Commit`, `DeleteSurrounding`).
    Edit {
        session: EditorSessionId,
        expected_rev: u64,
        edit: TextEdit,
    },
    /// Directional anchor and focus in UTF-8 bytes of the displayed document.
    Select {
        session: EditorSessionId,
        expected_rev: u64,
        selection: (usize, usize),
    },
    /// Semantic editing command executed identically across macOS, iOS, Linux, Web, and DST.
    Command {
        session: EditorSessionId,
        expected_rev: u64,
        command: EditorCommand,
    },
    /// Offer a non-composing editor key to app handlers (`on_key`) before OS text dispatch.
    KeyIntercept {
        session: EditorSessionId,
        expected_rev: u64,
        key: KeyEvent,
    },
}
```

- `TextEdit` gains `DeleteSurrounding { before_bytes: usize, after_bytes: usize }` (serving Wayland `zwp_text_input_v3::delete_surrounding_text` cleanly alongside `Insert`, `Mark`, `Unmark`, `Cancel`, `Commit`).
- Delete legacy `EditorEvent::ProposeEdit`, `ProposeSelection`, `Composition`, `Commit`, `CancelComposition`, and `KeyFallback` everywhere across `octara-model`, `octara-tree`, `octara-runtime`, `octara-host-native`, `octara-host-web`, `octara-inspect`, and tests (zero legacy aliases).
- **Single-Point Command Execution**: `octara-tree::EditorSlot` + `octara-runtime/src/cycle/text.rs` execute every `EditorCommand` in one shared implementation — guaranteeing identical `AtomicRange` pill deletion, BiDi/word navigation, vertical column memory, clipboard policy, and Undo coalescing across macOS, iOS, Linux, Web, and DST.

### 2.2 CJK Multi-Clause Composition Styling (`MarkedClause`)

Upgrade `TextEdit::Mark` and `octara-tree::editor::Composition` to carry optional native IME clause styling:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseStyle {
    /// Inactive candidate segment (thin underline).
    Clause,
    /// Active selected candidate segment (thick underline).
    ActiveClause,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkedClause {
    pub range: Range<usize>, // UTF-8 byte range relative to marked text
    pub style: ClauseStyle,
}
```

- Stored inline as `clauses: SmallVec<[MarkedClause; 4]>` (zero heap allocation for standard CJK phrases).
- Populated from AppKit `NSAttributedString` marked-text attributes (`NSMarkedClauseSegmentAttributeName` / `NSUnderlineStyleAttributeName`), UIKit `markedTextStyle`, and Wayland preedit cursor/clause ranges; rendered natively by `octara-pipeline`'s text decoration pass (thick active underline vs thin inactive underline).

---

## 3. Shared `HostTextBridge` (`crates/octara-host-native/src/text_bridge.rs`)

Consolidate all platform-agnostic native IME machinery into one reusable module in `octara-host-native`:

1. **Optimistic Local Mirror + Revision Reconciliation**:
   - Owns `Session { id: EditorSessionId, rev: u64, mirror: EditorMirror, predicted: VecDeque<(u64, EditorMirror)>, pending: Vec<Input>, caret: Option<Rect>, geometry: Option<TextGeometry>, traits: InputTraits, secure: bool, multiline: bool }`.
   - Synchronously answers immediate OS queries (`selectedRange`, `markedRange`, `attributedSubstring`, `surrounding_text`) within the same OS callback frame while queuing `EditorEvent`s for the next cycle, and reconciles cleanly when `sync_text_input` delivers the committed tree revision (rolling back/notifying OS delegate only on actual server-side rejection or external mutation).
2. **Shared Coordinate & Encoding Conversions**:
   - Lossless UTF-8 ↔ UTF-16 offset/range conversion over `EditorRope` (shared by macOS `NSRange`, iOS `UITextRange`, and future Windows TSF).
   - Bounded **≤ 4000-byte UTF-8 `surrounding_text(max_bytes: 4000)`** window centered on the caret on valid UTF-8 char boundaries, returning `(window_str, cursor_in_window, anchor_in_window)` for Wayland `zwp_text_input_v3`.
3. **Shared `TextGeometry` Queries**:
   - `first_rect(byte_range) -> Option<(Range<usize>, Rect)>` (IME candidate window anchoring),
   - `selection_rects(byte_range) -> Vec<SelectionBox>` (UIKit `UITextSelectionRect`, macOS Look Up / Writing Tools),
   - `character_index_at(point) -> Option<usize>` (macOS `characterIndexForPoint:` & UIKit `closestPositionToPoint:`).
4. **Thin OS Protocol Adapters**:
   - **`os/macos/editor.rs`**: Thin `NSTextInputClient` + `NSResponder` selector → `EditorCommand` mapper over `HostTextBridge` (supporting `DefaultKeyBinding.dict`, dead keys, Press-and-Hold accent popover, Dictation, `undo:`/`redo:`).
   - **`os/ios/editor.rs`**: Thin `UITextInput` / `UITextInputTraits` / `UIKeyCommand` adapter over `HostTextBridge` (replacing the separate `os/ios/draft.rs` mirror).
   - **`os/linux/wayland/`**: Thin `zwp_text_input_v3` adapter over `HostTextBridge` feeding `surrounding_text`, `content_type` (mapped 1:1 from `InputTraits`), `cursor_rectangle`, `preedit_string`, `delete_surrounding_text`, and `commit_string`.

---

## 4. Deterministic 4-Tier Key Routing Law

Delete `shell.rs::fallback_edit` and enforce one strict priority chain across all direct native hosts:

1. **Tier 1 — Active IME Composition (`bridge.is_composing()`)**:
   - Native OS input method receives the key **first** (`Enter`, `Escape`, `Space`, `Backspace`, `1..9`, arrows control the active candidate session and are never hijacked by app submit/dismiss shortcuts).
2. **Tier 2 — App Shortcuts & `KeyIntercept` (non-composing only)**:
   - Global app chords (`Cmd+K`) and focused `on_key` interceptors run before default text editing; if consumed, stop.
3. **Tier 3 — Native OS Text System**:
   - `NSTextInputContext::handleEvent` / `UITextInput` / Wayland `zwp_text_input_v3` + XKB dead-key composition translate the key into `EditorEvent::Edit` or `EditorEvent::Command`.
4. **Tier 4 — Core Default Execution**:
   - `octara-tree` / `octara-runtime` executes the resulting `EditorEvent` in one unified path.

---

## 5. Clipboard Dependency Hygiene (`arboard` Removal)

- Remove `arboard` completely from root `Cargo.toml`, `crates/octara-host-native/Cargo.toml`, and `Cargo.lock`.
- Every Tier-1 native host uses its OS clipboard directly (`NSPasteboard`, `UIPasteboard`, Wayland `wl_data_device`, X11 `x11rb` ICCCM selections).

---

## 6. Hard Exit Gates (`0.0.10` Wave D4)

1. **Zero Legacy `EditorEvent` Variants & Zero `arboard`**:
   - `git grep -E "EditorEvent::(ProposeEdit|ProposeSelection|Composition|Commit|CancelComposition|KeyFallback)" crates/` returns **0 hits**.
   - `git grep -i arboard Cargo.toml Cargo.lock crates/ modules/` returns **0 hits**.
2. **Shared `HostTextBridge` Verified**:
   - `os/ios/draft.rs` deleted; macOS, iOS, and Linux Wayland all build atop `octara-host-native/src/text_bridge.rs`.
   - Unit tests cover optimistic prediction reconciliation, rejected-proposal resync, UTF-8↔UTF-16 emoji/surrogate pairs, 4000-byte Wayland `surrounding_text` boundary slicing, and `DeleteSurrounding`.
3. **4-Tier Key Priority & Multi-Clause CJK Verified**:
   - Active IME composition prevents `Enter` / `Escape` from leaking to app `on_key` / modal dismiss on macOS, iOS, Linux Wayland, Web, and DST.
   - Multi-clause `MarkedClause` (`ActiveClause` vs `Clause`) underline rendering verified in pipeline + CPU/GPU parity tests.
   - Unified `EditorCommand` execution verified identically across macOS AppKit selectors, iOS Simulator, Linux Wayland, Chromium DOM, and DST (including `AtomicRange` pills, Undo/Redo, and clipboard).
4. **Full Release Matrix**:
   - `python3 tools/check_layers.py`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo test -p octara-cli --doc`, DST performance gates, default + WebGPU + 4 module Wasm checks, `aarch64-apple-ios-sim`, Linux check, and live macOS AppKit / iOS Simulator / Chromium DOM regressions all pass.
