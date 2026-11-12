# Research: the text core

Status: research notes (2026-11) for [the text core](../design/text.md): rich text, block layout
for streaming, selection across blocks and virtualized rows, accessibility of text. Sources: the
reference clones (`~/code/archive/reference/`: `rune`, `zed` (GPUI only, no `markdown` crate),
`exact`), the crate sources of AccessKit 0.25.2 and Parley 0.12.0, Flutter and Compose source.

## Rich text

| | Representation | Inline interaction |
| --- | --- | --- |
| Rune | one `String` per text plus side lists of byte ranges: `Span { start, end, weight, mono, size, color, bg, underline, italic }` (`text/span.rs:11`); brushes are indices into the spans | `Link { start, end, id, role, label, hover, pressed, focused }` (`span.rs:210`); a link's line boxes come from the layout, so only its glyphs take the pointer; links are their own elements to tools |
| GPUI | `StyledText { text, runs: Vec<TextRun { len, font, color, background, underline, strikethrough }> }`: runs tile the string; `with_default_highlights(style, [(Range, HighlightStyle)])` | `InteractiveText::on_click(ranges, f(range_ix))` (press and release in one range), `on_hover`, `tooltip`, pointing-hand cursor |
| Compose | `AnnotatedString`: one string, `spanStyles` and `paragraphStyles` by range, typed annotations; `LinkAnnotation.Url` / `Clickable` | per annotation |
| Flutter | a tree of `TextSpan`s with inherited styles, `WidgetSpan` for inline widgets | recognizers on spans |
| Exact | the Markdown source is the only value; `style(source)` gives paragraphs, spans (bitflags), hidden and replaced ranges, atoms (pills); offsets in UTF-16 | the platform's text view |

Parley 0.12 builds mixed styles from one string three ways: `ranged_builder` (`push(prop,
range)`, splits ranges itself), `tree_builder` (spans, collapses whitespace), `style_run_builder`
(a style table and contiguous runs over `0..len`: the cheapest for flat runs). `InlineBox` holds
a slot at a byte index. `Cursor::from_point`, `Selection::{from_point, word_from_point,
extend_to_point, geometry_with}` hit-test and draw a selection, all within one layout.

## Markdown and streaming

- **Rune**: blocks, not one text: pulldown-cmark into `Block::{Para, Heading, Code, List, Quote,
  Rule, Table, ..}`, each its own element, keyed `b{i}` / `i{j}`; "appending to a stream only
  ever changes the last block or adds blocks after it, so earlier blocks keep their keys"
  (`rune-kit/src/markdown.rs:18-24`). A growing source re-parses from the last top-level block
  that starts on a finished line (`markdown/cache.rs:89`), a full parse when reference
  definitions appear. The tail is repaired for the parse (unclosed emphasis and code spans
  closed, partial links completed, a lone table row held) without touching the source
  (`markdown/stream.rs`). Each block maps source ranges to rendered ranges, so a whole-block
  selection can copy the Markdown source. New words can fade in.
- **Streamdown** (Vercel): block lexing plus memoized blocks; a block is frozen when a blank line
  ends it and two more follow; the last two always re-lex; footnotes or reference definitions
  keep the document one block. **remend** closes `**`, `` ` ``, `~~`, `$$` and unfinished links on
  a display copy. **mdstream** (Rust): one pending block, committed blocks immutable by id,
  invalidations when a late definition lands.
- Pitfalls: an open code fence swallows everything after it; a next list item or blank line
  changes a list; a table header is a paragraph until its delimiter row; a setext underline turns
  a paragraph into a heading; late reference definitions change earlier blocks.
- **Costs** (Rune, `research/27-long-note-editing.md`, M4 Pro): reshaping is about 0.22 ms per
  KB; one field for a 50 KB note costs 12 ms per keystroke, one field per block 0.82 ms. A single
  huge block (a long code fence) stays the worst case; per-line sub-layouts are planned there, not
  built.
- **Caches**: Rune keys a text's layout by its text, spans, style, scale and width, drops a
  superseded layout at the end of the commit ("as typing or streaming makes"), and flattens
  glyphs once per layout into a bounded cache that recycles a growing text's buffer; painting
  walks only the visible lines. GPUI keeps line layouts for two frames and reuses a subtree's.

## Selection across texts and rows

- **Flutter** (`SelectableRegion`, `SelectionContainer`, `Selectable`): positions are per
  selectable (index and local offset); order is geometric (screen position); copy concatenates
  the registered selectables' plain text; a lazily built child that scrolls away unregisters, so
  its text is not copied and an anchor there loses its geometry.
- **Compose** (`SelectionContainer`): anchors are `(selectableId, offset)`; order by layout
  position; copy builds an `AnnotatedString` joined by `\n`; selected items are pinned so lazy
  containers keep them composed.
- **SwiftUI**: `.textSelection(.enabled)` selects within one `Text`; no cross-view selection.
- **Rune**: texts are `selectable`, a `selection_group` makes them one selection, texts between
  the ends selected whole, unselectable ones (list markers) skipped. An end in a virtual list is
  `VirtualEnd { list_path, row_key, row_index, rel_path, offset }`; the two end rows are pinned
  mounted; rows between are hashed when the selection is made and the selection is dropped if
  one changes; an append after the selected bytes keeps the selection. Copy builds unmounted rows
  view-only (no layout, mount or GPU), capped at 10 000 rows and 4 MiB, joins texts by newline,
  tab between side-by-side texts or blank lines where asked, with list prefixes; whole Markdown
  blocks can copy as source. Word and paragraph units use ICU segmenters.
- **GPUI** (framework layer): no selection across elements.

## Accessibility of text

- AccessKit's text model: `Role::TextRun` leaves with `value`, `character_lengths` (bytes per
  user-perceived character, a hard break counted at the end of its line), `word_starts`,
  optional `character_positions` / `character_widths`; `TextPosition { node: a run,
  character_index }`; `TextSelection { anchor, focus }` set on the range's root. Text ranges are
  supported on text inputs and on `Label`, `Document` and `Terminal` nodes that have runs; runs
  under intermediate nodes (paragraphs) count for their root, so one `Document` with paragraph
  and heading children holding runs is one range, and a selection across two separate labels
  cannot be expressed. macOS (`NSAccessibility` text attributes, UTF-16 converted by the
  adapter), Windows (`ITextProvider`) and AT-SPI (`Text`) implement ranges; macOS and AT-SPI
  announce live regions. A position needs a run's node, so unrealized rows cannot hold one.
- Rune emits runs per laid-out line for fields and selectable text, omitted above 64 KB;
  `word_starts` is capped by AccessKit's u8 index; links become Link children with layout
  bounds. Its Markdown sets no live region. GPUI exposes text as a `Label` with the whole string.

## Conclusions

1. Flat runs over one string per block (Compose, GPUI, Rune), built with Parley's style-run
   builder; links as ranges with their own state.
2. Blocks are elements; Markdown renders to them with stable keys and re-parses only from the
   last block, with a repaired display tail.
3. A selection is data: two ends addressed by stable keys and byte offsets, ordered by document
   order (list keys, tree order), not geometry; rows can leave and come back without pinning;
   copy reads the source, building unmounted rows when needed.
4. A selectable region is one AccessKit `Document` with runs per line, its selection clamped to
   the realized runs; a streaming message is a polite live region announced by block, not token.
