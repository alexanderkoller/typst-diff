# typst-diff — Technical Reference

## Table of Contents

1. [Overview](#overview)
2. [Crate structure](#crate-structure)
3. [Pipeline walkthrough](#pipeline-walkthrough)
4. [Module reference](#module-reference)
   - [world](#world)
   - [eval](#eval)
   - [diff](#diff)
   - [annotate](#annotate)
   - [render](#render)
   - [diag](#diag)
5. [Key data structures](#key-data-structures)
6. [Algorithms in depth](#algorithms-in-depth)
   - [Block extraction](#block-extraction)
   - [Block-level LCS diff](#block-level-lcs-diff)
   - [Edit zone pairing](#edit-zone-pairing)
   - [Word-level diff](#word-level-diff)
   - [Similarity metric](#similarity-metric)
7. [Annotation strategy](#annotation-strategy)
8. [Rendering and layout convergence](#rendering-and-layout-convergence)
9. [Git revision mode](#git-revision-mode)
10. [Performance notes](#performance-notes)
11. [Limitations and known edge cases](#limitations-and-known-edge-cases)

---

## Overview

typst-diff compares two Typst documents and produces a PDF that highlights
word-level changes: insertions in green, deletions in red strikethrough.

Unlike source-level diffing tools (e.g. `latexdiff`), typst-diff operates on
the *evaluated* Content trees produced by the Typst compiler. This means:

- `#include` directives and user-defined functions are fully expanded before
  diffing.
- Structural differences caused by macro expansion (not visible in source)
  are captured.
- The output is a valid Typst PDF, rendered with the new document's fonts and
  assets.

The cost is that eval is required for both documents, even for trivially small
changes.

---

## Crate structure

```
src/
├── lib.rs             — library root; public re-exports
├── main.rs            — CLI binary: argument parsing + orchestration
├── world.rs           — World trait implementation (filesystem + fonts)
├── eval.rs            — Content tree extraction from a World
├── diff.rs            — Two-level block+word diff
├── content_slots.rs   — Named sub-positions inside structured elements
├── annotate.rs        — Build annotated Content from a DiffResult
├── render.rs          — layout_document + typst_pdf → Vec<u8>
└── diag.rs            — Diagnostic formatting (file:line:col messages)
```

The crate is both a library and a binary. `src/lib.rs` exports the public API
so integration tests in `tests/` can call into individual pipeline stages.

---

## Pipeline walkthrough

```
old/main.typ ──► SystemWorld A ──► eval_to_realized_content ──► old_content: Content
                                                                       │
new/main.typ ──► SystemWorld B ──► eval_to_realized_content ──► new_content: Content
                                                                       │
                                                        diff::diff_content(&old, &new)
                                                                       │ DiffResult
                                                        annotate::build_annotated_content
                                                                       │ Content
                                               render::render_to_pdf(content, world_b)
                                                                       │
                                                                    diff.pdf
```

The new document's world (World B) is used for both annotation and rendering
because fonts and binary assets resolve relative to it. World A is only used
during evaluation and is discarded afterwards.

---

## Module reference

### world

`SystemWorld` implements the Typst `World` trait with filesystem-backed file
loading. Each instance is anchored to a root directory derived from the entry
file.

**Key design decisions:**

- Virtual paths in Typst (e.g. `/chapter.typ`) are resolved by stripping the
  leading `/` and joining with the root directory. This mirrors `typst-cli`'s
  behaviour.
- Source files and binary files are cached in separate `Mutex<HashMap<FileId,
  _>>` maps so repeated lookups within a single compilation are free.
- Fonts are loaded via `typst-kit`'s `FontSearcher`, which searches system
  font directories and embeds a set of fallback fonts. The book and font
  list are stored on `SystemWorld` and served directly via `book()` and
  `font()`.
- `today()` returns `None`; the diff output does not embed a document date.

### eval

Two public functions with different levels of expansion:

**`eval_to_content(world)`**
Calls `typst_eval::eval` on the main source file and returns `module.content()`.
The result is a shallow Content tree; some Typst elements (e.g. paragraphs,
headings) are still represented as their unresolved types. This is sufficient
for the unit tests and for the integration tests that call `diff_content`
directly, because the diff algorithm works on the raw element types.

**`eval_to_realized_content(world)`**
Used by the CLI. Runs two additional passes over the Content tree:

1. **`layout_introspector`** — performs a full layout pass (up to 5 iterations
   for convergence) to build a stable `Introspector`. The introspector records
   counters, labels, and references that may affect realized content.
2. **`realize_to_content`** — calls `ROUTINES.realize` with
   `RealizationKind::LayoutDocument`, which expands all lazy Typst show rules,
   counter steps, and document structure into a flat sequence of `(Content,
   StyleChain)` pairs.

Before realization, `collect_preserved_by_span` records every `EquationElem`
and every node that `content_slots::is_slot_container` recognises (lists,
tables, figures, etc.). These nodes would become opaque layout output after
realization. After realization, `restore_preserved` substitutes them back by
span, recursing through `SequenceElem`, `StyledElem`, and `ParElem`.

The realized output is then re-wrapped into a `Content::sequence` where each
item carries its inline styles (non-page styles), and the whole sequence is
wrapped with root page styles. Page styles on individual items are extracted
and attached as `DiffBlock::page_styles` downstream.

The realized pipeline is heavier but produces more accurate diffs for documents
that use Typst's document structure features (chapter counters, auto-generated
headings, etc.).

### content_slots

Defines the *slot* abstraction: a named, text-bearing sub-position inside a
structured `Content` element, identified by a `Vec<SlotStep>` path from the
element root.

**`extract_slots(content)`** walks a `Content` tree and returns every leaf slot
in document order. Returns an empty `Vec` for elements with no addressable slots
(plain text, headings, raw blocks, etc.). The full set of handled containers is:
`ListElem` / `ListItem`, `EnumElem` / `EnumItem`, `TermsElem` / `TermItem`,
`FigureElem` (body + caption), `FootnoteElem`, `QuoteElem`, `TableElem`,
`GridElem`, `StackElem`, and single-body wrappers (`AlignElem`, `PadElem`,
`PlaceElem`, `ColumnsElem`, `BoxElem`, `BlockElem`, `RectElem`, `CircleElem`,
`EllipseElem`).

**`is_slot_container(content)`** returns `true` for all of the above types. Used
in `eval.rs` to identify nodes that must be captured before realization (see
[eval](#eval)).

**`replace_slot(template, path, replacement)`** writes a new `Content` value
into the addressed slot of a cloned tree, returning `None` if the path doesn't
match.

**`normalize_list_item_runs(content)`** wraps consecutive bare `ListItem` /
`EnumItem` / `TermItem` nodes into their container elements. The Typst evaluator
sometimes emits items as siblings in a `SequenceElem`; this normalisation ensures
the tree always uses the container form before `extract_slots` runs.

### diff

The core diff logic. Operates entirely on `Content` values — no source text.

See [Algorithms in depth](#algorithms-in-depth) for implementation details.

Public API:
- `extract_blocks(content)` → `Vec<Content>`
- `extract_words(content)` → `Vec<Token>`
- `diff_blocks_raw(old, new)` → `Vec<BlockOp>`
- `match_edit_zones(ops)` → `Vec<BlockOp>`
- `diff_words(old, new)` → `Vec<WordOp>`
- `diff_content(old, new)` → `DiffResult`

### annotate

Converts a `DiffResult` into a single annotated `Content` value ready for
rendering.

**Page-style grouping:** Blocks are accumulated into groups that share the
same page styles. When page styles change between consecutive blocks, the
current group is flushed and wrapped with its page styles before starting a
new group. This preserves custom page formatting (margins, orientation,
headers/footers) from the original document.

**`replace_text_container`:** Before applying word-level annotations,
the annotator looks for the innermost paragraph or sequence container in a
block's Content tree and replaces its children with the annotated inline
content. This preserves surrounding structure (e.g. a `HeadingElem` keeps its
styling and level) while the text inside becomes coloured/struck.

See [Annotation strategy](#annotation-strategy) for the per-op rules.

### render

**`render_to_pdf(content, world)`**

Runs `typst_layout::layout_document` followed by `typst_pdf::pdf`.

Layout is run in a convergence loop (up to 5 iterations). Each iteration:
1. Builds an `Engine` with the current `Introspector`.
2. Calls `layout_document`, which returns a `PagedDocument` with a new
   `Introspector`.
3. Checks whether the introspector stabilised using
   `constraint.validate(&next_introspector)`.
4. Breaks early on stabilisation; otherwise continues with the updated
   introspector.

After the loop, delayed errors from `Sink` are collected. Any errors abort
with a formatted diagnostic message. This catches errors that are only
detectable after layout (e.g. missing label references).

PDF export uses `PdfOptions { tagged: false, ..default() }`. Tagged PDF is
disabled because the annotated content does not carry the semantic metadata
required for a conformant tagged PDF.

### diag

`format_diagnostics(world, diags)` turns a `&[SourceDiagnostic]` into a
newline-joined string of `path:line:col: message` entries.

The path is the virtual path (e.g. `chapter.typ`), not the absolute disk path,
so it matches what the user wrote in their `.typ` source.

---

## Key data structures

### `Content` (typst)

An opaque, reference-counted, hashable Typst content node. Every element type
(TextElem, HeadingElem, StrikeElem, …) is wrapped in a `Content`. Equality
and hashing compare the element type and all field values recursively.

`Content::sequence(iter)` creates a flat sequence node (`SequenceElem`) from
an iterator of `Content` values.

`content.styled(prop.set(value))` wraps a `Content` in a `StyledElem` that
applies a property override.

`content.styled_with_map(styles)` wraps with a full `Styles` map.

### `DiffBlock`

```rust
pub struct DiffBlock {
    pub content: Content,
    pub page_styles: Styles,
}
```

A block-level unit of content plus the page styles active at its location in
the document. Page styles accumulate from the document root and are made
sticky: a block without its own page-style update inherits the styles of the
nearest preceding block that had one.

### `BlockOp`

```rust
pub enum BlockOp {
    Equal(DiffBlock, DiffBlock),    // identical in both versions
    Delete(DiffBlock),              // only in old
    Insert(DiffBlock),              // only in new
    Replace(DiffBlock, DiffBlock),  // similar blocks, to be word-diffed
}
```

Produced first as `Equal`/`Delete`/`Insert` by the LCS diff, then
`Delete`/`Insert` pairs within edit zones are upgraded to `Replace` by the
similarity matcher.

### `DiffResult` / `DiffResultOp`

```rust
pub enum DiffResultOp {
    Equal(DiffBlock),
    Deleted(DiffBlock),
    Inserted(DiffBlock),
    Modified(DiffBlock, Vec<WordOp>),          // new block + word-level diff
    ModifiedSlots(DiffBlock, Vec<SlotDiff>),   // structured container, slots changed
}

pub struct DiffResult {
    pub block_ops: Vec<DiffResultOp>,
    pub root_styles: Styles,                   // document-level page styles
}
```

`root_styles` is extracted from the outermost `StyledElem` in the new
document's realized content. It carries document-level page styles, such as
default margins and headers/footers, and is applied to the entire annotated
output.

`ModifiedSlots` takes priority over `Modified`: when a `Replace` block pair
has the same slot shape (same number of slots at the same paths), `diff_content`
diffs corresponding slots word-by-word. If no slots are found or the shape
differs, it falls back to whole-block word diffing (`Modified`).

### `SlotDiff`

```rust
pub struct SlotDiff {
    pub path: Vec<SlotStep>,
    pub word_ops: Vec<WordOp>,
}
```

Identifies one changed slot within a structured container. `path` is the
`Vec<SlotStep>` address returned by `extract_slots`; `word_ops` is the
word-level diff for that slot's content. Only slots with at least one textual
change are included — unchanged slots are omitted from the `Vec<SlotDiff>`.

### `Token`

```rust
pub struct Token {
    pub text: String,    // equality key for the diff
    pub content: Content, // original Content node for reconstruction
}
```

Tokens are compared by `text` only; the `content` field is carried through for
faithful reconstruction of unchanged spans. For `TextElem` nodes the content is
a fresh `TextElem::packed(slice)` from the split, so it does not carry the
original element's style chain. For atomic inline nodes, `content` is the
original node verbatim.

### `WordOp`

```rust
pub enum WordOp {
    Equal(Vec<Token>),
    Delete(Vec<Token>),
    Insert(Vec<Token>),
}
```

Adjacent same-tag ops from the raw `similar` output are coalesced into a single
`WordOp` before the vector is returned. A second pass (`merge_substitution_zones`)
then absorbs whitespace-only `Equal` ops that sit between `Delete`/`Insert` runs
into those runs (see [Word-level diff](#word-level-diff)).

### `HashableContent`

An internal newtype that adds `Eq + Ord + Hash` to `Content` for use as slice
elements in `similar::capture_diff_slices`. Ordering is by `plain_text()` with
hash as tiebreaker, satisfying the `Ord`/`Eq` consistency contract required by
`similar`.

---

## Algorithms in depth

### Block extraction

`extract_block_units_with_styles(content, inherited_page_styles)` walks the
Content tree and segments it into block-level units.

The tree is structurally one of:
- `SequenceElem` — a flat list of children (dispatch to
  `collect_blocks_from_children`)
- `StyledElem` — a wrapper carrying a `Styles` map around a child; page styles
  are split from non-page styles; non-page styles are propagated to each
  extracted child block via `apply_block_styles`
- Anything else — treated as a single atomic block

Within `collect_blocks_from_children`, each child is classified:

| Child type | Action |
|---|---|
| `StyledElem` wrapping an inline sequence | Treated as inline; pushed to current paragraph accumulator |
| `StyledElem` wrapping a block sequence | Flush paragraph; recurse |
| `StyledElem` wrapping a single block node | Flush paragraph; emit as atomic block |
| `SequenceElem` that is inline-only | Accumulated into current paragraph |
| `SequenceElem` with block children | Recurse (`collect_blocks_from_children`) |
| `ParbreakElem` | Flush paragraph; emit as its own `DiffBlock` |
| `HeadingElem`, `RawElem`, display `EquationElem` | Flush paragraph; emit as atomic block |
| `TableElem` | Flush paragraph; emit as atomic block; cell bodies are handled later by table-specific replacement logic |
| Known inline nodes | Append to current paragraph |
| Unknown nodes | Flush paragraph; emit as atomic block |

A node is "known inline" if it is one of: `TextElem`, `SpaceElem`,
`LinebreakElem`, `StrongElem`, `EmphElem`, `LinkElem`, `SmartQuoteElem`,
`UnderlineElem`, `OverlineElem`, `StrikeElem`, `HighlightElem`, `SubElem`,
`SuperElem`, or inline `EquationElem`. `StyledElem` nodes whose child is an
inline-only sequence are also treated as inline.

**Paragraph flushing:** A pending paragraph is flushed into a `DiffBlock` only
if it contains at least one non-`SpaceElem` node. Whitespace-only paragraphs
are discarded.

**Page style stickiness:** After all blocks are extracted, `make_page_styles_sticky`
does a single forward pass and copies the most recently seen non-empty
`page_styles` to every subsequent block that doesn't have its own. This ensures
that the page layout (margins, orientation) established by a `set page(...)` at
the top of a section applies to all blocks within that section.

### Block-level LCS diff

`diff_block_units_raw(old, new)` calls `similar::capture_diff_slices` (Myers
algorithm) on two slices of `HashableContent` values. The `HashableContent`
wrapper makes `Content` usable as a `similar` element.

`similar` returns `DiffOp::Equal`, `DiffOp::Delete`, `DiffOp::Insert`, and
`DiffOp::Replace` variants. `Replace` ops are immediately decomposed into
`Delete` + `Insert` sequences so that the edit zone matcher receives a
normalised stream.

The output is a `Vec<BlockOp>` with only `Equal`, `Delete`, and `Insert` tags
(no `Replace` at this stage).

### Edit zone pairing

`match_edit_zones(ops)` scans the raw block ops and identifies *edit zones*:
maximal contiguous runs of `Delete` and/or `Insert` ops (in any order). Within
each zone, it attempts to pair each deleted block with an inserted block.

**Greedy pairing algorithm (in `pair_edit_zone`):**

1. For each deleted block, in its original order, compute the similarity score
   against every not-yet-paired inserted block.
2. Accept the highest-scoring pairing if similarity ≥ 0.3; otherwise leave the
   deleted block unpaired.
3. Emit all pairings as `Replace(old, new)`.
4. Emit remaining unpaired deletes as `Delete`.
5. Emit remaining unpaired inserts as `Insert`.

Replaced pairs appear in the output in the original order of their deleted
halves, with unpaired inserts appended afterwards. This keeps the output
readable when there are more inserts than deletes (or vice versa).

### Word-level diff

`diff_words(old_tokens, new_tokens)` calls `similar::capture_diff_slices` on
two `Vec<Token>` slices. `Token` implements `PartialEq`, `Eq`, `Hash`, and
`Ord` based on the `text` field only, satisfying the trait bounds required by
`similar`.

`DiffOp::Replace` from `similar` is decomposed into Delete + Insert pairs
before coalescing. Adjacent ops of the same tag are merged by `coalesce` so
the result never has two consecutive ops of the same type.

**Substitution zone merging (`merge_substitution_zones`):** Myers LCS operates
on individual tokens, so replacing an entire sentence word-by-word produces an
alternating Delete / Insert / Equal(space) / Delete / Insert / … pattern. The
whitespace-only `Equal` ops prevent `coalesce` from joining the Deletes (or
Inserts) into a single run.

`merge_substitution_zones` runs as a final pass inside `diff_words`. It
identifies *zones*: maximal contiguous runs of `Delete`, `Insert`, and
whitespace-only `Equal` ops. Within each zone:

- Two per-side pending-whitespace buffers accumulate whitespace tokens from
  `Equal` ops.
- On `Delete`: flush the delete-side buffer into the delete token list, then
  append the op's own tokens.
- On `Insert`: same for the insert side.
- Trailing whitespace (no following op to consume it) is dropped.

The result is at most one `Delete` followed by at most one `Insert` per zone,
with whitespace embedded. A sentence-level substitution therefore renders as
one contiguous red strikethrough followed by one contiguous green run instead
of alternating red–green at every word boundary.

### Similarity metric

`similarity(a, b)` returns a score in `[0.0, 1.0]` where 1.0 means identical.

Three code paths depending on input length:

1. **Both empty** → 1.0
2. **`max_chars ≤ 2000`** → edit distance with early exit:
   - Computes the maximum edit distance allowed for similarity ≥ 0.3:
     `max_distance = floor((1 - 0.3) * max_chars)`
   - Runs a space-optimised O(m×n) Levenshtein DP but abandons (returns `None`)
     as soon as the minimum value in any DP row exceeds `max_distance`.
   - If the length difference alone exceeds `max_distance`, skips the DP
     entirely.
   - Score: `1.0 - edit_distance / max_chars`
3. **`max_chars > 2000`** → Sørensen–Dice word overlap:
   - Tokenise both strings on whitespace.
   - Count the overlap (minimum count of each word appearing in both).
   - Score: `2 * overlap / (len_a + len_b)`

The 2000-character threshold prevents O(n²) edit distance from becoming a
bottleneck on large blocks (e.g. code listings).

**Token extraction for large atomic nodes:** `collect_tokens` checks whether a
non-text, non-space node has a `plain_text()` longer than 500 characters. If
so, the text is tokenised at whitespace boundaries and each piece becomes its
own `Token`, even though the original Content structure is opaque. This prevents
a single large strong/emph block from becoming one giant undiffable token.

**Math tokens:** `EquationElem` nodes are tokens keyed by `equation.body.repr()`
rather than by `plain_text()`. This preserves distinctions that text extraction
would lose, such as scripts, fractions, and other math structure. The current
granularity is expression-level: symbols inside a single equation are not yet
diffed independently.

**Slot containers:** `diff_slots` handles structured-container-vs-container
replacements. It calls `extract_slots` on both sides and, if the slot count
and paths match, word-diffs each slot pair. Only slots with textual changes
become `SlotDiff` entries. The slot mechanism covers tables (cell-by-cell),
lists (item-by-item), figures (body + caption), footnotes, quotes, grids, and
most layout wrappers. If the slot shape differs (structure changed), `diff_slots`
returns `None` and the block falls back to whole-block word diffing.

---

## Annotation strategy

`build_annotated_content(result)` iterates over `DiffResultOp` values and
produces annotated `Content`:

| Op | Strategy |
|---|---|
| `Equal(block)` | Emit `block.content` unchanged. |
| `Inserted(block)` | Wrap `block.content` with `.styled(TextElem::fill.set(green()))`. All text inside inherits green fill. |
| `Deleted(block)` | Flatten the block to plain text via `plain_content()`, apply red fill, wrap in `StrikeElem`. Using plain text avoids re-rendering structural side effects (e.g. a deleted `HeadingElem` would otherwise still add a chapter number to the document). |
| `Modified(new_block, word_ops)` | Build inline annotated content from `word_ops` (see below), then use `replace_text_container` to graft it into the original block structure. |
| `ModifiedSlots(new_block, slot_diffs)` | For each changed slot: build annotated inline content from the slot's `word_ops`, use `replace_inline_content` to graft it into the slot body, then use `replace_slot` to write the slot back into a clone of `new_block.content`. |

**Word-op inline rendering:**

- `Equal` tokens → emit `token.content` as-is.
- `Insert` tokens → join all token contents in a `Content::sequence`, wrap with
  green fill style.
- `Delete` tokens → render token-by-token. Text-like tokens are wrapped with
  red fill and `StrikeElem`. Equation tokens are rebuilt as `EquationElem`
  containing `CancelElem`, because text strikethrough does not apply inside
  math layout.

**`replace_text_container(template, replacement)`** walks the block's Content
tree looking for the node to replace the inline children:
1. If the block is a `ParElem`, replace `par.body` directly.
2. If the block is a `StyledElem`, recurse into its child.
3. If the block is a `SequenceElem` whose children are all "inlineish" (no
   `ParElem` or nested `SequenceElem`), replace all children with the
   replacement sequence.
4. Otherwise, recurse into `SequenceElem` children and replace the first
   successful match.

If no suitable container is found (e.g. a `HeadingElem` without a bare inner
sequence), `replace_text_container` returns `None` and the annotated content is
used directly without structural wrapping.

**Page-style grouping:** Blocks are batched by `page_styles` identity. Whenever
`page_styles` changes between consecutive blocks, the accumulated batch is
wrapped with `Content::sequence(...).styled_with_map(page_styles)` and added to
the output. The final batch is always flushed. The entire output sequence is
then wrapped with `result.root_styles` (document-wide page styles).

---

## Rendering and layout convergence

Typst documents may contain cross-references and counter-dependent content
(e.g. `@label`, `counter(heading).display()`). Resolving these correctly
requires the `Introspector`, which is only available after layout.

The convergence loop in `render_to_pdf`:

```
introspector = Introspector::default()
repeat up to 5 times:
    engine.introspector = introspector.track_with(&constraint)
    document = layout_document(&mut engine, content, styles)
    next_introspector = document.introspector.clone()
    if constraint.validate(&next_introspector): break
    introspector = next_introspector
```

`constraint.validate` returns `true` once the introspector's observable state
has not changed between iterations — i.e., all cross-references have stabilised.
In practice, 1–2 iterations suffice for the diff output since it does not
introduce new labels or counters.

The same loop runs in `eval.rs::layout_introspector` before realization, to
ensure that any content that depends on page numbers or counters in the
*original* document is evaluated with a stable introspector.

---

## Git revision mode

When `--revision <REV>` is passed, `resolve_git_inputs` runs:

1. `git rev-parse --show-toplevel` from the file's directory to find the git
   root.
2. Derives the relative path of the entry file inside the working tree.
3. `git archive --format=tar <REV>` — produces a tar archive of the entire tree
   at `REV`. The full tree (not just the entry file) is archived so that
   `#include` paths in the snapshot resolve correctly.
4. The tar stream is piped directly to `tar -x -C <tmpdir>` to unpack into a
   `TempDir`.
5. The old entry point is `<tmpdir>/<relative_path>`.

The `TempDir` is stored in `ResolvedInputs::_snapshot` and dropped (cleaned up)
when `main()` returns.

**Limitations:**
- Requires `git` and `tar` on `PATH`.
- The file must be tracked by the repository at the given revision.
- Binary assets (images, fonts not included in the Typst kit) must also be
  present at the revision for the old document to render.

---

## Performance notes

**Evaluation:** Each `eval_to_realized_content` call performs a full Typst
compilation including layout. For large documents this is the dominant cost. No
incremental compilation is implemented.

**Similarity:** The edit distance computation is O(m×n) but aborts early when
the minimum row value exceeds the allowed distance. For blocks that are clearly
dissimilar this exits within a few rows. For the >2000-character case, the
Sørensen–Dice word overlap runs in O(a + b) time.

**LCS diff:** `similar` uses Myers' O(nd) algorithm where d is the edit
distance. For large documents with few changes, d is small and the algorithm
runs close to O(n). For documents where every block changes, d ≈ n and the
algorithm is O(n²).

**Font loading:** `FontSearcher::search()` scans system font directories once
per `SystemWorld` construction. For the two-document comparison this happens
twice; the two worlds do not share a font cache.

---

## Limitations and known edge cases

**Moved paragraphs:** A paragraph that is moved from one location to another
appears as a deletion at the original site and an insertion at the new site.
Moved-block detection is not implemented.

**Math equations:** Display equations are treated as atomic blocks (never
word-diffed). Inline equations are atomic tokens. Changes inside equations are
shown as whole-equation delete + insert. Deleted equations use Typst's
`math.cancel` element instead of text strikethrough.

**Code blocks (`RawElem`):** Treated as atomic blocks. Source-level changes
inside code blocks are shown as whole-block delete + insert.

**Slot shape matching:** `diff_slots` only proceeds when the old and new
container have the same number of slots at the same paths. If the list or
table structure itself changes (items added/removed, columns inserted), the
whole container falls back to plain word diffing or whole-block
delete + insert.

**Block similarity threshold:** The 0.3 threshold is a fixed constant.
Completely rewritten paragraphs will sometimes exceed 0.3 similarity and be
paired for word-level diffing, which is usually the desired behaviour. A
threshold of 0.0 would pair everything; 1.0 would never pair.

**Structural headings:** Deleted `HeadingElem` blocks are rendered as plain
strikethrough text to avoid incrementing chapter counters or generating
table-of-contents entries for deleted content. Inserted headings retain full
heading styling.

**Page-style tracking:** Page style stickiness is a forward pass over extracted
blocks. If a document sets page styles in a way not captured by the `StyledElem`
unwrapping logic, blocks may inherit incorrect page styles.

**Two-document restriction:** The CLI takes exactly two document versions. Git
range diffing (three-way merge, more than two revisions) is out of scope.

**HTML output:** Only PDF output is supported.

**Configurable colours:** Add/delete colours (green `#00b400`, red `#dc0000`)
are hardcoded constants. No CLI flag or document-level configuration is
provided.
