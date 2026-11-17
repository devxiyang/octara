# Shared semantic resolution

Status: C2a node-local resolution and C2b independent input hints are implemented on `init`.
Broader composite naming remains open; this is not a full accessible-name computation algorithm.

`kernel/semantics.rs` is the owner of the node-local name rule. AX, Inspector, name lookup,
diagnostic descriptions and missing-name checks consume it rather than each reconstructing
label/text fallback. It is a borrowed projection of current Kernel facts, with no retained cache
or new public primitive. `Kernel::label` still reads only the explicit label.

| Fact | Rule |
| --- | --- |
| Explicit label | Wins, including an explicit empty string |
| Absent label on document text | Nonempty own text supplies the name |
| Editable or other control value | Never supplies the name |
| Input placeholder | Independently exported display hint; never supplies a name or description |
| Empty document text | No fallback name |
| Description | Explicit description wins, including empty; otherwise tooltip unless it equals the resolved name |
| Visibility | Name lookup follows the same visible subtree rule as AX, including clipped internal links |

Adapters preserve their output conventions: AccessKit static text encodes implicit text as
`value`, while Inspector records it as `name`. An explicit label remains an AX label. Dynamic
text/label updates use existing semantic invalidation; neither targets nor editing owners change.

Missing-name diagnostics require non-whitespace names. Explicit empty labels suppress text
fallback but cannot satisfy the naming requirement. Existing descendant-text checks for compound
controls remain separate from the node-local name: they ignore hidden descendants and honor
text labels. These checks do not claim that every platform derives an identical aggregate name.
Input controls require their own name regardless of their value or surrounding text.

This follows the recorded [Exact review](../research/agent-exact-architecture-review.md): share
pure rules while retaining host-specific AX encoding. The [Rune review](../research/agent-rune-architecture-review.md)
also showed why naming an outer Field cannot establish the actual editor's name. No implicit
label transfer or new Root/Part protocol is introduced here.

The regression compares AX, Inspector, name lookup and diagnostics for Chinese document text,
explicit and empty labels, unnamed nonempty inputs, hidden subtrees, duplicate tooltips and
live renaming. It verifies incremental AX updates and stable Agent target identity. Real OS
screen-reader/IME verification is separate and has been deferred at the user's request.

Validation (2026-11-17): **902 workspace tests passed, 0 failed, 3 existing ignored
doctests**. Workspace all-targets Clippy, formatting, diff and dependency-layer checks passed.
