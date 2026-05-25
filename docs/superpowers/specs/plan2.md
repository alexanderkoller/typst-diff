# Plan: replace the realize-time hybrid tree with an annotated realized tree

## Context

The current pipeline has a structural smell: `realize_to_content` in
[src/eval.rs](src/eval.rs) produces a *hybrid* `Content` tree. Most nodes are
in realized form (what Typst would actually render), but `EquationElem`,
`HeadingElem`, and every slot-container node (`ListElem`, `TableElem`,
`FigureElem`, …) are *swapped back* to their pre-realization form via a
span-keyed lookup (`collect_preserved_by_span` / `restore_preserved`,
[eval.rs:218-389](src/eval.rs#L218-L389)). The result is a tree whose node
types are inconsistent with what realization actually produced — handy for
the diff (which needs structural element types) but lossy (realized
counter values, show-rule expansions, etc. inside the swapped subtree are
discarded) and brittle.

Two concrete brittleness symptoms have been observed:

1. **Span uniqueness can't be guaranteed.** A function body that expands N
   times yields N realized nodes sharing one source span. The fix
   ([eval.rs:218-233](src/eval.rs#L218-L233)) is a `HashMap<Span,
   VecDeque<Content>>` consumed in document order — a workaround for the
   wrong addressing scheme. Discussed in detail in
   [docs/walkthrough.md §15 Bug 1](docs/walkthrough.md).
2. **Slot machinery is fragile.** [src/content_slots.rs](src/content_slots.rs)
   is a 1000-line file hand-encoding "for each element type, here are its
   named text-bearing sub-positions." Several bugs the user recalls fixing
   as special cases originated from this registry (recognizing new
   container types, getting wrapper recursion right, infinite-recursion
   traps like the `ParBody` issue in
   [content_slots.rs:200-214](src/content_slots.rs#L200-L214)).

This plan replaces the hybrid tree with a parallel **annotated tree**: each
realized node is paired with a small `Annotation` carrying its semantic
identity (pre-realization element type, slot map). The realized side is
preserved unchanged so the rendering pipeline gets exactly what Typst
produced. The diff reads semantics off annotations instead of from
swapped-in element types. The span-keyed swap goes away entirely; slot
information becomes a per-node annotation rather than a per-type lookup.

The user has explicitly agreed to the read-only invariant: annotations are
built once at eval time and not mutated thereafter. Phases that need to
attach further state (e.g. diff status) build a new tree shape, e.g.
`AnnotatedContent` → `AnnotatedContentWithStatus`.

## Approach

### New data model

Add a module [src/annotated.rs](src/annotated.rs):

```rust
/// A realized Content node together with semantic information recovered
/// from the pre-realization tree.
pub struct AnnotatedContent {
    /// The realized content as Typst produced it. This is what gets rendered.
    pub realized: Content,
    /// Semantic information about this node, derived once at eval time.
    pub annotation: Annotation,
    /// Children, mirroring the descent points of the realized tree.
    /// Empty for leaves and for nodes we don't descend into.
    pub children: Vec<AnnotatedContent>,
}

pub struct Annotation {
    /// The pre-realization element type, if this node corresponds to one
    /// of the structural elements we track. `None` for plain text, spaces,
    /// realized-but-anonymous wrappers, etc.
    pub semantic_kind: Option<SemanticKind>,
    /// Semantic slots inside this node, with paths *into the children vec*
    /// of this AnnotatedContent (not into the realized Content directly).
    /// Empty for non-container nodes.
    pub slots: Vec<SemanticSlot>,
    /// Footnote marker info, if this node is the realized site of a
    /// FootnoteElem (replaces the `restore_footnote_markers` walk).
    pub footnote: Option<FootnoteInfo>,
    /// Pass-through span for diagnostics. Not used as a lookup key.
    pub span: typst::syntax::Span,
}

pub enum SemanticKind {
    Paragraph, Heading, RawBlock,
    List, Enum, Terms,
    Table, Grid, Stack,
    Figure, Footnote, Quote, Equation,
    Wrapper(WrapperKind),  // Align/Pad/Place/Columns/Box/Block/Rect/Circle/Ellipse
}

pub struct SemanticSlot {
    pub label: SlotStep,           // reuse existing SlotStep enum
    pub child_index: usize,        // index into AnnotatedContent.children
}
// No pre-realization body stored: the annotated child at children[child_index]
// already carries everything the diff needs (its own realized content + its
// own annotation + its own children). Avoids per-slot Content duplication.
```

The crucial property: `children[i]` corresponds to descending into a
specific position of `realized`. The slot map tells us which children
indices are semantically meaningful and what they're called.

### How the annotated tree gets built

Add `eval::annotate_realized` (replaces `restore_preserved` +
`restore_footnote_markers`). It takes the pre-realization tree and the
realized tree and walks them *together*, matching by structural position
where the shapes align and by span where they diverge (e.g. realized
descended into a `BlockElem(GridElem(…))` that was originally a
`ListElem`).

Pseudocode:

```rust
fn annotate_realized(
    pre: &Content,        // pre-realization
    realized: &Content,   // post-realization
) -> AnnotatedContent
{
    // 1. If `pre` is a structural element we care about (list, table, …),
    //    locate it inside `realized` by walking down through realized
    //    wrappers (BlockElem, GridElem, …) and build a slot map from
    //    pre's typed children to indices in the realized children list.
    //
    // 2. If `pre` is a transparent wrapper (SequenceElem, StyledElem,
    //    ParElem), descend pairwise into pre's children and realized's
    //    children, building AnnotatedContent for each.
    //
    // 3. If `pre` is a leaf (TextElem, SpaceElem), wrap realized verbatim.
}
```

This walk is the heart of the refactor. It must produce one
`AnnotatedContent` per realized node. The mapping from pre's typed children
to realized children indices is element-type-specific (e.g. for `ListElem`,
each `ListItem` typically maps to one realized child of the inner
`GridElem`; for `FigureElem`, body and caption map to specific positions).
This per-element knowledge is what `content_slots.rs` already encodes —
we'll port it into a `pre_to_realized_slot_map(pre, realized) -> Vec<(SlotStep, usize)>`
helper rather than the current `extract_slots` registry.

### Handling one-to-many expansion

A single pre-realization node routinely corresponds to multiple realized
nodes: counter values get injected, show rules add prefix/suffix text,
list items gain markers, container elements get wrapped in extra layers.
The walker must produce one `AnnotatedContent` per realized node while
giving an unambiguous answer to "which of those carries the
`semantic_kind`?" Three patterns occur, each with a defined rule:

**Outer wrapping** (e.g. `ListElem` → `BlockElem(GridElem(…))`,
`HeadingElem` → `ParElem(…)`). The pre node's element type changes
during realization and extra wrappers appear around it. Only the
*outermost* realized node a pre node maps to carries `semantic_kind`;
intermediate wrappers (the inner `GridElem`, etc.) are anonymous. The
slot indices on the outer `Annotation` point into the annotated tree's
children vec — which mirrors the realized tree's children vec — so a
slot lookup naturally crosses the intermediate wrappers without needing
to address them separately. Element-specific descent rules (one per
supported container type) encode the wrapper chain Typst is known to
produce.

**Inner injection** (counter values, list markers, show-rule prefix
text). The realized form has more interior nodes than the pre form, but
the structural element is still recognizable at the outer level — e.g.
`HeadingElem(TextElem("Intro"))` with numbering becomes
`ParElem(SequenceElem(TextElem("1"), SpaceElem, TextElem("Intro")))`.
The outer `ParElem` already carries `SemanticKind::Heading` per the
outer-wrapping rule. Every realized child inside it — including the
preserved `TextElem("Intro")` that originated in the pre body — is
anonymous (no `semantic_kind`). The semantic identity lives at the
outer level; the diff treats the heading as a single block whose
realized inline text gets word-diffed via the kept
`replace_text_container`. No need to identify which inner realized
child *is* the original body.

**Sibling-level expansion** (rare; user show rules like
`#show heading: it => [#it; #it]` that yield multiple siblings from one
pre node). Pre's `SequenceElem` has N children, realized's has M > N.
The walker descends pairwise where shapes align; on count divergence it
falls back to span-based child alignment — for each pre child, find the
realized child(ren) whose `Content::span()` matches and pair them in
document order. Unmatched realized children become anonymous annotated
nodes. Spans here are a *positional hint within a span group*, not a
primary key: when several children share a span (the Bug 1 collision
class), document-order pairing within the matching span group still
yields the correct match. No `HashMap<Span, …>` collapse.

These rules cover every realization shape Typst currently produces. The
brittleness boundary is the element-specific descent rule: if a
user-defined show rule fundamentally restructures a slot-bearing element
(e.g. `#show list: it => block[…]` that doesn't preserve the
`BlockElem(GridElem(…))` shape), the descent rule fails to recognize the
realized form and returns no slots. The block then becomes opaque,
falling through to flat word-diff in tier 4 below — which is also
today's behavior on such documents, so no regression.

For elements where the realized form is *opaque* and we can't reliably
locate slot positions, `slots` is empty. This is the fallback chain when
two blocks pair as `Replace` at the block-LCS stage:

1. **Equation pair → atomic substitution.** If both nodes have
   `SemanticKind::Equation`, produce a `Modified` node with empty
   `word_ops`. annotate.rs renders this by wrapping old in `CancelElem`
   and old + new is the atomic-equation behavior the codebase already has
   ([annotate.rs:236-241](src/annotate.rs#L236-L241)). No
   sub-expression diff is attempted.
2. **Container pair with matching slot shapes → slot recursion.** Both
   nodes' `annotation.slots` have the same `SlotStep` labels in the same
   order. Recurse: diff each pair of slot children (an annotated child of
   old vs. the corresponding annotated child of new) using the same
   logic, producing per-child statuses on the resulting tree. The
   top-level node's status is `HasChangedDescendants` (its outer realized
   shape is unchanged; changes live on the children). Children that were
   actually unchanged hash-equal short-circuit to `Unchanged`.
3. **Container pair with mismatched shapes → slot-level LCS.** Old has
   3 list items, new has 4. Old's slots are `[ListItem(0), ListItem(1),
   ListItem(2)]`; new's are `[ListItem(0), ListItem(1), ListItem(2),
   ListItem(3)]`. Run the same block-level LCS algorithm
   (`HashableContent` + `similar`) on the *slot children's annotated
   trees*, then apply edit-zone matching to pair up adjacent
   delete/insert as `Replace`. Recurse into each replace. Result: a
   `DiffNode` for the list with `status: HasChangedDescendants` and
   children carrying per-item statuses — some `Unchanged`, some
   `Inserted`, some `Deleted`, some `Modified`. This is a **strict
   improvement** over today's behavior, where any shape mismatch falls
   back to flat word-diffing the entire block (per walkthrough §16). The
   new design enables this naturally because slot children are already
   annotated trees suitable for the same diff machinery.
4. **Non-container blocks (headings, paragraphs, raw blocks) → word
   diff.** Extract tokens from the realized form and run word LCS.
   Top-level node gets `Modified(word_ops)`.

**Behavior change advisory.** Tier 3 changes diff output for documents
where a list/table/etc. gained or lost items. Today such cases produce
flat strikethrough + green over the whole list. The new design produces
per-item insertions/deletions. The corpus contains such cases (e.g.
`19-list-item-added`, `69-nested-list-item-inserted`). **Plan
recommends two-phase rollout**:

- Phase A (refactor): preserve today's behavior exactly. Tier 3 falls
  through to tier 4 (flat word diff on the whole block) just like today.
  All corpus and unit tests pass unchanged.
- Phase B (improvement): enable slot-level LCS. Re-baseline the affected
  corpus outputs. Commit the new outputs after visual inspection.

So "no slots" doesn't mean "give up." It means "don't recurse via
slots" — word diff still applies for text-bearing blocks, and equations
get their existing atomic treatment.

### Eval pipeline

[src/eval.rs](src/eval.rs) changes:

- `eval_to_realized_content(world) -> Result<AnnotatedContent>` (new
  return type).
- `realize_to_content` still calls `ROUTINES.realize` to produce the
  realized `Content`, but the post-processing changes from
  "restore_preserved + restore_footnote_markers + style wrapping" to
  "annotate_realized + style wrapping".
- Delete: `collect_preserved_by_span`, `restore_preserved`,
  `restore_footnote_markers*`, `is_footnote_marker*`,
  `is_footnote_scaffold`. Their work is subsumed by `annotate_realized`,
  which records footnote info as an annotation instead of swapping the
  marker node.
- Keep: `layout_introspector`, `normalize_list_item_runs` (still needed
  pre-realization to canonicalize bare item runs into containers),
  `page_styles` / `non_page_styles` / `marginal_styles`.

The footnote handling specifically: walk the pre-realization tree once
to collect `FootnoteElem` nodes in document order; walk the realized tree
and at each footnote-marker site, attach a `FootnoteInfo { body: ... }`
annotation. The diff then sees the marker as having structured footnote
content available, no number-text matching required.

### Diff pipeline

[src/diff.rs](src/diff.rs) changes:

- `DiffBlock { content: Content, page_styles: Styles }` becomes
  `DiffBlock { node: AnnotatedContent, page_styles: Styles }`.
- `extract_block_units` walks the annotated tree. The block-vs-inline
  classification uses `annotation.semantic_kind` instead of
  `content.is::<HeadingElem>()` etc. The inline-styled-wrapper case
  (regression: [Bug 2](docs/walkthrough.md)) reads cleanly: if a node's
  semantic kind is `Paragraph` or a transparent wrapper of inlines, treat
  it as inline; otherwise as a block.
- `extract_slots(node: &AnnotatedContent) -> &[SemanticSlot]` becomes
  a direct accessor of `node.annotation.slots`. No per-call walking.
- `diff_slots` operates on annotated children directly. The same-shape
  check compares `SemanticSlot` paths; matching slots recurse into the
  child `AnnotatedContent`s.
- `HashableContent` continues to wrap `Content` for the LCS step (we
  compare realized content for equality, which is what we want — two
  blocks should be considered structurally equal iff they render the
  same; this feeds into the `Unchanged` status determination).

The diff result becomes a tree, eliminating `DiffResultOp::ModifiedSlots`
and `SlotDiff` entirely:

```rust
pub struct DiffResult {
    pub blocks: Vec<DiffNode>,
    pub root_styles: Styles,
}

pub struct DiffNode {
    pub node: AnnotatedContent,
    pub status: NodeStatus,
    pub children: Vec<DiffNode>,   // mirrors node.children
}

pub enum NodeStatus {
    /// This entire subtree is byte-equal to its counterpart on the other
    /// side. Detected by hashing `node.realized` and comparing. The
    /// rendering phase can emit the realized content as-is and skip
    /// recursing into `children`.
    Unchanged,
    /// This node's outer structure matches, but at least one descendant
    /// has a non-`Unchanged` status. The rendering phase emits the
    /// realized node and recurses into `children` to find decorations
    /// to apply at deeper positions.
    HasChangedDescendants,
    /// This block was present in old but not in new.
    Deleted,
    /// This block was present in new but not in old.
    Inserted,
    /// This node's text changed; word-level edits are in the vec.
    Modified(Vec<WordOp>),
}
```

The naming distinction matters. The old `DiffResultOp::Equal` conflated
two things: "this block is unchanged" and "this block's wrapper is
unchanged but its slots contain changes." In a tree-shaped diff the
second case is far more common (a list whose one item changed is
"`HasChangedDescendants` at the list level" — the list itself is fine).
Conflating them would force every consumer to look inside before
trusting an "Equal" verdict. Separating them lets the renderer
short-circuit cleanly on `Unchanged`.

**Hash-based Unchanged detection** is a free optimization the
annotated tree enables. When diffing two `AnnotatedContent` subtrees:

1. Hash both `realized` contents (cheap — Typst's `Content` is already
   `Hash`). If equal, return `DiffNode { status: Unchanged, children: [] }`
   immediately. No recursion, no per-child Vec to allocate.
2. Otherwise, look at semantic kinds and slot shapes to decide between
   `HasChangedDescendants` (recurse), `Modified` (word-diff), or — at
   the top level — `Deleted` / `Inserted` (from the block-LCS verdict).

This short-circuit isn't possible in today's design because there's no
tree to short-circuit on — the `Vec<DiffResultOp>` is flat and every
block goes through `HashableContent`-based equality at the LCS stage
regardless.

Slot-level changes are expressed by the tree itself: a list whose item
2 was edited produces a top-level `DiffNode` with `status:
HasChangedDescendants` and `children` where most child statuses are
`Unchanged` but child[3] has `status: Modified(word_ops)`. No
`ModifiedSlots` variant, no separate `SlotDiff` struct — the recursion
falls out of the tree shape.

The block-LCS phase still produces a flat sequence of intermediate
operations (the `BlockOp` enum already in [src/diff.rs](src/diff.rs):
`Equal / Delete / Insert / Replace` — names internal to the LCS step,
not exposed in the final `DiffResult`). The conversion to `DiffNode`
happens at the seam: a `BlockOp::Equal` becomes a `DiffNode` with
`status: Unchanged`; a `BlockOp::Delete` / `Insert` becomes `Deleted` /
`Inserted`; a `BlockOp::Replace(old, new)` either recurses (matching
slot shapes → `HasChangedDescendants` at top + per-child statuses) or
word-diffs (no slots or mismatched shapes → leaf-style `Modified` with
word ops).

### Annotation pipeline (output construction)

[src/annotate.rs](src/annotate.rs) changes:

- `build_annotated_content` walks `DiffResult` and produces a final
  `Content` tree by reading `node.realized` and applying colors based on
  status.
- `apply_fill_inside` (the tight-list-spacing fix at
  [annotate.rs:311-324](src/annotate.rs#L311-L324)) becomes simpler:
  instead of calling `extract_slots(content)` to find sub-positions,
  walk the annotated children. For each child marked as a semantic slot,
  apply the fill to *its realized content*; the outer realized element
  is emitted bare. Same behavior, cleaner addressing.
- `replace_text_container` and slot-splicing operations construct fresh
  `Content` from `realized` plus colored insertions; they don't need
  annotations on the output (which is a plain Content tree fed to render).

### Content slots module

Audit of [src/content_slots.rs](src/content_slots.rs)'s 16 public/internal
items, deciding each one's fate:

| Item | Today's role | Fate |
|---|---|---|
| `SlotStep` enum | Slot identity | **Keep** — `SemanticSlot.label: SlotStep` |
| `ContentSlot` struct | `(path, content)` tuple from `extract_slots` | **Delete** — replaced by `SemanticSlot` |
| `extract_slots(&Content)` | Find addressable sub-positions on bare Content | **Delete** — callers read `node.annotation.slots` |
| `is_slot_container(&Content)` | Used only by `collect_preserved_by_span` to decide what to preserve | **Delete** — the preservation walk is replaced by `annotate_realized` |
| `normalize_list_item_runs` | Wrap bare ListItem siblings into ListElem pre-realization | **Keep** — still runs once, pre-realization |
| `collect_slots` + helpers (`collect_table_slots`, `collect_grid_slots`, `wrapper_body`, `push_slot`, `collect_*_item_slot*`) | Build `Vec<ContentSlot>` per element type | **Move** to `annotated.rs` as the per-element `pre_to_realized_slot_map` builder. Same per-type knowledge, different output shape (`Vec<SemanticSlot>` with child indices into the AnnotatedContent's children, instead of paths into a bare Content). |
| `replace_slot` + table/grid helpers | Splice a replacement into a path inside a Content | **Reduce to one caller, then evaluate.** Today used in annotate.rs's `apply_fill_inside` and `restore_preserved`. The restore-preserved use disappears. The annotate.rs use becomes "walk annotated tree, recursively build output Content from `realized` while substituting colored bodies at slot children" — a fold, not a path-rewrite. If that fold reads cleanly, `replace_slot` can be **deleted**; if it reads worse than path-based splicing, keep `replace_slot` for the annotate phase only. Decide during Stage 4 implementation, not now. |
| `replace_inline_content(&Content, &Content)` | Inline-content splice used by annotate.rs's `replace_text_container` | **Keep** — orthogonal to slot machinery, still needed for grafting word-diff results into heading/paragraph bodies |

Net: the module shrinks from ~1000 lines to roughly: `SlotStep`,
`normalize_list_item_runs`, `replace_inline_content`, and possibly a
slimmed `replace_slot`. Everything else moves into `annotated.rs` or
disappears.

### Staging

This is a large refactor. Break into commits:

1. **Add `annotated.rs` with the types and `annotate_realized` helper.**
   No callers yet. Unit tests construct small pre+realized pairs and
   assert the produced AnnotatedContent has the expected slot maps and
   footnote info.
2. **Switch `eval_to_realized_content` to return `AnnotatedContent`.**
   Provide a `.realized` accessor used by everything downstream. Diff
   and annotate phases still read `.realized` and call the existing
   `extract_slots(&Content)`. All existing tests must still pass.
3. **Migrate `diff.rs` to read semantics from annotations.** Replace
   `is::<HeadingElem>()` / `extract_slots(content)` calls with
   `node.annotation.semantic_kind` / `node.annotation.slots`.
   `DiffBlock` and `DiffResultOp` switch to carrying `AnnotatedContent`.
4. **Migrate `annotate.rs` to consume the annotated diff result.**
   `apply_fill_inside` uses annotated children.
5. **Delete the swap-back machinery** in `eval.rs` (`collect_preserved_by_span`,
   `restore_preserved`, `restore_footnote_markers*`) and shrink
   `content_slots.rs` (drop `extract_slots(&Content)`, `is_slot_container`,
   per-element collectors).

Each stage is independently testable: the corpus and unit tests should
continue passing at every commit.

## Files to modify

- **New**: [src/annotated.rs](src/annotated.rs) — `AnnotatedContent`,
  `Annotation`, `SemanticKind`, `SemanticSlot`, `FootnoteInfo`,
  `annotate_realized`, `pre_to_realized_slot_map`.
- [src/eval.rs](src/eval.rs) — `eval_to_realized_content` returns
  `AnnotatedContent`; delete `collect_preserved_by_span`,
  `restore_preserved`, footnote-marker walk; new
  `annotate_realized` call in `realize_to_content`.
- [src/diff.rs](src/diff.rs) — `DiffBlock`, `DiffResultOp` carry
  `AnnotatedContent`; block extraction and slot diffing read annotations.
- [src/annotate.rs](src/annotate.rs) — consumes annotated diff result;
  `apply_fill_inside` uses annotated children.
- [src/content_slots.rs](src/content_slots.rs) — shrinks; helpers move
  into annotated.rs; `extract_slots(&Content)` and `is_slot_container`
  deleted.
- [src/lib.rs](src/lib.rs) — register the new module; update the
  pipeline doc comment showing `AnnotatedContent` flowing through.
- [docs/walkthrough.md](docs/walkthrough.md) — §4 (eval) and §11 (slots)
  rewritten around the annotated tree; §15 Bug 1 marked as
  architecturally impossible.

## What this preserves vs eliminates

**Eliminates**:
- The span-key collision class entirely (Bug 1 in walkthrough §15) — we
  no longer key anything by span.
- The realize-time hybrid: realized tree stays as Typst produced it.
- Per-call walking in `extract_slots` — slot info is cached on nodes.
- Footnote-marker number-string matching (currently fragile,
  [eval.rs:349-353](src/eval.rs#L349-L353)).

**Preserves**:
- The two-level diff algorithm (block LCS → zone matching → word LCS or
  slot recursion). Unchanged.
- Similarity scoring, page-style stickiness, compact-substitutions mode,
  inline-styled wrapper handling, all corpus-passing behaviors.
- `SlotStep` vocabulary and the per-element knowledge of which sub-
  positions exist. Just relocated from a runtime registry to per-node
  annotations.

## Verification

End-to-end:

1. `cargo build` — confirms the new types compile across the workspace.
2. `cargo test` — every existing unit and integration test must pass.
   Particularly important:
   - `repeated_function_expansions_with_same_span_keep_their_own_content`
     ([tests/integration.rs:457-498](tests/integration.rs#L457-L498)) —
     should now be trivially true since spans aren't lookup keys.
   - `restore_preserved_consumes_same_span_values_in_order`
     ([src/eval.rs:511-526](src/eval.rs#L511-L526)) — the function is
     gone; rewrite as an `annotate_realized_handles_repeated_function_expansions`
     test against the new walker.
   - All `content_slots.rs` tests — port to test
     `pre_to_realized_slot_map` and the `Annotation.slots` accessor.
   - `inline_styled_wrapper_does_not_fragment_paragraph_into_multiple_blocks`,
     `mixed_body_inline_change_detected_and_nested_structure_preserved`,
     `inserted_list_block_stays_bare_list_not_styled_wrapper`,
     `inserted_parbreak_is_not_wrapped_in_styled_elem` — must still pass.
3. `tests/run_corpus.sh` — 48 corpus pairs, each producing visual PDF
   output. Run `--verbose` and eyeball spot-check at least the cases
   covering each `SemanticKind` (list edit, table cell edit, figure
   caption edit, footnote edit, heading edit, equation change, deeply
   nested structures).
4. New unit tests in [src/annotated.rs](src/annotated.rs).

   **Relation to existing tests.** The tests below are not additive —
   they *replace* unit tests of the old machinery while preserving the
   same guarantees:
   - `restore_preserved_consumes_same_span_values_in_order`
     ([src/eval.rs:511-526](src/eval.rs#L511-L526)) → replaced by
     `annotate_handles_repeated_function_expansions_with_distinct_content`.
   - `restore_footnote_markers_*` ([src/eval.rs:551-588](src/eval.rs#L551-L588),
     3 tests) → replaced by
     `annotate_footnote_marker_carries_footnote_info_with_body`
     and parameterized variants for styled markers and non-matching numbers.
   - `collect_preserved_by_span_keeps_multiple_values_for_same_span`
     ([src/eval.rs:497-508](src/eval.rs#L497-L508)) → made obsolete by
     the walker design (spans are no longer lookup keys).
   - `restore_preserved_recurses_into_slot_container_children`
     ([src/eval.rs:528-542](src/eval.rs#L528-L542)) → replaced by the
     per-element `annotate_*_maps_*` tests below.
   - All `content_slots.rs` tests for `extract_slots` (~10 tests,
     [src/content_slots.rs:758-1062](src/content_slots.rs#L758-L1062))
     → replaced by per-element `annotate_*` tests below, which exercise
     the same slot-extraction knowledge through `pre_to_realized_slot_map`.
   - `replace_slot` tests in `content_slots.rs` → keep if `replace_slot`
     survives (Stage 4 decision); otherwise replaced by tests of the
     annotated-tree fold in `annotate.rs`.

   **End-to-end tests in [tests/integration.rs](tests/integration.rs)
   stay unchanged** and serve as the regression baseline. In particular,
   `repeated_function_expansions_with_same_span_keep_their_own_content`
   ([tests/integration.rs:457-498](tests/integration.rs#L457-L498)) must
   still pass — it asserts on the diff output, not on how spans are
   tracked, so it's implementation-agnostic.

   The walker is also exercised transitively by every existing corpus
   test (any change in walker behavior shows up as a corpus diff).

   New tests, organized by what each pins down:

   **Slot-bearing element types (per-type smoke tests for the
   `pre_to_realized_slot_map` builder):**
   - `annotate_list_maps_each_item_body_to_a_realized_child` — 3-item
     `ListElem`; annotation has 3 `SemanticSlot`s with `SlotStep::ListItem(i)`
     labels; each `child_index` selects a realized child whose text
     matches the corresponding item body.
   - `annotate_enum_maps_each_item_body_to_a_realized_child` — same for
     `EnumElem` / `EnumItem`.
   - `annotate_terms_maps_term_and_description_separately` —
     `TermsElem`; verify two slots per term (`Term`, `TermDescription`).
   - `annotate_table_maps_each_cell_by_document_order_index` — 2×2
     table; 4 `TableCell` slots in row-major order.
   - `annotate_grid_maps_each_cell_by_document_order_index` — same for
     `GridElem`.
   - `annotate_figure_maps_body_and_caption_separately` — `FigureElem`
     with caption; two slots.
   - `annotate_quote_maps_body` — `QuoteElem`; one slot.
   - `annotate_footnote_marker_carries_footnote_info_with_body` — text
     with `#footnote[note]`; verify the realized marker site has
     `Annotation.footnote = Some(FootnoteInfo { body: ... })` with body
     text "note". This is the test that replaces
     `restore_footnote_markers_replaces_markers_in_document_order`.
   - `annotate_stack_maps_block_children` — `StackElem` with two block
     children.
   - `annotate_each_wrapper_kind_maps_body` — parameterized over
     `align`, `pad`, `place`, `columns`, `box`, `block`, `rect`,
     `circle`, `ellipse`; one slot per case.

   **Opaque-after-realization elements (verify `SemanticKind` set, slots
   intentionally empty):**
   - `annotate_equation_has_kind_equation_and_no_slots` — `$ a + b $`;
     `SemanticKind::Equation`, `slots: []`. Diff machinery falls back to
     atomic substitution per tier-1 of the fallback chain.
   - `annotate_heading_keeps_kind_heading_through_show_rule` — doc with
     `#show heading: it => [§ #it.body]`; the realized heading site is
     a `ParElem` but annotation has `SemanticKind::Heading`. Lets
     annotate.rs preserve heading-level styling on modified headings.
   - `annotate_realized_show_rule_addition_is_not_a_slot` — counter
     value text added by a show rule (`#it.numbering` expansion) is
     annotated as ordinary text, not as a slot.

   **Shape-change cases (drive the tier-3 slot-level LCS path):**
   - `list_with_inserted_item_produces_per_item_statuses` (Phase B
     only) — old 3 items, new 4 items; result is a `DiffNode` for the
     list with `status: HasChangedDescendants`, one child `Inserted`,
     others `Unchanged`.
   - `list_with_deleted_item_produces_per_item_statuses` (Phase B
     only) — symmetric.
   - `table_with_extra_row_produces_per_cell_statuses` (Phase B
     only) — 2×2 → 3×2; verify the 2 new cells are `Inserted`.
   - `table_with_extra_column_produces_per_cell_statuses` (Phase B
     only) — 2×2 → 2×3; same idea.
   - `list_with_reordered_items_pairs_by_content` (Phase B only) —
     old `[A, B, C]`, new `[B, A, C]`; verify the LCS picks the longer
     common subsequence (either `[A, C]` or `[B, C]`) rather than
     marking every item changed.
   - `phase_a_falls_back_to_flat_word_diff_on_shape_mismatch` (Phase A
     only) — same input as `list_with_inserted_item_…`; verify the
     entire list block has `Modified(word_ops)` and the per-child
     statuses are not populated. This pins down the Phase A
     behavior-preserving contract.

   **Same-span / repeated-expansion (reformulated Bug 1):**
   - `annotate_handles_repeated_function_expansions_with_distinct_content`
     — `#let framed(body) = block[*body* — #body]` called three times;
     verify each invocation's annotation points at its own
     pre-realization body, not the last one's.
   - `annotate_handles_nested_function_expansions_in_loops` —
     `#for x in (1, 2, 3) [#x]`; spans collide; verify each iteration's
     realized node has the right annotation.

   **Tree-shape walking edge cases:**
   - `annotate_paragraph_with_nested_list_descends_into_list` —
     paragraph body contains a list; verify both the paragraph
     annotation (kind `Paragraph`, empty slots) and the nested list
     annotation (kind `List`, populated slots) are produced.
   - `annotate_styled_wrapper_around_inline_sequence_is_one_node` —
     pins down Bug 2 in the new design: the styled wrapper should be
     annotated as part of a single paragraph, not as a separate block.
   - `annotate_deeply_nested_wrappers_all_get_annotations` — `#box[#align(center)[#pad(5pt)[Text]]]`;
     verify each wrapper kind is recorded.
   - `annotate_empty_list_has_kind_list_and_zero_slots`.
   - `annotate_preserves_realized_content_unchanged` —
     `node.realized` byte-for-byte equals what `ROUTINES.realize`
     produced, for several inputs. This is the read-only invariant
     made testable.

   **Diff-result tree (in [src/diff.rs](src/diff.rs)):**
   - `diff_list_with_one_modified_item_produces_has_changed_descendants_with_one_modified_child`
     — pins the tier-2 recursion result shape: outer list is
     `HasChangedDescendants`, unchanged items are `Unchanged`, edited
     item is `Modified`.
   - `diff_unchanged_document_produces_all_unchanged_nodes_recursively`
     — every `DiffNode` in the result tree has `status: Unchanged`.
     Verifies the hash-based short-circuit fires at the top level (no
     `children` populated below `Unchanged` roots).
   - `diff_text_only_change_inside_table_cell_marks_just_that_cell` —
     cell update produces a tree with `HasChangedDescendants` table,
     `HasChangedDescendants` row, `Modified` cell, all other cells
     `Unchanged`. (Phase B only — Phase A still recurses one level via
     the existing slot-shape check, which a single cell change
     satisfies; this test is about deeper structures.)
   - `diff_hash_equal_subtree_short_circuits_to_unchanged` — construct
     two trees where a deeply-nested subtree hashes equal but its
     parent's siblings differ. Verify the subtree's `DiffNode` is
     `Unchanged` with empty `children`, while the parent is
     `HasChangedDescendants`.

   **Output construction (in [src/annotate.rs](src/annotate.rs)):**
   - `annotate_walks_tree_and_emits_colors_at_correct_depth` —
     `DiffNode` with `HasChangedDescendants` self and `Modified` child
     produces an output Content where only the child's text is colored.
   - `annotate_skips_recursion_on_unchanged_subtree` — `DiffNode` with
     `Unchanged` status emits its realized content verbatim without
     descending into `children` (verifying the rendering short-circuit
     companion to the diff-side hash check).
   - `apply_fill_inside_via_annotated_children_keeps_outer_list_bare` —
     pins down Bug 4 (tight-list spacing) in the new design: an
     inserted list block applies green fill to each item's body, not to
     the outer `BlockElem(GridElem(…))`.

## Known limitations of this refactor

- **Does not change the diff algorithm itself.** Slot-shape mismatches
  still fall back to flat word-diff; moves still appear as delete+insert;
  equations are still atomic. The annotated tree makes those decisions
  cleaner, not better.
- **Annotation drift is a new failure mode.** If a future change to
  `annotate_realized` produces an `AnnotatedContent` whose `slots`
  child_indices point at wrong realized children, the diff silently
  reads the wrong content. The walker needs careful unit tests.
- **Memory footprint is dominated by the annotation struct itself,
  not by content duplication.** Typst's `Content` uses refcounted
  internal storage (`Packed<T>`/`Arc`-backed) so cloning a `Content`
  handle is O(1) and shares the underlying element data. The
  `AnnotatedContent` tree holds handles to the same realized data, not
  copies. The actual overhead is the per-node `Annotation` (one enum
  tag, a small `Vec<SemanticSlot>` of `{SlotStep, usize}` pairs, an
  `Option<FootnoteInfo>`, a `Span`) plus a `Vec<AnnotatedContent>` for
  children — tens of bytes per realized node. The pre-realization
  `Content` tree is dropped after `annotate_realized` returns; it's
  only needed during the walk. Worth measuring on a large doc to
  confirm, but the earlier 2-3x claim was wrong.

  **One caveat**: the original draft of this plan put
  `original_body: Content` inside `SemanticSlot`. That would have been
  duplication. The fix is to **not** carry pre-realization bodies in
  the annotation — the annotated children at the slot's `child_index`
  already carry whatever the diff needs to recurse into. The
  `SemanticSlot` struct is just `{label: SlotStep, child_index: usize}`.
