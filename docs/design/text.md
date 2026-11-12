# The text core

Based on the [research](../research/text.md). Status: confirmed; not implemented yet. The [core](core.md)'s step 3: rich
text, blocks laid out and painted one at a time, a selection across blocks and rows of a
virtual list, and accessible text. Designed for the AI chat app: messages streaming in as
Markdown, long transcripts, select and copy across messages.

## Rich text

- A text node holds one string and **runs**: contiguous byte ranges covering it, each a style
  patch over the node's inherited text style: weight, slant, family (or `mono`), size, color and
  background (tokens or colors), underline, strikethrough, and a **link** (a URL or a name the
  app's handler gets).
- Built with `rich()`, a value: `rich().text("See ").link("the docs", url).text(" or ").code("cargo
  run")` (`bold`, `italic`, `code`, `color(..)`, `span(text, |s| s.weight(600).underline())`).
  `text(..)` takes a `Rich` like a string, fixed or from a closure; `LiveText` stays plain (a
  stream of prose is Markdown's job, below).
- Layout: the engine builds the paragraph with Parley's style-run builder (a style table and the
  runs); its cache keys a node's paragraph by its text, its runs and the width, as today by text
  and width. Inline boxes (pills, images in a line) come later, on Parley's `InlineBox`.
- Links: hit-tested on the link's line boxes (only its glyphs take the pointer); hover and press
  states per link; `.on_link(|link: &str| ..)` on the text or an ancestor gets a click or Enter;
  the cursor is the pointing hand (with the platform boundary's cursor). To assistive technology
  and the inspector a link is a child `link` node with its bounds; Tab reaches it.

## Blocks

- A block (paragraph, heading, code block, list item, quote, table cell) is an element; blocks
  are laid out and painted one at a time, so appending to the last block of a message re-lays out
  that block only (the engine's cache already keeps the others).
- Painting keeps each text node's glyphs: the kernel stores the shaped runs a node painted with,
  keyed by its text version, runs, style and width, and reuses them until one changes (a growing
  text recycles its buffer). Assembling the display list from kept parts is the next step's
  ([incremental frames](core.md#incremental-frames)); this one makes a stream's per-frame shaping
  cost its tail.
- A very long single block (a code fence of thousands of lines) is laid out per hard line inside
  the block: the code block element holds one text per line (virtualized when long), so a stream
  into it reshapes its last line.

## Markdown (octara-kit)

- `markdown(source)` (a string, a closure or a `LiveText`): pulldown-cmark into blocks, each a
  kit element with stable keys (`b{i}`, list items `i{j}`, their blocks `b{k}`), inline styles
  as runs. Headings, paragraphs, emphasis, inline code, links, code blocks (monospace, a copy
  button), lists (ordered, unordered, tasks), quotes, rules, tables (GFM); images when images
  load.
- Streaming: when the source grows (it starts with the last one), parsing restarts at the last
  top-level block that starts on a finished line; earlier blocks keep their keys and are not
  touched. A reference definition or footnote makes a full parse. The tail is repaired for the
  parse only (closing emphasis, code spans and strike opened at a word start, completing a
  partial link, holding back a lone table row and a trailing run of markers), never in the
  source.
- Each block maps rendered byte ranges to source ranges, for `edit.copy_markdown`.

## Selection

- **A selectable region** (`.selectable()` on a container, the kit's `markdown` and a chat
  transcript use it): its texts can be selected as one document. Texts outside a region, and a
  region's texts marked `.select_none()` (list markers, buttons' labels), are not.
- **A selection is data**, the region's: an anchor and a focus, each a `TextPoint { path, offset
  }`. The path is stable across a row unmounting and mounting again: the keys of the `each` /
  `virtual_each` rows between the region and the text, then the text's ordinal among the
  selectable texts of that row in tree order. The offset is a byte offset in the text.
- **Order is the document's**: rows by their list's key order, texts by tree order, so a
  selection between two rows covers every row between, mounted or not. Nothing is pinned: a row
  scrolling out keeps its part of the selection, and draws it again when it comes back.
- **What survives a change**: text appended after an end keeps it (a streaming message); a text
  that changes under an end clamps the offset; a row removed that holds an end ends the
  selection there (the other end stays); a region unmounted clears it.
- **Interaction** (a pointer recognizer in the region, the field's rules otherwise): press and
  drag selects across texts (scrolling the region's scroll container at its edges), double press
  a word (Unicode word boundaries), triple press a block, Shift+press extends, `mod+a` selects the
  region, `mod+c` copies, Escape or a press elsewhere clears. A press on a link, a button or a
  field acts as it does; a drag starting on a link selects. Text fields keep their own selection;
  focusing one clears the region's.
- **Painting**: each mounted text draws its part's highlight rects (Parley's selection
  geometry) behind its glyphs, in a new `Token::Selection` (the one text fields draw their selection in too).
- **Copy** reads the source: mounted texts their strings; rows between that are not mounted are
  built view-only (the row's view run in a detached scope, its texts read, the scope disposed),
  capped at 10 000 rows and 4 MiB. Texts join with a newline, blocks (paragraph, heading, code
  block, list item, quote) with a blank line, table cells with a tab; list items get their
  markers. `mod+c` copies that plain text, always. `edit.copy_markdown` (`mod+shift+c`, and the
  context menu) copies Markdown: a region can supply its own source for the blocks it covers
  (`markdown` gives the touched blocks' source; partly selected blocks are cut at the selection
  through their source maps).
- **Agents and tests**: `Window::select(from: (id, offset), to: (id, offset))`,
  `Window::select_all(region)`, `Window::selected_text()`; the snapshot's region nodes carry
  `selection` (`[from, to]` as test ids and offsets); requests `select` and `copy`.

## Accessibility

- A selectable region is a `Document` node: its blocks are `Paragraph`, `Heading` (with level),
  `ListItem`, `Code` …, each holding `TextRun` children, one per laid-out line, with
  `character_lengths`, `word_starts` (within AccessKit's limits) and, cheaply from the layout,
  `character_positions` and `character_widths`. A text larger than 64 KB has no runs (its value
  only).
- The region's selection is the `Document`'s `text_selection`, clamped to the runs that exist
  (rows not mounted have none); copying still takes the whole selection.
- Text outside regions stays a `Label` with its value.
- **Live regions**: `.live(Live::Polite)` on an element; the kit's streaming message sets it, and
  what assistive technology reads is the block that finished, not each token (the message's
  label changes when a block commits).

## Steps

1. Rich text: runs, `rich()`, the engine's style runs, links (hit testing, states, `on_link`,
   link nodes); per-node glyph reuse. Proof: a link in a paragraph is pressed by test id and by
   point; restyling a run re-lays out that text only; a stream's per-frame shaping is its tail.
2. Selection: regions, `TextPoint`s, document order, gestures, keyboard, painting, copy with
   unmounted rows, `select` / `copy` requests. Proof: a selection from row 3 to row 900 of a
   virtual list survives scrolling both ends out and back and copies all 898 rows' text; an
   append to the focus text keeps it.
3. Accessibility: `Document`, block roles, text runs, the selection, live regions. Proof: the
   accessibility tree of a selected region has runs with lengths summing to their values and a
   selection whose ends are runs.
4. Markdown in the kit, with streaming and source copy. Proof: streaming a 2 000-token answer
   re-parses only the tail block per token and keeps every earlier block's node; a code fence
   arriving token by token shows its content as code while open and the text after its close as
   prose.
