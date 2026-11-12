# The text core

Based on the [research](../research/text.md). Status: implemented (steps 1 to 4). The [core](core.md)'s step 3: rich
text, blocks laid out and painted one at a time, a selection across blocks and rows of a
virtual list, and accessible text. Designed for the AI chat app: messages streaming in as
Markdown, long transcripts, select and copy across messages.

## The engine

Parley (Linebender) lays text out, its fontique collects fonts, swash rasterizes glyphs. Not
the platform's text engines: the same layout in every window, headless run and renderer is
what lets agents and tests see what people see.

```
view       text("..") / text(rich()..) / text(&text_buffer)
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
  `text(..)` takes a `Rich` like a string, fixed or from a closure; `TextBuffer` stays plain (a
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
  the block: the code block element holds one text per line, so a stream into it reshapes its
  last line.

## Paced text

`paced(&source)` is a `TextBuffer` that follows `source`, showing what is appended to it over a few
frames instead of in bursts (a model's reply arriving in chunks): each frame shows a quarter of
the backlog (at least 8 bytes, cut on a character boundary), so a burst of a few hundred bytes
shows within about a quarter of a second and a trickle shows as it comes. Edits other than
appends, and Reduce Motion, show at once. It is motion: pending work lists it while it is behind,
`settle_motion` lands it, `run_until_idle` runs it out. Pass it where the source would go
(`text(paced(&reply))`, `markdown(&paced(&reply))`).

## Markdown (octara-kit)

- `octara_kit::markdown::markdown(source)`: a string, a closure (followed), or a `TextBuffer` (a
  stream: its end is repaired). pulldown-cmark (strikethrough, tables, task lists) into blocks:
  headings, paragraphs (strong, emphasis, strike, inline code, links as runs), code blocks,
  lists (ordered from their start, unordered, tasks), quotes, rules, tables; HTML shows as
  written. Soft breaks are spaces.
- Each block is an element under a stable key: top-level blocks `b{i}` (also the blocks inside a
  list item or quote), list items `i{j}`, code lines `l{n}`, table rows `r{k}` and cells `c{j}`.
  A block whose kind changes is made again; one that changed updates its text. Roles: `Heading`
  with its level, `Paragraph`, `List` / `ListItem`, `Code`, `Blockquote`, `Table` / `Row` /
  `Cell`; a task's box is a `CheckBox`. Markers are not selected and not read.
- A code block: its language, a copy button (`write_to_clipboard`), and one text per line, so a
  stream into it re-lays out its last line.
- **Streaming**: when the source grows, parsing restarts at the last top-level block whose first
  line is finished (the start of a line may still change what block it starts); earlier blocks
  keep their keys and nodes, and the list is synced from the first block that changed. A source
  that changed otherwise, or holds `]:` (a reference definition or footnote may change links
  anywhere), parses whole.
- **The end of a stream** is repaired for the parse only, never in the source, within the last
  inline run (a paragraph until a blank line, a heading's line, a table cell): emphasis, strong
  and strike opened at a word start, code spans and links are closed; a trailing run of markers,
  a `[`, a trailing `!` and openers with nothing after them are left out; `snake_case`, `2 * 3`
  and a single `~` stay text. A repair whose parse would still show a marker falls back to the
  parse without it. Trailing `|` lines wait for their delimiter row; a header of two or more cells
  starting a block shows as a table's head meanwhile. A finished paragraph keeps its markers as
  written; an open code fence shows as code.
- **Copy**: `mod+c` copies the text, joined by roles (see Selection). `edit.copy_markdown`
  (`mod+alt+c`) asks the elements that copy as Markdown (`.copy_markdown(f)`; `markdown` is
  one): a selection inside one block copies its text; one touching several blocks copies their
  source whole, from the first block's line to the next block after the last; one covering every
  block copies the source as written.
- **Assistive technology**: the document is a `Document` of text runs (see Accessibility). A
  stream adds a live region (polite) whose label is the block that just finished, so a screen
  reader hears each block once, not each token.

## Selection

- **A selectable region** (`.selectable()` on a container, the kit's `markdown` and a chat
  transcript use it): its texts can be selected as one document. Texts outside a region, and a
  region's texts marked `.select_none()` (list markers, buttons' labels), are not. Regions
  inside a region are one document with it: the outermost holds the selection, so a selectable
  transcript of Markdown messages selects across messages, and copying as Markdown still asks
  each message for its part (the nearest `copy_markdown` above each text).
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
  capped at 10 000 rows. Texts join by the role of what holds the second below where their paths
  part: a cell by a tab; a row or a list item by a newline; a paragraph, heading, list, table,
  quote or code block by a blank line; lines of a code block by a newline; otherwise by a newline,
  and rows of a list by a blank line.
- **Lists**: each row's root carries its key; a virtual list registers on the window how it
  orders keys (through its weighted mirror) and how it builds a row off screen. Selecting all
  uses two points outside every text (before the first, after the last), so it covers rows that
  were never mounted. `mod+c` copies that plain text, always. `edit.copy_markdown` (`mod+alt+c`)
  copies Markdown: an element can give its own for the selected texts in it (`copy_markdown`;
  `markdown` gives the touched blocks' source, whole).
- **Agents and tests**: `Window::select(from: (id, offset), to: (id, offset))`,
  `Window::select_all(region)`, `Window::selected_text()`; the snapshot's region nodes carry
  `selection` (`[from, to]` as test ids and offsets); requests `select` and `copy`.

## Accessibility

- A selectable region is a `Document` (unless it states a role, or is inside another region). Its texts keep their roles
  (a `Label` by default; `.role(Role::Heading).level(1)` for a heading) and hold `TextRun`
  children, one per laid-out line: a hard break ends the run before it, and a line longer than
  255 characters is split into runs on the same line (`next_on_line`), since AccessKit's word
  starts are bytes. Each run carries `character_lengths` (graphemes), `word_starts` (where a
  segment that is not blank starts), and `character_positions` / `character_widths` from the
  layout's caret stops; its bounds are its line's. A text larger than 64 KB, a text marked
  `select_none`, and text outside regions have no runs.
- A run's AccessKit id is its text's slot with the run's number above it; an action on a run
  goes to its text.
- Static text is a `Label`'s value (AccessKit reads a label's text there); other roles take the
  text as their name.
- The region's selection is the `Document`'s `text_selection`: anchor and focus where they are
  mounted, else where the mounted part of the selection begins or ends. Copying still takes the
  whole selection. Assistive technology sets it with `SetTextSelection` on the region.
- Runs derive from the text's kept lines and are sent with their text: a stream or a reflow
  sends the text, its runs, and its region when the selection ends in it.
- **Live regions**: `.live(Live::Polite)` (or `Assertive`) on an element; the kit's streaming
  message sets it, and what assistive technology reads is the block that finished, not each
  token (the message's label changes when a block commits).

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
