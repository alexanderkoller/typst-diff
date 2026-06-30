# Plan: realized annotated tree with semantic descendant paths

## Summary

Refactor the current list/container traversal fixes around a stricter
annotated-tree invariant:

- `AnnotatedContent` mirrors the realized Typst tree closely enough to rebuild
  it by folding children back into each node's `realized` content.
- Semantic metadata is attached during annotation construction.
- Logical slots point to descendant paths inside the annotated realized tree.
- Diffing and rendering consume those paths; they do not rediscover the mapping
  between logical structure and realized structure later.

This is a refactor, but it should enable future behavior improvements for
nested lists, tables, figures, quotes, terms, wrappers, and other containers.
Worst-case behavior remains local: if a container cannot expose meaningful
semantic slots, the opaque realized node itself is marked inserted, deleted, or
modified.

## Corrected Core Invariant

The previous immediate-child version is not robust enough. Typst realization can
change structure substantially, for example:

```text
pre:      ListElem(items...)
realized: BlockElem(GridElem(cells...))
```

In that case the outer realized node should still carry
`SemanticKind::List`, but list item slots cannot be expressed as immediate child
indices. They must address descendants through the realized wrapper chain.

Replace the slot shape:

```rust
pub struct SemanticSlot {
    pub label: SlotStep,
    pub child_index: usize,
}
```

with:

```rust
pub struct SemanticSlot {
    pub label: SlotStep,
    pub path: Vec<usize>, // path through AnnotatedContent.children
}
```

`path` is an address in the annotated tree, not a separate projection model. The
node at that path owns its own realized subtree, so rendering can still collapse
the diff tree back to plain `Content` by reading `realized` fields.

## Implementation Changes

Add path helpers on `AnnotatedContent`:

- `get_path(&self, path: &[usize]) -> Option<&AnnotatedContent>`
- `get_path_mut(&mut self, path: &[usize]) -> Option<&mut AnnotatedContent>`
- `replace_path(self, path: &[usize], replacement: Content) -> Option<Content>`

Update annotation construction:

- Container mappers in `annotated.rs` must build realized children that reflect
  Typst's realized wrapper structure.
- Each mapper records semantic slots as descendant paths into those children.
- List/enum slots should point through `BlockElem`/`GridElem`/`StyledElem`
  realization wrappers to the realized item body nodes.
- Table/grid slots should point to realized cell body nodes.
- Figure, quote, footnote, terms, stack, and wrapper slots should point to their
  realized body/caption/description descendants.
- Nested containers must be annotated during construction so later phases do not
  need heuristics like "find the one slot-bearing descendant."

Update diffing:

- Recurse over slot sequences by resolving each `SemanticSlot.path` to an
  annotated subtree.
- Matching slot labels recurse into the resolved old/new subtrees.
- Shape-mismatched slot sequences run slot-level LCS over the resolved subtrees.
- Inserted slots use the new slot subtree's `realized` content.
- Deleted slots use the old slot subtree's `realized` content and are emitted at
  the merged semantic position.
- Opaque or unmapped nodes fall back to `Modified` on the node's realized
  content, or whole-node `Inserted`/`Deleted`.

Update rendering:

- `diff_annotated` returns path-addressed `RealizedEdit`s, not a status tree.
- Rendering starts from each block's annotated realized `base` and applies edits.
- `ReplaceAt` replaces an addressed descendant with inserted/deleted/modified
  content.
- `InsertBefore` / `InsertAfter` place deleted old content next to surviving
  new-side anchors.
- `WholeBlock` handles opaque inserted/deleted/modified blocks.
- Do not reconstruct semantic containers. The new document's realized tree
  remains the rendering source of truth.

Remove or demote tactical helpers:

- Remove `find_slot_bearing_descendant_pair`; nested behavior must come from
  annotation-owned paths.
- Unify duplicate `effective_content` / `effective_render_content` into one
  helper that resolves through annotated paths and realized content.
- Stop using equality-search subtree patching as the main mechanism. It may
  remain temporarily as a migration fallback, but path-based replacement should
  be the intended route.
- Remove renderer-side `List | Enum | Table | Grid` reconstruction as primary
  behavior.

## Edge Cases

- Structural expansion is handled by descendant paths. A list realized as
  `Block(Grid(...))` can still expose item slots without flattening the list.
- Insert/delete slot mismatches are handled by diffing slot sequences, not by
  assuming old and new child vectors have equal length.
- Deleted old slots have no location in the new realized tree. They are emitted
  at the merged semantic slot position produced by slot-level LCS.
- If annotation cannot identify semantic slots for a container, the container is
  opaque and diffing marks the realized node itself.

## Tests

Add annotation invariant tests:

- Every `SemanticSlot.path` resolves to an existing annotated descendant.
- List item slots resolve through realized wrapper expansion.
- Nested list semantic nodes exist after annotation construction.
- Table/grid cell slots resolve to realized cell bodies.
- Figure body/caption and terms term/description paths resolve correctly.

Add diff tests:

- Same-shape list item changes recurse via slot paths.
- Inserted/deleted list items use slot-level LCS.
- Nested list changes do not require `find_slot_bearing_descendant_pair`.
- Table row/cell insertions and deletions remain child-level changes.
- Opaque containers fall back to whole-node marking.

Add renderer fold tests:

- Unchanged subtree returns exact realized content.
- Modified descendant path is substituted into the parent realized tree.
- Inserted and deleted slots are emitted in semantic order.
- No semantic container reconstruction is needed for list/table cases.

Run:

- `cargo test`
- `tests/run_corpus.sh --filter 18`
- `tests/run_corpus.sh --filter 19`
- `tests/run_corpus.sh --filter 20`
- `tests/run_corpus.sh --filter 35`
- `tests/run_corpus.sh --filter 64`
- `tests/run_corpus.sh --filter 65`
- `tests/run_corpus.sh --filter 69`

## Assumptions

- `docs/superpowers/speccs/generalization.md` refers to
  `docs/superpowers/specs/generalization.md`.
- The new document's realized tree remains the rendering source of truth.
- Mapping logical structure to realized structure belongs entirely to annotated
  tree construction.
- Descendant slot paths are the minimal extra metadata needed to make the plan
  robust to pre/post-realization structural mismatches.




---


• Fixed the Corpus 18 bug.

  The root cause was that the annotated tree was adding synthetic fallback list-item children when Typst’s realized list node was
  opaque. Rendering then projected both the original realized list and those synthetic children, producing the duplicated flattened
  text. I changed the model so:

  - realized stays the actual Typst-realized node for matching.
  - Annotation::patch_surface carries a structured local edit surface only when realization is opaque.
  - Rendering applies edits to that patch surface and then hands the resulting normal Content to Typst.
  - Lists and tables now expose stable slot paths without adding extra renderable children.
