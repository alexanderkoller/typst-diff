# typst-diff System Walkthrough

This document is a current, pedagogical walkthrough of `typst-diff`. It is
written for contributors who know Typst as users and want to understand how the
crate turns two Typst entry files into an annotated PDF.

The central idea is simple:

> Compare the document Typst evaluates, not the text the author typed.

That choice drives almost every design decision. Typst source is a program:
`#include`, `#let`, show rules, counters, references, footnotes, and contextual
headers can all change what a reader sees without producing a useful source
diff. `typst-diff` therefore evaluates both documents through Typst, recovers
semantic structure from the pre-realization tree, compares that structure, and
renders a third Typst tree whose additions and deletions are visible.

## The Whole Pipeline

At a high level, the CLI in `src/main.rs` performs this pipeline:

```text
old.typ -> SystemWorld -> eval_to_realized_content -> old: AnnotatedContent
new.typ -> SystemWorld -> eval_to_realized_content -> new: AnnotatedContent
                                      |
                       diff_annotated_with_rendered_regions
                                      |
                               DiffResult
                                      |
                    build_annotated_content_from_tree
                                      |
                                 Content
                                      |
                              render_to_pdf
                                      |
                                diff.pdf
```

The old and new documents each get their own `SystemWorld`. The old world is
used only to evaluate and, for some page-region cases, to lay out the old
document. The new world is reused for the final annotated render because the
output should resolve assets, packages, fonts, page setup, and includes in the
new document's environment.

The main modules are:

| Module | Responsibility |
| --- | --- |
| `world.rs` | Implements Typst's `World` trait over the local filesystem and packages. |
| `eval.rs` | Evaluates, lays out, realizes, and annotates a Typst document. |
| `annotated.rs` | Defines `AnnotatedContent` and builds the annotated tree. |
| `container_ops.rs` | Owns structured container slot mapping and child replacement. |
| `diff.rs` | Extracts blocks and tokens, computes block, slot, word, and page-region diffs. |
| `annotate.rs` | Applies a `DiffResult` back onto Typst `Content` with colors and strikeout. |
| `render.rs` | Lays out the annotated content and serializes it as PDF. |

## Why Source Diffing Is Not Enough

Consider this Typst source:

```typst
#let speaker = "Alice"
#speaker said hello.
```

If the next version changes only `speaker` to `"Bob"`, a source diff marks the
definition line. The reader sees a different sentence: "Alice said hello." has
become "Bob said hello." The visible change is not located at the source line
where it was authored.

Show rules make this more important:

```typst
#show heading: it => [Section: #it.body]
= Setup
```

The source heading body is just `Setup`, but the realized visible content is
`Section: Setup`. A useful diff must compare what Typst's evaluation and
realization produce.

## Worlds: Giving Typst A Filesystem

Typst compilers do not read files directly; they ask a `World` for sources,
bytes, fonts, packages, and date information. `SystemWorld` is this crate's
filesystem-backed implementation.

`SystemWorld::new(entry)`:

1. Canonicalizes the entry file.
2. Uses the entry file's parent as the project root.
3. Creates a virtual `FileId` like `/main.typ`.
4. Loads fonts through `typst-kit`.
5. Creates package storage using `typst-kit`.
6. Preloads the entry source into a cache.

Source files and binary files are cached separately. Once a `SystemWorld` has
read a file, later compiler passes see the same bytes even if the file changes
on disk. This gives each evaluation a stable input snapshot. `today()` returns
`None`, so Typst documents that depend on today's date do not produce
time-dependent diffs.

## Evaluation And Realization

`eval.rs` exposes two public entry points:

```rust
eval_to_content(world) -> Content
eval_to_realized_content(world) -> AnnotatedContent
```

`eval_to_content` is the raw Typst evaluation step. It returns Typst `Content`
before layout-dependent realization has finished.

`eval_to_realized_content` is the production path. It does more work:

1. Evaluate source to pre-realization `Content`.
2. Normalize bare list, enum, and term item runs into real containers.
3. Run layout up to five iterations to build a stable `Introspector`.
4. Call Typst's realization routine.
5. Build an `AnnotatedContent` tree by walking pre-realization and realized
   content together.
6. Attach equation origins and footnote bodies as extra annotation metadata.

The layout loop matters because counters, references, footnotes, and contextual
content depend on where things land. Typst converges these features through
layout iterations; `typst-diff` follows the same model.

## Content Trees

Typst represents evaluated documents as `Content` trees. A small document:

```typst
= Animals

The *quick* fox.
```

is roughly:

```text
SequenceElem
├─ HeadingElem
│  └─ TextElem("Animals")
├─ ParbreakElem
└─ ParElem
   └─ SequenceElem
      ├─ TextElem("The ")
      ├─ StrongElem
      │  └─ TextElem("quick")
      └─ TextElem(" fox.")
```

This is not a stable enough tree to diff directly. Typst realization may wrap
nodes with styles, rewrite headings through show rules, turn footnotes into
markers plus page-footer material, or turn equations into layout-oriented
carriers. The diff needs both sides:

- **Realized content**, because it is what gets rendered.
- **Semantic origin**, because it tells the diff that a set of realized blocks
  came from a list item, table cell, figure caption, footnote body, or wrapper
  body.

That is the purpose of the annotated tree.

## System Invariants

The system is easiest to reason about if you keep its invariants separate from
its current fallback behavior. These are the contracts the pipeline relies on.

### Evaluation Invariants

- A `SystemWorld` is a stable input snapshot. Once a source or binary file is
  read, later compiler passes see the cached value.
- Evaluation is deterministic with respect to dates: `today()` returns `None`.
- The old and new documents are evaluated independently. No state from the old
  world may be required to render normal body output for the new-side diff.
- The final annotated document is rendered in the new world.

### Annotated-Tree Invariants

- `AnnotatedContent.realized` is Typst's realized content, preserved verbatim
  at construction time.
- `Annotation` may explain or supplement `realized`, but it should not rewrite
  what Typst produced.
- Every `SemanticSlot.path` must resolve through `AnnotatedContent.children`.
  A slot whose path cannot be resolved is not a usable semantic slot.
- Slot labels are semantic positions, not display text. `ListItem(1)`,
  `FigureCaption`, and `TableCell(3)` describe roles in a container.
- A node may recurse through slots only when old and new nodes have the same
  `semantic_kind` and both have resolved slots.
- A `patch_surface`, when present, is the local surface to patch and render; it
  does not replace the meaning of `realized`.
- Empty realized text is not proof that a node is semantically empty. Empty
  wrappers, equations, shapes, and page regions may still carry changes.

### Diff Invariants

- Block diffing decides where edits occur in document order; it should not be
  the only place structural meaning is recovered.
- Slot diffing is the preferred path for structured containers. Word diffing is
  the final local edit surface once structural descent is exhausted.
- `plain_text()` is a useful similarity signal, but it is not a complete
  identity. Links, labels, equations, styles, and repeated identical text can
  differ while visible text is the same.
- Delete edits are anchored in the new-side tree: before an available new slot,
  after a previous new slot, or appended when no anchor exists.
- The edit script must be renderable. Every `ReplaceAt`, `InsertBefore`, and
  `InsertAfter` path must be meaningful on the annotated base or its
  `patch_surface`.

### Rendering Invariants

- Inserted content must remain present in the rendered output and be styled as
  inserted.
- Deleted content must remain present in the rendered output and be styled as
  deleted.
- Modified content must preserve the surrounding block/container whenever the
  edit is local to a slot or inline region.
- Page styles are grouped and applied at group boundaries; page-style changes
  must not be pushed into ordinary inline/block styles.
- PDF is the final serialization step, not the primary debugging surface.

## The Annotated Tree

`AnnotatedContent` is defined in `src/annotated.rs`:

```rust
pub struct AnnotatedContent {
    pub realized: Content,
    pub annotation: Annotation,
    pub children: Vec<AnnotatedContent>,
}
```

The invariant is:

> `realized` is preserved exactly as Typst produced it; annotations are extra
> information attached beside the realized tree, not mutations of it.

This invariant is checked by tests such as
`annotate_preserves_realized_content_unchanged`. It is also the reason edit
application works on a cloned annotated base rather than mutating the evaluated
document in place.

`Annotation` contains:

| Field | Meaning |
| --- | --- |
| `semantic_kind` | The pre-realization semantic identity, such as `List`, `Table`, `Figure`, `Paragraph`, or `Wrapper(Box)`. |
| `slots` | Named semantic child positions inside the node. |
| `footnote` | Footnote body attached to a realized footnote marker. |
| `patch_surface` | A structured replacement surface used when the realized tree is too opaque or loses layout-bearing nodes. |
| `equation_origins` | Source equations associated with realized math carriers. |
| `span` | Source span retained for diagnostics and alignment hints. |

### Realized Children Versus Semantic Slots

`children` mirrors useful descent points in the realized tree. A child path such
as `[2, 0]` means "child 2, then child 0" in this annotated tree.

`slots` names important semantic positions inside `children`. A slot has:

```rust
pub struct SemanticSlot {
    pub label: SlotStep,
    pub path: Vec<usize>,
}
```

The path invariant is important: if `node.annotation.slots` contains a slot,
`node.get_path(&slot.path)` should return the annotated child for that slot.
Downstream diffing treats unresolved paths as absent, so a broken path silently
removes structure from the diff.

For a list:

```typst
- Alpha
- Beta
- Gamma
```

the annotated list node has semantic kind `List` and slots like:

```text
List
├─ slot ListItem(0) -> path [0] -> "Alpha"
├─ slot ListItem(1) -> path [1] -> "Beta"
└─ slot ListItem(2) -> path [2] -> "Gamma"
```

For terms:

```typst
/ API: Application Programming Interface
```

one `TermItem` becomes two slots:

```text
Terms
├─ slot Term(0)            -> "API"
└─ slot TermDescription(0) -> "Application Programming Interface"
```

For a figure:

```text
Figure
├─ slot FigureBody
└─ slot FigureCaption
```

For wrappers such as `box`, `block`, `align`, `pad`, `place`, `columns`,
`rect`, `circle`, and `ellipse`, the slot is `WrapperBody`.

Slots are the mechanism that lets a table cell or list item be diffed as the
changed unit instead of treating the whole table or whole list as a single
paragraph.

Slot labels also form a shape invariant. When old and new slot-label sequences
are identical, the diff can compare children pairwise. When labels differ, the
diff falls back to an LCS over slot child text to model insertions and
deletions. Either way, a slot is expected to identify a semantic position in
the container, not a visual fragment found by chance.

## Constructing The Annotated Tree

`annotate_realized(pre, realized)` walks two trees together:

- `pre` is the normalized pre-realization `Content`.
- `realized` is the content Typst will render.

The walker handles four broad cases.

### Structural Containers

If `pre` is a known container, `container_ops::map_container` builds a
`SlotMapping`. The mapping provides:

- the patch surface to use for edits,
- annotated child nodes,
- semantic slots.

The container module is deliberately centralized. Lists, enums, terms, tables,
grids, stacks, figures, footnotes, quotes, and wrappers all implement the same
internal `ContainerOps` interface: identify the container kind, extract slot
parts, replace a child, and optionally insert a child.

### Transparent Wrappers

If both sides are sequences, styled nodes, or paragraphs, the walker descends
through them. If two sequences have the same length, children are paired by
position. If their lengths differ, `pair_sequence_by_span` walks both sequences
in document order and uses source spans as an alignment hint.

The span pairing is not a primary identity system. It is a practical bridge for
realization cases where the number of siblings changes. When no span match is
found, the current implementation may still pair positionally; the review
document calls out this fallback as a cleanup target.

### Leaves

If no structured descent applies, the realized content becomes a leaf
`AnnotatedContent`. The annotation may still remember a semantic kind such as
`Equation`, `Heading`, or `RawBlock`.

### Post-Passes: Equations And Footnotes

Equations and footnotes are special because realization changes their shape.

For equations, `annotate_equation_origins` collects source `EquationElem`s and
assigns them, in document order, to realized math carriers. The word tokenizer
can then use source equation content as the comparable token instead of relying
only on realized layout wrappers.

For footnotes, `annotate_footnote_markers` walks realized content and attaches
the pre-realization footnote body to the marker site. This is how a footnote
body can be diffed even though Typst has moved the rendered note to the page
footer.

## Patch Surfaces

Most nodes render from their `realized` content. Some nodes carry a
`patch_surface`: a substitute content tree used locally when edits must be
applied.

Patch surfaces exist because realized content can be a poor edit target. For
example:

- realization may drop an explicit `ParbreakElem` that is needed to preserve
  the paragraph/list boundary in the final annotated output;
- a container may realize to fewer addressable children than its semantic slot
  count;
- a wrapper may hide its body under styled or block output.

A patch surface should be understood as "the content tree we will patch and
render for this node", while `realized` remains "the content Typst gave us."
The long-term design direction should keep patch surfaces principled and tied
to structural invariants, not to one-off visual repairs.

A good patch surface preserves three things at once: the semantic slots being
edited, the renderable container shape, and any layout boundary that Typst's
realized tree omitted but the annotated output still needs. If a patch surface
exists only because one corpus case needed a leading paragraph break, that is a
sign the invariant has not yet been expressed cleanly enough.

## Block Extraction

The first diff stage extracts block-level units from the effective new and old
content. A `DiffBlock` contains:

```rust
pub struct DiffBlock {
    pub content: Content,
    pub page_styles: Styles,
}
```

Block extraction walks sequences and styled content, accumulating inline nodes
into paragraphs and flushing when it sees block-level content. The important
rules are:

- Inline text, spaces, styling, links, highlights, inline equations, and similar
  nodes are accumulated into a paragraph.
- Headings, raw blocks, display equations, parbreaks, and unknown block-ish
  content become block boundaries.
- Structured containers remain single block units so their slots can be diffed
  internally.
- Page styles are separated from non-page styles and made sticky, so each block
  knows the active page setup at its position.

Text runs are normalized inside paragraphs so equivalent visible text does not
fail block equality merely because Typst split it into different `TextElem` and
`SpaceElem` runs.

## The Diff Algorithm

The diff is layered. Each layer tries to preserve structure; lower layers are
used only when higher-level structure is not available.

The layer invariant is:

```text
document order -> block edit -> semantic slot edit -> word edit -> renderable edit script
```

Each layer should pass enough ownership information to the next layer that the
next layer does not have to guess. Whenever a later layer searches by visible
text or uniqueness, it is compensating for missing ownership information.

### 1. Block LCS

The old and new block slices are passed through `similar::capture_diff_slices`
with Myers LCS. `Content` is wrapped in `HashableContent` so the library can
compare blocks.

The raw output is a sequence of:

```text
Equal
Delete
Insert
```

`Replace` operations from the library are expanded into delete and insert runs
because the next step performs its own pairing.

### 2. Edit-Zone Pairing

`match_edit_zones` groups contiguous delete/insert runs into edit zones. Within
each zone, each deleted block is greedily matched to the unused inserted block
with the highest plain-text similarity. If similarity is at least `0.3`, the
pair becomes `BlockOp::Replace`.

Example:

```text
old blocks: A, "The old paragraph.", C
new blocks: A, "The new paragraph.", C
```

The LCS sees `A` and `C` as equal and a delete/insert zone in between.
Similarity pairs the two middle blocks into a replacement. That replacement can
then be word-diffed rather than shown as a whole deleted paragraph followed by a
whole inserted paragraph.

### 3. Annotated Owner Lookup

For each block operation, `diff_annotated` finds the corresponding annotated
subtree. This matters because a block may be the realized representation of a
larger semantic owner, such as one item inside a list or one cell inside a
table.

The lookup prefers exact realized-content matches with slots. If an exact match
is not slot-bearing, it may look for a single-block semantic owner whose
extracted block equals the target.

### 4. Slot Diff

If both old and new annotated owners have the same semantic kind and have
slots, the diff recurses through slots.

There are two slot shapes:

#### Same Labels

If labels match exactly:

```text
old slots: ListItem(0), ListItem(1), ListItem(2)
new slots: ListItem(0), ListItem(1), ListItem(2)
```

children are compared position by position. Equal annotated subtrees produce no
edit. Changed children either recurse into nested slots or produce a word-level
`Modified` edit at that slot path.

#### Changed Labels

If labels differ, usually because items were inserted or deleted, `diff_slot_edits_lcs`
uses Myers LCS over each slot child's effective plain text.

Worked example:

```typst
// old
- Alpha
- Gamma

// new
- Alpha
- Beta
- Gamma
```

The list slots differ:

```text
old texts: Alpha, Gamma
new texts: Alpha, Beta, Gamma
```

The slot LCS yields:

```text
Equal Alpha
Insert Beta
Equal Gamma
```

The insert becomes a `ReplaceAt` edit on the new `Beta` slot with
`EditContent::Inserted`. Rendering applies green fill inside the list item while
preserving the list structure.

For deletions, the edit is inserted before or after a nearby new-side slot, or
appended if there is no anchor.

### 5. Word Diff

When a block or slot cannot recurse further, the diff tokenizes inline content
and runs Myers LCS over tokens.

Tokens are:

- text chunks split at whitespace boundaries,
- whitespace chunks,
- equations as atomic tokens using equation source when available,
- semantic child tokens for containers,
- otherwise one atomic token containing the node's plain text.

Token equality uses only `Token.text`. The original `Content` is retained so
unchanged tokens and changed tokens can preserve styling when rendered.

That is a deliberate split: token equality is about matching comparable visible
text, while token content is about reconstructing styled output. It should not
be confused with document identity.

Whitespace-only equal operations inside substitution zones are merged into the
adjacent delete/insert side. This avoids awkward output where a replacement is
split into many tiny alternating operations around spaces.

## Page Regions

Headers, footers, background, and foreground are not ordinary body blocks. The
diff handles them separately.

There are two paths:

1. **Semantic page-region diff.** If the page-region content stored in page
   styles differs, it is annotated as a region edit.
2. **Rendered page-region diff.** If the semantic content is equal but the
   rendered text changes per page, as with `counter(page).final()`, the old and
   new documents are laid out and text is extracted from artifact-tagged header
   or footer areas. Those per-page strings are word-diffed and turned into a
   generated Typst `#context` expression.

The rendered-region path exists for contextual values that cannot be seen by
comparing the style content tree alone.

## Applying Edits

`DiffResult` is a structured edit script. It does not itself contain colored
Typst output. `annotate.rs` applies it.

The edit-script invariant is that every edit must name a base and a place where
the rendered payload can be applied. If a path cannot be applied to the base or
patch surface, the output may silently lose an insertion or deletion.

Main edit forms:

| Edit | Meaning |
| --- | --- |
| `ReplaceAt` | Replace the content at an annotated path. |
| `InsertBefore` | Insert deleted content before an anchor path. |
| `InsertAfter` | Insert deleted content after an anchor path. |
| `Append` | Append deleted content when no anchor exists. |
| `WholeBlock` | Replace a whole block with inserted, deleted, modified, or nested content. |

`EditContent` says how to render the payload:

| Payload | Rendering |
| --- | --- |
| `Inserted` | Green fill inside the content. |
| `Deleted` | Red fill plus strikeout; equations use math cancel. |
| `Modified` | Word ops are converted into inline colored/struck content and grafted into the base. |
| `Nested` | Recursively apply another edit list to an annotated subtree. |

After each block is annotated, blocks are grouped by page style. A new group is
started whenever page styles change. The final output is a `Content::sequence`
wrapped with root page styles plus per-group page styles.

## Rendering

`render_to_pdf` lays out the annotated content with the new world and then asks
`typst_pdf` for PDF bytes. Layout uses the same convergence loop idea as
evaluation, because the annotated document can contain counters, footnotes,
headers, and contextual content too.

The project intentionally treats PDF as an output artifact. Tests may render
PDFs or compare rendered PNG references, but contributors should not inspect PDF
files directly when debugging. Work from source, Typst content trees, layout
frames, modification logs, or rendered images.

## A Worked Example: One List Item Changes

Input:

```typst
// old
- Install Rust
- Run the old command
- Read the output

// new
- Install Rust
- Run the new command
- Read the output
```

The pipeline behaves like this:

1. Evaluation produces list containers.
2. Annotation marks the list node as `SemanticKind::List` with three
   `ListItem` slots.
3. Block extraction treats the whole list as one block.
4. Block LCS pairs the old and new list as a replacement.
5. `diff_annotated` finds the list owner on both sides and recurses through
   slots.
6. Slot labels match, so slot 0 and slot 2 are equal.
7. Slot 1 is word-diffed:

```text
Equal  "Run the "
Delete "old"
Insert "new"
Equal  " command"
```

8. Rendering replaces only slot 1's body. The output remains a list, with
   "old" struck red and "new" green.

## A Worked Example: A Table Row Is Inserted

Input:

```typst
// old
#table(
  columns: 2,
  [A], [1],
  [C], [3],
)

// new
#table(
  columns: 2,
  [A], [1],
  [B], [2],
  [C], [3],
)
```

`TableOps` extracts cells in document order:

```text
old slots: TableCell(0)="A", TableCell(1)="1", TableCell(2)="C", TableCell(3)="3"
new slots: TableCell(0)="A", TableCell(1)="1", TableCell(2)="B", TableCell(3)="2", TableCell(4)="C", TableCell(5)="3"
```

The slot LCS runs over cell plain text. The inserted row appears as inserted
cell edits at the new paths. Rendering applies green text inside the inserted
cells and preserves the table container.

This design is not a full table-layout diff. It does not understand row groups,
column spans, or header/footer insertion as first-class row operations. It uses
document-order cells as the current semantic unit.

## A Worked Example: Contextual Footer

Input:

```typst
#set page(footer: context [Page #counter(page).get().first() of #counter(page).final().first()])
```

If a body edit changes the total page count from 2 to 3, the footer style
content may be semantically equal in both versions. The rendered text is not:

```text
old page 1 footer: Page 1 of 2
new page 1 footer: Page 1 of 3
```

The rendered-region path lays out both documents, extracts footer text from
page frames, diffs those strings, and creates a generated context expression
that renders the per-page annotated footer.

## Design Principles

The codebase is most coherent when it follows these principles:

1. Diff evaluated, realized output, not raw source.
2. Preserve Typst's realized content verbatim; attach semantic annotations
   beside it.
3. Represent structure once through annotated slots, then recurse generically.
4. Treat block diffing, slot diffing, and word diffing as layers of the same
   algorithm, not as independent code paths.
5. Keep container-specific knowledge centralized in `container_ops.rs`.
6. Prefer structural identity and explicit paths over `plain_text()` guesses.
7. Keep patch surfaces principled: they should repair editability while
   preserving semantic intent, not encode visual one-offs.
8. Turn nontrivial bugs into tests at the level where the invariant failed:
   annotation, slot diff, edit application, layout, or rendered output.

Another way to say the same thing: keep the invariants explicit, and treat
fallbacks as temporary evidence that an invariant is missing or underspecified.

## Where To Start When Debugging

When diagnosing a bug, walk the pipeline in order:

1. Does `eval_to_content` contain the semantic source element you expect?
2. Does `normalize_list_item_runs` put items into the expected containers?
3. Does realization produce visible content with the expected text?
4. Does `annotate_realized` attach the expected `semantic_kind` and slots?
5. Does block extraction produce the expected block boundaries?
6. Does block matching pair the intended old/new blocks?
7. Does slot diff recurse to the intended semantic unit?
8. Does word diff produce the expected token operations?
9. Does `build_annotated_content_from_tree` preserve the surrounding container?
10. Does layout/rendering show the same content contract the edit script
    promised?

For invariant-focused debugging, attach an expectation to each step. For
example: "the list node should have three `ListItem` slots whose paths resolve";
"the changed table should produce one inserted cell edit"; "the rendered output
should contain every deleted token in a `StrikeElem`." Then probe the first
stage where the expectation fails.

Small probes are often best: inspect plain text, annotated slot labels and
paths, modification logs, or layout frame text runs. Avoid reading PDFs
directly; render or inspect the data structures before PDF serialization.
