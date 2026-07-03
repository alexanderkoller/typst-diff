# Lean Deletion Implementation Plan

This plan follows the deep-cut refactor. The premise is that the new
abstractions are generally cleaner than the old code, but the codebase is still
long because compatibility bridges keep older decision paths alive.

The implementation goal is not to add another abstraction layer. The goal is to
make the clean abstractions authoritative and delete the old code they replace.

Current measured baseline:

- Production Rust excluding embedded unit-test modules: about 15,212 lines.
- All `src/*.rs`, including embedded unit tests: 19,010 lines.
- Expected production net reduction from this plan: 1,100 to 1,900 lines.

Rules for every phase:

- Preserve the passing corpus unless a legacy path only worked by unprovable
  guessing.
- Do not update corpus references.
- Do not replace old post-hoc guesses with new post-hoc guesses.
- If provenance is unavailable, produce an explicit unsupported or no-op
  boundary and keep the debt ledger honest.
- Keep tests that protect desired behavior. Delete or rewrite only tests that
  protect behavior intentionally removed as legacy.
- Do not keep functions just because they have direct tests or may be useful.
  Check for every function whether it is useful after the refactor; if not,
  delete it alongside tests that only protect the obsolete private behavior.
- After every major implementation phase, update `TECHNICAL-DECISIONS.md`.

## Phase 0: Baseline And Deletion Inventory

Purpose: make the deletion work measurable and avoid deleting by vibe.

Steps:

1. Record production LOC excluding embedded test modules, all `src` LOC, and
   the current top modules by production LOC.
2. Run the normal gates:

   ```bash
   cargo check --all-targets
   cargo test --all-targets
   bash tests/check_fallback_ledger.sh
   bash tests/run_passing_corpus.sh
   ```

3. Create a short local inventory of the symbols expected to disappear:

   ```bash
   rg "BlockOwnerCursor|EquationOriginBlockCursor|find_annotated_block_owner|collect_block_owner_claims|collect_equation_origin_block_claims" src
   rg "map_slot_parts|map_unique_partial_item_container|unique_realized_wrapper_path|opaque_pre_surface|patch_surface_for_opaque_realization" src
   rg "patchable_surface_for_index|apply_path_edit|rendered_region_source_wrapper|authored_align_wrapper|parse_align_call_alignment" src
   rg "word_or_opaque_replacement_edits|block_context_key_for|block_context_key\\(" src
   ```

Exit criteria:

- Baseline command results are known.
- Baseline production LOC is recorded.
- The deletion symbol list is known before implementation begins.

Estimated net LOC: 0.

## Phase 1: Promote Indexed Attributed Blocks

Clean abstraction to promote: `AttributedBlockStream`.

Problem today: the block diff works on cloned `Content` values. Later, the
stream has to recover ownership by searching for matching realized content,
which keeps `BlockOwnerCursor`, `EquationOriginBlockCursor`, and
`find_annotated_block_owner` alive.

Progress:

- Done: production block ops are indexed, and public/debug `BlockOp` is now an
  adapter over the indexed matcher.
- Done: block-op consumption reads `AttributedBlockStream` by index instead of
  searching for a matching realized block.
- Done: `BlockOwnerCursor`, `EquationOriginBlockCursor`, content-search
  attributed block lookup, duplicate realized stream payloads, and the duplicate
  content-rich edit-zone matcher have been removed.
- Deferred to Phase 8: `collect_block_owner_claims`,
  `collect_equation_origin_block_claims`, and `find_annotated_block_owner` still
  recover attribution from realized content inside stream construction.
  Replacing those requires retained owner/path provenance from annotation or
  block extraction.

Target design:

- Keep `DiffBlock` as the extracted block payload.
- Add a private indexed block operation type for production, for example:

  ```rust
  enum IndexedBlockOp {
      Equal { old: usize, new: usize },
      Delete { old: usize },
      Insert { new: usize },
      Replace { old: usize, new: usize },
  }
  ```

- Keep the current public/test-facing `BlockOp` only as a compatibility view if
  tests still need content-rich operations. Production should walk
  `IndexedBlockOp`.
- Build old and new `AttributedBlockStream`s directly from the same
  non-parbreak block vectors used by the indexed block ops.
- Each stream item must carry:
  - realized block content,
  - primary owner,
  - owner path,
  - owner key,
  - equation origins,
  - footnote bodies,
  - owner summary metadata used for tracing.
- Stream lookup in the block-op loop must be by index, not by content equality.

Implementation steps:

1. Add `IndexedBlockOp` and make `diff_block_units_raw` return indexed ops for
   internal use.
2. Add a small adapter for the existing public/test `diff_blocks_raw` API if
   preserving that shape is less disruptive than changing tests.
3. Change `match_edit_zones_inner` and `pair_edit_zone` to preserve indices.
   Similarity may still score by visible text, but selected replacements must
   carry old/new indices.
4. Change `prepare_diff_inputs` so it stores indexed raw and matched ops.
5. Change the block-op loop to read `old_stream[old_index]` and
   `new_stream[new_index]` directly.
6. Keep owner/equation claim construction inside `AttributedBlockStream`
   construction until Phase 8 introduces attributed block extraction. The old
   cursor objects should be gone, but the remaining claim helpers are deferred
   debt rather than local Phase 1 deletions.
7. Delete:
   - `BlockOwnerCursor`
   - `BlockOwnerMatch` if no longer needed outside temporary transition code
   - `EquationOriginBlockCursor`
   - `attributed_block_index`
   - `take_attributed_block` and `peek_attributed_block` content-search forms

Tests to add or update:

- Repeated identical visible text in two owned containers still maps edits to
  the correct owner.
- Equal repeated blocks are consumed by index, not by first matching content.
- Empty equation carrier next to another empty block does not consume the wrong
  equation origin.
- Existing block-op unit tests continue through the public adapter or are
  rewritten to assert indexed behavior.

Exit criteria:

- No production `rg` hits for the deleted owner/equation cursor symbols.
- Stream items are consumed by index only.
- Remaining realized-content owner/equation claim helpers are documented as
  Phase 8 debt.
- Passing-corpus gate passes.
- `TECHNICAL-DECISIONS.md` records that block attribution is indexed and no
  longer recovered by content matching.

Estimated net production LOC: -300 to -500.

## Phase 2: Make ContainerOps Own Slot Mapping

Clean abstractions to promote: `ContainerOps`, `PatchSurface`, and
`SemanticSlot`.

Problem today: `ContainerOps` names container-specific behavior, but the generic
`map_slot_parts` tail can still invent mappings by leaf order, unique visible
text, and opaque grafting. That means the old mapping model remains active.

Target design:

- `ContainerOps::map_slots` is the only authority for a container's slot paths.
- If a container cannot prove its slot paths, it returns an empty/unsupported
  mapping with no guessed slots.
- Patch surfaces are chosen by container semantics, not by global repair logic.
- Single-item list/enum/terms surfaces are addressed by slot label and source
  part, not by matching the realized text to exactly one source item.

Implementation steps:

1. Change `ContainerOps::map_slots` from an optional override into the main
   mapping route. The default implementation may handle direct one-to-one slot
   paths only when `slot_parts` and `realized_child_contents` have the same
   length and the paths are direct.
2. Implement explicit `map_slots` for:
   - list: `ListItem(i)` maps to direct item path `[i]`.
   - enum: `EnumItem(i)` maps to direct item path `[i]`.
   - terms: `Term(i)` and `TermDescription(i)` map to direct term item paths.
   - table/grid: cell slots map through indexed cell bodies, including header,
     body, and footer cells already exposed by indexed cell helpers.
   - stack: `StackChild(i)` maps to direct stack child paths.
   - quote: `QuoteBody` maps to `[0]`.
   - figure: keep authored figure body/caption patch-surface mapping and
     realized caption/body patch paths.
   - wrappers: keep direct wrapper-body mapping and structural wrapper body
     paths.
3. Replace the unique partial-item path with explicit single-item patch-surface
   construction called from list/enum/terms `map_slots` when the realized output
   represents only one known slot.
4. Remove global mapping fallback behavior that uses leaf-order or text-unique
   matching after a container declines.
5. Delete:
   - `map_slot_parts`
   - `collect_leaf_block_child_paths` if no remaining explicit container mapper
     needs it
   - `map_unique_partial_item_container`
   - `SingleItemPatch` if the single-item logic moves into container-specific
     functions
   - `single_item_patch_surface` as a global helper if replaced by per-container
     functions
   - `patch_surface_for_opaque_realization`
   - `graft_opaque_patch_surface`
   - `opaque_pre_surface`
   - `has_nested_list_container`
   - `unique_realized_wrapper_path`
   - `collect_realized_wrapper_paths`

Tests to add or update:

- Single changed list item renders using a list patch surface without visible
  text uniqueness.
- Single changed enum item preserves enum tightness and numbering.
- Single changed terms entry preserves term/description slot identity.
- Repeated identical list item text does not collapse to the wrong item.
- Table/grid header and footer cell edits either map explicitly or produce an
  explicit unsupported result, never a guessed leaf-order edit.
- Wrapper body mapping works when multiple wrappers of the same kind appear.

Exit criteria:

- `ContainerOps` owns all slot-to-path decisions.
- No production hits for the deleted generic mapping/grafting symbols.
- `FB-007`, `FB-008`, and `FB-009` can be retired or narrowed to a documented
  unsupported boundary.
- Passing-corpus gate passes.
- `TECHNICAL-DECISIONS.md` records that container mapping is container-owned.

Estimated net production LOC: -250 to -450.

## Phase 3: Promote content_tree For Render Path Editing

Clean abstraction to promote: `content_tree`.

Problem today: render application has its own recursive path editor in
`annotate.rs`, plus a fallback that synthesizes a patchable surface from
`node.children` when the declared patch surface does not contain the path. That
keeps post-hoc path repair alive.

Target design:

- `content_tree` owns all realized-content path replacement and insertion.
- `SemanticSlot.patch_path`, when present, is the only translation from logical
  slot path to patch-surface path.
- If a path does not resolve on the patch surface, the edit is unsupported or
  skipped with diagnostics. It must not synthesize a new surface from children.

Implementation steps:

1. Add `content_tree::insert_realized_content_at_path`.
2. Move the generic path edit logic currently in `annotate.rs` into
   `content_tree`, or replace it with calls to:
   - `realized_content_at_path`
   - `replace_realized_content_at_path`
   - `insert_realized_content_at_path`
3. Keep `patch_path_for_logical_path`, but make it the only patch-path
   translation.
4. Delete:
   - local `PathEdit`
   - local `apply_path_edit`
   - `patchable_surface_for_index`
   - the child-sequence fallback that creates
     `Content::sequence(node.children.iter().map(effective_render_content))`
5. Add a diagnostic or trace event when a render edit path fails to resolve.
   Do not silently invent a surface.

Tests to add or update:

- A valid `ReplaceAt` with a `patch_path` applies to the patch surface.
- A missing patch path does not create a synthetic child-sequence surface.
- Insert-before and insert-after still work for sequence/list/enum direct paths.
- Failed path application is observable in debug trace or explicit test return
  behavior.

Exit criteria:

- No production hits for `patchable_surface_for_index` or local
  `apply_path_edit`.
- Path editing code lives in `content_tree` or delegates directly to it.
- Passing-corpus gate passes.

Estimated net production LOC: -50 to -100.

## Phase 4: Promote content_key And Diff Surface Selection

Clean abstractions to promote: `content_key`, `DiffSurfaceEdit`, and
`DiffAreaKind`.

Problem today: the code has a cleaner surface/key vocabulary, but replacement
selection still has old wrappers and local context-key helpers. Some decisions
are trace-labeled as areas without being dispatched through a shared selection
object.

Target design:

- One replacement selection type carries area, surface, and edit content:

  ```rust
  struct DiffSelection<T> {
      area: DiffAreaKind,
      surface: DiffSurfaceKind,
      content: T,
  }
  ```

- Body blocks, slot edits, semantic page regions, and rendered page regions use
  the same selection vocabulary where they do replacement-style diffing.
- `content_key` owns all presentation, context, slot, block, and visible-unit
  keys.
- Plain text remains a similarity input, not ownership identity.

Implementation steps:

1. Add `DiffSelection<T>` or extend `DiffSurfaceEdit<T>` to include
   `DiffAreaKind`.
2. Replace `select_modified_fragment_surface` with a function that returns the
   area+surface selection.
3. Route these callers through the same selection function:
   - block replacement fallback,
   - presentation-changed equal blocks,
   - `modified_fragment_edit_content`,
   - semantic page-region replacement,
   - rendered-region word/segment selection.
4. Move local context-key helpers into `content_key`:
   - `block_context_key_for`
   - `annotated_block_context_key`
   - `semantic_heading_context` if it becomes key comparison
   - `block_context_key`
   - `is_block_context` if only used for context classification
5. Delete:
   - `word_or_opaque_replacement_edits`
   - trace-only `_area` locals
   - duplicated wrapper functions that only unwrap `DiffSurfaceEdit`
   - remaining local context-key helpers after migration
6. Keep the actual raw-line, word-token, equation-token, non-token display, and
   opaque visual behavior initially unchanged. This phase consolidates the
   decision boundary before changing semantics.

Tests to add or update:

- Same visible text with different presentation still selects the intended
  surface.
- Raw block changes still select raw-line surface.
- Equation-origin changes still select equation-token surface.
- Semantic page-region text changes use the same selection path as body text.
- Rendered-region segment changes record rendered-region surface kinds.

Exit criteria:

- Replacement selection has one area+surface return path.
- No production hits for `word_or_opaque_replacement_edits`.
- Context-key logic lives in `content_key`.
- `FB-010` is either retired or narrowed to explicit unsupported-surface cases.
- Passing-corpus gate passes.

Estimated net production LOC: -150 to -300.

## Phase 5: Delete Rendered-Region Source Parsing

Clean abstractions to promote: `context_recording`, structural content-tree
inspection, `DiffAreaKind::RenderedPageRegion`, and rendered-region surfaces.

Problem today: rendered page-region wrapper recovery first uses structural
`AlignElem` when available, but still parses source text for `align(...)` when
context output is opaque.

Target design:

- Wrapper recovery comes from retained content/provenance:
  - `AlignElem` in the source or recorded context output,
  - otherwise rendered layout alignment as a layout fallback.
- There is no source-span string parsing for wrapper identity.
- Generated Typst snippets for page-number-dependent context output may remain
  for now, because direct `ContextElem` construction requires Typst closure
  internals and is not a low-risk deletion step.

Implementation steps:

1. Try structural wrapper extraction from:
   - `page_region_content(new_source_styles, kind)`,
   - recorded context output for the region span when available,
   - existing retained semantic region content if present.
2. Keep `rendered_region_layout_wrapper` as a layout fallback when no structural
   wrapper exists.
3. Delete:
   - `rendered_region_source_wrapper`
   - `authored_align_wrapper`
   - `parse_align_call_alignment`
4. Remove `FB-013` warning code only if no source-string parser remains. If
   layout alignment is still considered a fallback, record it separately and
   accurately.

Tests to add or update:

- Header/footer with `align(right)[...]` preserves alignment via `AlignElem`.
- Contextual header/footer with recorded output preserves wrapper if recording
  exposes it.
- Source text containing the word `align` inside ordinary text does not affect
  wrapper detection.
- Rendered layout alignment still works when no structural wrapper exists.

Exit criteria:

- No production hits for the deleted source parser symbols.
- No source text is inspected to infer rendered-region wrapper identity.
- Passing-corpus gate passes.

Estimated net production LOC: -40 to -90.

## Phase 6: Tighten Equation And Footnote Provenance

Clean abstraction to promote: provenance carried by annotation and indexed
attributed streams.

Problem today: equation provenance was improved, but duplicate block-level
carrier recovery remains in `diff.rs`. Footnote marker matching still uses the
visible marker number when no stronger marker provenance exists.

Target design:

- Equation origins are assigned once during annotation and consumed through the
  attributed stream.
- Diff construction does not re-scan annotated trees for equation-origin block
  claims.
- Footnote marker matching remains only if no Typst provenance is available,
  and it is narrow, ledgered, and tested.

Implementation steps:

1. After Phase 1 indexed streams, verify every equation-origin consumer reads
   from stream items or annotation directly.
2. Delete any remaining duplicate equation-origin helpers in `diff.rs`.
3. Deduplicate equation carrier predicates between `annotated.rs` and `diff.rs`
   if both remain.
4. Audit `annotate_footnote_markers` for possible retained marker provenance.
   If no cleaner source exists, keep it but make the fallback boundary explicit
   and ensure the ledger describes it as the last footnote marker debt.
5. Delete or narrow:
   - duplicate `realized_equation_carrier_count_for_diff`
   - duplicate `collect_annotated_equation_origins`
   - any block-level equation claim structs left after Phase 1

Tests to add or update:

- Empty block adjacent to display equation does not take equation provenance.
- Multiple equations in one paragraph keep the correct token order.
- A visible `1` before a footnote marker does not take the footnote body if a
  stronger marker signal is available. If no stronger signal exists, keep this
  as a documented failing/unsupported probe rather than guessing more broadly.

Exit criteria:

- Equation origins have one assignment/consumption path.
- Remaining footnote marker debt is explicit and tested.
- Passing-corpus gate passes.

Estimated net production LOC: -120 to -220.

## Phase 7: Prune Debug, Ledger, And Docs

Purpose: remove documentation and diagnostics that only exist for deleted
legacy paths.

Steps:

1. Update `docs/fallback-debt-ledger.md`:
   - retire entries whose production code is gone,
   - keep active entries only for active warning codes,
   - ensure each active entry has current source sites and removal criteria.
2. Update `src/decision.rs` to remove retired warning codes.
3. Update debug summaries in `src/debug.rs` so they no longer mention deleted
   fallback paths or duplicate-pruning concepts.
4. Update `docs/technical.md` and `docs/system-walkthrough-annotated-tree.md`
   to describe:
   - indexed attributed block streams,
   - container-owned slot paths,
   - unified area+surface selection,
   - the remaining unsupported boundaries.
5. Add a final entry to `TECHNICAL-DECISIONS.md` with:
   - final production LOC,
   - deleted legacy paths,
   - remaining intentional debt,
   - tradeoffs.
6. Re-run deletion searches:

   ```bash
   rg "BlockOwnerCursor|EquationOriginBlockCursor|find_annotated_block_owner" src docs tests
   rg "map_slot_parts|map_unique_partial_item_container|opaque_pre_surface" src docs tests
   rg "rendered_region_source_wrapper|authored_align_wrapper|parse_align_call_alignment" src docs tests
   rg "word_or_opaque_replacement_edits|patchable_surface_for_index" src docs tests
   ```

Exit criteria:

- Fallback ledger audit passes.
- Docs do not advertise deleted bridges as current architecture.
- Final production LOC is recorded.

Estimated net production LOC: -150 to -250.

## Phase 8: Extract Annotated Blocks With Retained Provenance

Clean abstraction to promote: annotated block extraction as the producer of
both `DiffBlock` payloads and `AttributedBlockStream` claims.

Problem today: Phase 1 made production block ops indexed and removed the owner
and equation cursor objects, but stream construction still recovers ownership
after block extraction by matching realized content. The remaining bridge is:

- `collect_block_owner_claims`
- `collect_equation_origin_block_claims`
- `find_annotated_block_owner`
- `find_single_block_semantic_owner`
- `collect_single_block_semantic_owners`
- `owned_block_matches`
- `BlockOwnerClaim`
- `EquationOriginBlockClaim`

Important technical note:

- A local deletion attempt that walked the annotated tree directly and emitted
  attribution claims separately from block extraction broke owner placement for
  tables, grids, figures, footnotes, display equations, and opaque wrappers.
- The failures were not random. They showed that stream cardinality and owner
  placement must be determined by the same logic that creates block boundaries.
- Padding missing stream items with ownerless defaults restores cardinality but
  does not preserve semantic ownership. It can shift table/figure/footnote owner
  claims onto the wrong realized block or duplicate opaque-wrapper edits.
- Therefore the correct replacement is not another post-hoc attribution walk.
  It is an `extract_annotated_block_units`-style primitive that emits each
  realized block together with its retained owner/equation/footnote provenance
  in one pass.
- Tests alone are not a reason to keep the current helper functions. They remain
  only because they are active production helpers. Once attributed block
  extraction replaces them, delete those helpers and any tests that only protect
  their private legacy behavior.

Target design:

- Introduce an internal attributed block extraction API, for example:

  ```rust
  struct AttributedDiffBlock<'a> {
      block: DiffBlock,
      claim: AttributedBlockClaim<'a, SemanticOwnerKey>,
  }
  ```

- The attributed extractor must preserve the exact block sequence currently
  produced by `extract_block_units` / `non_parbreak_blocks`.
- Owner, equation-origin, footnote-body, patch-surface, and semantic-key
  metadata must be attached at the same moment the block boundary is emitted.
- Empty carrier blocks must be represented only when they are real extracted
  blocks, not invented or skipped by a later attribution pass.
- Table/grid effective-render blocks, figure body/caption carriers, footnote
  marker/body carriers, display-equation shells, opaque wrapper surfaces, and
  page-style propagation must all keep their current tested behavior.
- `AttributedBlockStream` should be constructed from attributed block claims
  without any realized-content owner lookup.

Implementation steps:

1. Add focused tests around attributed block extraction cardinality and owner
   placement for:
   - plain paragraphs with inline styling,
   - table/grid cells,
   - figures with body and caption edits,
   - footnotes near unchanged visible text,
   - display equations adjacent to empty blocks,
   - opaque visual wrappers.
2. Build `extract_annotated_block_units` alongside `extract_block_units` and
   assert in tests that the emitted block payload sequence is identical to
   `extract_block_units(&root.realized)` after non-parbreak filtering.
3. Move semantic owner and equation origin attachment into the extractor at the
   point where each block is emitted.
4. Change `prepare_diff_inputs` / stream construction to use attributed block
   vectors as the source of both block matching and `AttributedBlockStream`.
5. Delete the old realized-content recovery bridge:
   - `collect_block_owner_claims`
   - `collect_equation_origin_block_claims`
   - `find_annotated_block_owner`
   - `find_single_block_semantic_owner`
   - `collect_single_block_semantic_owners`
   - `owned_block_matches`
   - `BlockOwnerClaim`
   - `EquationOriginBlockClaim`
6. Delete tests that only cover those private recovery helpers, after replacing
   them with attributed-extraction behavior tests.
7. Update `TECHNICAL-DECISIONS.md` and any docs that still describe block
   attribution as recovered by content matching.

Exit criteria:

- No production hits for the deleted owner/equation claim recovery symbols.
- `AttributedBlockStream` is built from retained attributed extraction output,
  not by re-scanning the annotated tree for matching realized content.
- Existing table, figure, footnote, equation, opaque-wrapper, and repeated-block
  integration tests pass.
- Passing corpus passes.

Estimated net production LOC: -250 to -500.

## Final Acceptance Gate

Run:

```bash
cargo check --all-targets
cargo test --all-targets
bash tests/check_fallback_ledger.sh
bash tests/run_passing_corpus.sh
bash tests/run_corpus.sh --only-failures
```

Final acceptance criteria:

- Passing corpus has no regressions.
- Full test suite passes, or any pre-existing failures are documented.
- No deleted legacy symbols remain in production code.
- Active fallback ledger entries exactly match active warning codes.
- Production LOC is at least 1,100 lines below the Phase 0 baseline, unless a
  phase explicitly documents why preserving behavior required less deletion.
- Every surviving bridge has a named owner, a test, and a removal criterion.
