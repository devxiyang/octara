# The text core

Based on the [research](../research/text.md). Status: confirmed; steps 1 (rich text and links), 2 (selection) and 2.5 (the engine) implemented. The [core](core.md)'s step 3: rich
text, blocks laid out and painted one at a time, a selection across blocks and rows of a
virtual list, and accessible text. Designed for the AI chat app: messages streaming in as
Markdown, long transcripts, select and copy across messages.

## The engine

Parley (Linebender) lays text out, its fontique collects fonts, swash rasterizes glyphs. Not
the platform's text engines: the same layout in every window, headless run and renderer is
what lets agents and tests see what people see.

```
view       text("..") / text(rich()..) / text(&live_text)
kernel     a text node: its string, runs, version, and a TextLayout handle
             (glyphs, lines, links, selections and accessibility runs derive from it)
engine     TextEngine::layout(text, style, runs, previous) -> TextLayout
             a shape cache by content: (paragraph text, style, runs) -> a shaped paragraph,
             shared by every text and window showing it, bounded by bytes (least recently
             used out first, a superseded streaming tail out at once)
Parley     shaping, bidi, line breaking; Layout<u16> (the brush is the run's number)
renderers  glyph runs in the display list; swash rasterizes into an atlas both renderers share
```

- **One owner**: a text node owns its `TextLayout` (an `Rc` the engine made): its paragraphs,
  each a shaped paragraph from the shape cache broken at the node's width, with the sizes it was
  measured at. Measuring, glyphs, lines and every geometry come from it; it is made again only
  when the node's version (its string or runs), style or the fonts change, reusing the previous
  layout's unchanged paragraphs. Nothing else holds per-node text state, and a node's layout
  goes with the node.
- **By content**: a paragraph shaped once is found again by its text, style and runs: a row of a
  virtual list scrolling back, a label repeated in every row, the same text in two windows reuse
  it. Only what changes shape is in the key (family, size, weight, slant per run); colors and
  decorations are drawn from the runs at paint time.
- **Increments**: a paragraph (between `\n`s) is the unit of shaping; a block of Markdown is a
  node; a code block is a node per line. Appending to a stream reshapes its last paragraph; a
  new width only breaks lines again.
- **Threads**: shaping runs on the main thread (about 0.2 ms per KB, Rune measured). A layout is
  an immutable handle, so shaping a new block off the main thread, showing its estimated height
  meanwhile, can come later without changing the model.
- **Fonts**: the theme's families are `system-ui` and `monospace`. Headless (the default) they
  are the bundled Inter and IBM Plex Mono, so a run measures the same on every machine; a
  window's engine (`use_system_fonts`) resolves them to the platform's UI and monospace fonts
  (San Francisco, Segoe UI, …) with the system's fallback for scripts. A headless run can use
  the system fonts too, to match a window's pictures on that machine.
- **Input**: text fields lay out with the same engine; their editing model (selection, history,
  composition) is the runtime's. Platform text protocols (NSTextInputClient, TSF,
  text-input-v3) come with the owned backends.
- **Limits**: one paragraph without line breaks is shaped whole (100 KB: about 16 ms); mixed
  sizes under `line_clamp` measure the clamped height approximately; no hyphenation, no vertical
  text; inline boxes (pills, images in a line) come later on Parley's `InlineBox`.

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
- Links are nodes: each link run gets a `link` child of its text (kind `Link`), outside layout:
  its frame is the box around the line pieces its range covers in the text's lines, and only
  those pieces take the pointer. So a link has what every element has: focus (Tab reaches it,
  Enter presses it), hover and press states (the run draws in `AccentHover` then), a test id
  (`Span::id`), a name (its words) and its target as its value, the `press` action for agents
  and `Click` for assistive technology. `.on_link(|target: &str| ..)` on the text or an
  ancestor gets every press. The pointing-hand cursor comes with the platform boundary's
  cursor.
- Drawing: a run's color is its span's, else a link's accent, else the text's; backgrounds are
  drawn behind the glyphs around their ascent and descent; underlines (links too) and
  strikethroughs over them at the run's baseline.
- Kept work: a text's glyphs are kept until its version (bumped by every change of its string
  or runs), its width, its style or the fonts change; its lines (what links and selections are
  placed by) the same. A color change re-draws without reshaping; a weight change reshapes that
  paragraph.

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
- **Painting**: after each operation the window marks the range each mounted text of the
  region covers; the kernel draws it behind the glyphs from the text's kept lines (a line's
  break included), in the accent at 30 % as fields draw theirs.
- **Copy** reads the source: mounted texts their strings; rows between that are not mounted are
  built view-only (the row's view run in a detached scope, its texts read, the scope disposed),
  capped at 10 000 rows. Texts join with a newline, list rows with a blank line (Markdown's
  blocks, table cells and list markers join by their roles in step 4).
- **Lists**: each row's root carries its key; a virtual list registers on the window how it
  orders keys (through its weighted mirror) and how it builds a row off screen. Selecting all
  uses two points outside every text (before the first, after the last), so it covers rows that
  were never mounted. `mod+c` copies that plain text, always. `edit.copy_markdown` (`mod+shift+c`, and the
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
2.5. The engine: text layouts owned by their nodes, the shape cache by content (bounded,
   superseding), `system-ui` and `monospace` resolved per engine. Proof: a virtual list's row
   scrolling out and back shapes nothing; a thousand rows of one label shape it once; a stream
   keeps the cache's bytes flat.
3. Accessibility: `Document`, block roles, text runs, the selection, live regions. Proof: the
   accessibility tree of a selected region has runs with lengths summing to their values and a
   selection whose ends are runs.
4. Markdown in the kit, with streaming and source copy. Proof: streaming a 2 000-token answer
   re-parses only the tail block per token and keeps every earlier block's node; a code fence
   arriving token by token shows its content as code while open and the text after its close as
   prose.
