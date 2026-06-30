# Code Consistency Review

This review focuses on consistency with the project's design principle: prefer
clean, general structural solutions over fallbacks, heuristics, and special-case
patches. It is intentionally a review only; no implementation files were
changed.

## Executive Summary

The codebase has a strong current direction: `AnnotatedContent` plus semantic
slots gives the diff a single structural representation that can explain lists,
tables, figures, footnotes, wrappers, and page regions. Most recent tests also
encode useful contracts around slot-local edits and rendering.

The remaining inconsistency is that several older bridge mechanisms still sit
beside the annotated-tree model. The most important cleanup targets are:

- plain-text matching used as identity,
- positional and uniqueness fallbacks used when structural paths are absent,
- patch surfaces that inject `ParbreakElem` based on nested-list conditions,
- rendered page-region parsing built from source-string scanning and coordinate
  bands,
- duplicate-edit pruning by textual signature.

These mechanisms may be pragmatic, but they are exactly the kind of remnants
that can make future behavior harder to reason about.

## Invariant Map

The cleanup work should be guided by invariants, not only by removing code that
looks suspicious. These are the contracts I found in the current design:

| Invariant | Current pressure points |
| --- | --- |
| `SystemWorld` is a stable snapshot for one evaluation. | Package downloads and Git snapshot mode are external inputs; tests should keep these separate from diff correctness. |
| `AnnotatedContent.realized` preserves Typst output verbatim. | Patch surfaces and edit application must not blur "what Typst produced" with "what we render after edits." |
| Every semantic slot path resolves through `children`. | Index-based slot mapping and opaque realization can produce missing or misleading paths. |
| `semantic_kind` gates structural recursion. | Unique-descendant fallbacks bypass direct ownership and infer locality after the fact. |
| Patch surfaces are renderable local edit surfaces. | Leading `ParbreakElem` injection and pre-container grafting encode visual repairs rather than a named patch-surface contract. |
| Page styles are separate from ordinary content styles. | Rendered page-region diffs synthesize new style content through a separate string-based path. |
| Plain text is similarity, not identity. | Block pairing, layout matching, slot LCS, and partial container mapping all use visible text for decisions that can affect ownership. |
| The edit script is a render contract. | Duplicate pruning and failed insert/replace paths can silently drop edits before rendering. |
| Provenance beats recognition. | Equation origins, footnote markers, and rendered-region wrappers are currently recognized by broad predicates or source-string scans. |

The findings below are organized around places where the code bends one of
these invariants.

## Findings

### 1. Positional fallback in pre/realized sequence pairing can attach semantics to the wrong realized child

Location: `src/annotated.rs`, especially `pair_sequence_by_span` around lines
370-420.

When pre-realization and realized sequence child counts differ, the code first
tries to align by effective span. If no span match exists, it falls back to
pairing the pre child with the next realized child by position.

This violates the otherwise clean invariant that annotation records semantic
origin without guessing. A wrong positional pair is worse than an anonymous
realized leaf because it gives the diff a false structural explanation. The
risk is highest with show rules, wrappers, repeated function expansions, or
generated content whose spans are detached or rewritten.

Invariant affected: semantic origin must be proven before it is attached to a
realized child.

Suggested direction:

- Make the mismatch behavior explicit: pair only when a structural rule proves
  the relationship.
- If no proof exists, leave the realized child anonymous and preserve the pre
  child only in a clearly marked patch/edit surface if needed.
- Add an annotation-level test with two pre children and two realized children
  whose spans do not match and whose texts are swapped; assert that semantic
  kinds are not silently attached to the wrong realized node.

### 2. Container slot mapping relies on child counts and document-order paths instead of explicit container semantics

Location: `src/container_ops.rs`, especially `map_slot_parts` and
`collect_leaf_block_child_paths` around lines 586-754.

`map_slot_parts` derives realized paths with `collect_leaf_block_child_paths`
and then zips semantic slot parts onto those paths by index. This is general in
shape, but the mapping rules are still largely "find leaf block paths in
document order." It works for many containers because tests keep the realized
shape aligned, but it is fragile for tables/grids with headers, footers,
spans, gutters, or non-cell children.

Wrappers now have an explicit exception to this pressure point:
`WrapperOps::map_slots` maps `WrapperBody` to the direct realized wrapper body
and does not descend to the first leaf. That addresses the corpus `39`
replacement-tokenization bug, but the generic leaf-path concern still applies
to other container families until they own similarly explicit path semantics.

Suggested direction:

- Keep `ContainerOps` as the single owner, but make each container responsible
  for mapping its own semantic slots to realized paths.
- For table/grid, introduce an explicit cell-address model rather than a flat
  cell index where possible.
- Add tests for table/grid header and footer insertion/deletion. Current
  `ordinary_table_insert_index` and `ordinary_grid_insert_index` return `None`
  when the target anchor is inside a header/footer, so deleted or inserted
header/footer cells may silently fail to render as structured edits.

Invariant affected: every advertised semantic slot path should resolve to the
intended child, and container-specific semantics should be owned by the
container operation rather than reconstructed generically from leaf order.

### 3. Opaque realization patch surfaces are a fallback path, not a stable abstraction

Location: `src/container_ops.rs`, especially lines 718-854.

When realized paths are fewer than semantic parts, the code switches to
`patch_surface_for_opaque_realization`. It may graft the pre container into a
realized block, or fall back to `opaque_pre_surface`. `opaque_pre_surface`
injects a leading `ParbreakElem` if a nested list is detected.

This is one of the clearest remnants of earlier local fixes. It encodes a
rendering repair inside container mapping, based on one container family. The
same idea appears again in `src/annotate.rs` in `insertion_patch_surface`.

Invariant affected: a patch surface should be a named renderable edit surface,
not an implicit collection of corpus-specific repairs.

Suggested direction:

- Promote "patch surface" to an explicit abstraction with documented variants,
  for example `Realized`, `PreContainer`, `GraftedBlockBody`, and
  `LayoutPreservingSequence`.
- Move layout-boundary insertion into one place with an invariant-driven rule.
- Add tests that distinguish semantic patching from paragraph/list layout
  preservation, so the code no longer needs list-specific checks in multiple
  modules.

### 4. Unique-descendant fallback is a useful bridge but not a general diff rule

Location: `src/diff.rs`, especially `find_unique_changed_slot_pair` and
`find_slot_bearing_descendant_pair` around lines 2129-2281.

When direct owner matching fails, the diff searches for exactly one changed
slot-bearing descendant and recurses into it. This works for cases like "one
nested list changed inside an outer item," but it makes correctness depend on
uniqueness in the current tree rather than on a structural relation established
during annotation.

The risk is silent widening or narrowing. If a future document has two changed
nested containers, the fallback disappears and the algorithm drops to a word
diff. If two containers are structurally similar, uniqueness may not reflect
the user's intended edit locality.

Invariant affected: block and slot ownership should flow forward from
annotation and extraction, not be rediscovered by searching for a unique changed
descendant.

Suggested direction:

- Store owner relationships and block ownership during annotation/block
  extraction so `diff_annotated` can ask "which semantic node owns this block?"
  directly.
- Treat "unique changed descendant" as a temporary compatibility path and add
  tests that pin its current behavior before replacing it.

### 5. Plain text is used as identity in several places where it should only be a similarity signal

Locations:

- `src/diff.rs` `pair_edit_zone`, lines 747-763.
- `src/diff.rs` `layout_content_matches`, lines 2046-2052.
- `src/diff.rs` `diff_slot_edits_lcs`, lines 2415-2429.
- `src/container_ops.rs` `map_unique_partial_item_container`, lines 756-790.

Plain text is an appropriate similarity signal, but it is not a stable identity:
two table cells can contain the same text with different links, labels,
equations, styles, or referenced targets. The current code sometimes uses plain
text to decide ownership or slot alignment, not just whether a replacement is
plausible.

Invariant affected: visible text can score similarity, but structural identity
must come from content structure, semantic slots, provenance, or explicit edit
ownership.

Suggested direction:

- Define comparable keys per layer:
  - block identity key,
  - semantic slot key,
  - visible text similarity key,
  - structural equality key.
- Keep plain text in the similarity key only.
- Add tests for same visible text but changed link target, label, or equation
  reference inside repeated structures. There are corpus cases for link/label
  changes; a repeated-container variant would probe identity ambiguity better.

### 6. Duplicate edit pruning by text signature can hide legitimate repeated changes

Location: `src/diff.rs`, `prune_duplicate_empty_container_edits` around lines
1576-1612.

This pass removes edits from empty-text container blocks when a matching
signature has already appeared in nonempty blocks or earlier blocks. The
signature is based on single-line base/deleted/inserted text.

This is a post-hoc cleanup, not a structural guarantee. If a document really
has two identical edits in two empty realized containers, one may be hidden. It
also masks upstream ownership ambiguity instead of fixing the duplication at
the point where edit claims are made.

Invariant affected: every edit claim should have an owner. Duplicate
suppression should operate on owner identity, not on rendered text signatures.

Suggested direction:

- Replace text signatures with stable edit ownership IDs derived from annotated
  paths or block ownership.
- Add a regression test with two identical wrapper/list/table edits that should
  both be reported.
- Keep any duplicate suppression local to the ownership conflict it resolves,
  not as a broad pass over finished edits.

### 7. Rendered page-region diffing is a second diff pipeline with source-string parsing

Locations:

- `src/diff.rs` `diff_rendered_root_page_regions`, lines 1785-1823.
- `src/diff.rs` `rendered_region_wrapper` / `authored_align_wrapper`, lines
  1860-1912.
- `src/annotate.rs` `rendered_region_context_content`, lines 490-511.

Rendered page regions are necessary for contextual headers/footers, but the
current implementation has several special mechanisms:

- it extracts text from laid-out frames using coordinate bands,
- it only considers artifact-tagged text,
- it scans source snippets for `align(...)`,
- it synthesizes Typst source strings and evaluates them,
- it uses `.expect("generated rendered region Typst is valid")`.

This path is conceptually separate from the annotated-tree path. It can miss
non-textual region changes, region text outside the 20 percent header/footer
bands, custom wrappers other than simple `align`, and inserted pages because
`changed` is false when the old page does not exist.

Invariant affected: page regions should obey the same provenance and edit-script
contracts as body content, even when their final values are layout-contextual.

Suggested direction:

- Model rendered regions as first-class diff regions with provenance, not as a
  page-style afterthought.
- Replace source-string wrapper detection with content-tree wrapper analysis.
- Avoid string-generated Typst where possible; construct `Content` directly.
- Add tests for:
  - inserted pages with new footer/header text,
  - right/left aligned footers written through helper functions,
  - page regions containing shapes or images,
  - header/footer content outside the current coordinate thresholds.

### 8. The generated Typst snippet path can panic from library code

Location: `src/annotate.rs`, line 511.

`rendered_region_context_content` calls `eval_snippet_to_content(&source)` and
then `expect`s validity. The source is generated internally, but it includes
escaped user-visible text and wrapper markup inferred from source snippets. A
panic during annotation would be a poor failure mode for a CLI/library.

Invariant affected: annotation/render preparation should return a renderable
edit script or a diagnostic error, never panic from generated intermediary
source.

Suggested direction:

- Return `Result<Content>` from the rendered-region construction path and
  propagate errors through the existing `anyhow` pipeline.
- Better yet, build the context content with Typst `Content` constructors rather
  than source strings.
- Add a test with header/footer text containing brackets, hashes, backslashes,
  quotes, and non-ASCII text to ensure the snippet path cannot panic.

### 9. Equation-carrier detection is broad and can mis-assign equation origins

Locations:

- `src/annotated.rs` `is_realized_equation_carrier`, lines 561-565.
- `src/diff.rs` duplicate `is_realized_equation_carrier`, around lines
  499-504.

The carrier test treats any `EquationElem`, functions named `inline` or
`display`, and empty `BlockElem`s as equation carriers. That last clause is
especially broad. If another empty block appears before a realized equation,
source equation origins may be consumed at the wrong leaf.

Invariant affected: equation provenance should be attached by source relation,
not by broad recognition of empty realized shapes.

Suggested direction:

- Deduplicate the carrier predicate.
- Replace the empty-block rule with a more specific marker from Typst's realized
  math shape, or attach equation origins during the pre/realized walk where a
  source relation is still available.
- Add tests with empty blocks adjacent to equations and multiple equations in
  one paragraph.

### 10. Footnote marker matching by visible number is fragile

Location: `src/annotated.rs`, `annotate_footnote_markers` and
`is_footnote_marker_text` around lines 469-580.

Footnotes are assigned by walking realized content and matching a text node
whose text equals the next footnote number. This can confuse ordinary visible
numbers with footnote markers, especially in documents with superscript styling
or generated numeric text near footnotes.

Invariant affected: footnote provenance should identify marker sites, not merely
visible numeric text in document order.

Suggested direction:

- Use Typst metadata, tags, spans, or element provenance if available rather
  than bare text equality.
- At minimum, require marker-specific style/position evidence rather than just
  text.
- Add a test where a visible `1` appears before the first footnote marker and
  assert that the footnote body attaches to the marker, not the visible number.

### 11. Documentation in code and docs is partially stale

Locations:

- `src/eval.rs` doc comment for `eval_to_realized_content` still mentions
  restoring pre-realization `EquationElem` and slot-container nodes by span,
  but the current code builds annotations and attaches equation origins instead.
- `docs/technical.md` references removed names such as `content_slots` and
  older APIs such as `diff_content`.
- `docs/walkthrough.md` is explicitly outdated relative to the annotated-tree
  pipeline.

Suggested direction:

- Either retire stale docs or mark them historical.
- Promote `docs/system-walkthrough-annotated-tree.md` as the current narrative
  walkthrough.
- Update module comments that describe removed algorithms.

## Missing Features And Edge Cases Worth Testing

These are not necessarily bugs, but they are places where the current model
should be made explicit.

- **Moves:** moved paragraphs or list items are represented as delete plus
  insert. If move detection is out of scope, document it and test that moved
  items do not become misleading word substitutions.
- **Repeated identical edits:** identical changed text in two wrappers or two
  table cells should produce two edits, not one pruned signature.
- **Table/grid structure:** inserted/deleted header/footer cells, rowspans,
  colspans, gutters, and alignment-only changes need explicit expectations.
- **Opaque graphics:** diagrams, raw SVG, images, and packages such as Cetz may
  have empty `plain_text()`. Decide whether structural replacement, rendered
  image comparison, or "unsupported opaque change" is the intended behavior.
- **Contextual body text:** page counters and references in normal body content
  may change due to inserted sections. The rendered page-region system handles
  headers/footers only.
- **Link and label identity:** same visible text with changed target should be a
  meaningful change. Repeated same-text cases should test that matching picks
  the correct occurrence.
- **Unicode and segmentation:** tokenization splits on Rust `char` whitespace,
  not grapheme clusters or language-aware words. This may be acceptable, but it
  should be tested for combining marks and CJK text if those are supported.
- **Package/network behavior:** `SystemWorld` downloads packages on demand.
  Tests should distinguish package-resolution failures from diff failures.

## Recommended Cleanup Order

1. Update stale docs and module comments so contributors are not following an
   obsolete span-restoration model.
2. Add invariant probes for the current contracts: realized immutability,
   slot-path resolvability, edit-script renderability, and page-style
   separation.
3. Define explicit identity/comparison keys for blocks and slots. Move plain
   text out of identity decisions.
4. Replace duplicate-edit pruning with structural edit ownership.
5. Consolidate patch-surface behavior and remove duplicated nested-list
   `ParbreakElem` insertion logic.
6. Turn rendered page regions into a first-class region abstraction and remove
   source-string wrapper parsing.
7. Tighten equation and footnote provenance.
8. Expand tests around repeated structures, table/grid headers and footers,
   opaque empty-text content, and contextual changes.

## Review Notes

The strongest part of the codebase is the move toward one annotated-tree model.
Future cleanup should preserve that direction: every exception should either be
absorbed into annotation as a general structural concept or deleted. The most
useful test additions will be small probes at the failing pipeline stage:
annotation shape, block extraction, slot edit script, edit application, or
rendered frame text. That keeps failures actionable and avoids treating PDF
output as the first debugging surface.

In practical terms, every cleanup PR should be able to say which invariant it
strengthens. If the answer is "it handles this one case," the code probably
belongs behind a more general invariant first.
