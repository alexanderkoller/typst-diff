# Lean Deletion Implementation Plan

This plan follows the deep-cut refactor. The premise is that the new
abstractions are generally cleaner than the old code, but the codebase is still
long because compatibility bridges keep older decision paths alive.

The implementation goal is not to add another abstraction layer. The goal is to
make the clean abstractions authoritative and delete the old code they replace.

The phase order is dependency-first. We are deliberately not optimizing for
early quick wins; each phase should disentangle one dependency layer so later
deletions are safe, mechanical, and smaller in scope.

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
- At the end of every phase, report honestly on improvements that were not
  made and why, and point out opportunities for further improvements or cleaner
  generalizations. Record those deferred or rejected improvements in this plan
  and, when they affect architecture, in `TECHNICAL-DECISIONS.md`.

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
the later owner-scan bridge alive.

Progress:

- Done: production block ops are indexed, and public/debug `BlockOp` is now an
  adapter over the indexed matcher.
- Done: block-op consumption reads `AttributedBlockStream` by index instead of
  searching for a matching realized block.
- Done: `BlockOwnerCursor`, `EquationOriginBlockCursor`, content-search
  attributed block lookup, duplicate realized stream payloads, and the duplicate
  content-rich edit-zone matcher have been removed.
- Deferred beyond this phase: the later owner-scan bridge still recovered some
  attribution from realized content inside attributed block construction. Phase
  5 deletes that bridge after retaining table/grid/raw ownership during
  extraction.

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
   construction until Phase 3 builds attributed block extraction and Phase 4
   switches production stream construction to it. The old cursor objects should
   be gone, but the remaining claim helpers are deferred debt rather than local
   Phase 1 deletions.
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
  Phase 4 debt.
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

Progress:

- Done: the global `map_slot_parts` tail is deleted. `map_container` no longer
  invents a mapping after a container mapper declines.
- Done: list, enum, terms, table, grid, stack, quote, figure, and wrapper
  mapping decisions live in the corresponding `ContainerOps::map_slots`
  implementations.
- Done: the old helper names `map_unique_partial_item_container`,
  `patch_surface_for_opaque_realization`, `graft_opaque_patch_surface`,
  `opaque_pre_surface`, `has_nested_list_container`,
  `unique_realized_wrapper_path`, and `collect_realized_wrapper_paths` are gone.
- Done: quote owners now claim their text-empty realized carrier directly during
  block-owner stream construction, so quote body changes remain slot edits.
- Narrowed but not retired: `FB-007`, `FB-008`, and `FB-009` remain active under
  container-owned helper names. Tests showed these are still required when block
  extraction presents a single realized item, a text-empty wrapper carrier, or
  fewer realized leaf paths than source slots. Removing them cleanly requires
  the retained block/slot/wrapper provenance deferred to Phases 3 and 4.
- Actual LOC result for this phase is not a reduction: the old global code was
  deleted, but the behavior-preserving container-owned routes and ledger updates
  are net-positive. The later reduction now depends more heavily on Phase 4
  replacing the narrowed FB-007/008/009 bridges with retained provenance.

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

## Phase 3: Build Attributed Block Extraction Foundation

Clean abstraction to promote: annotated block extraction as the producer of
block payloads and attribution claims.

Purpose: disentangle the dependency that blocked Phase 2 without doing a risky
production switch. This phase builds and tests the new extractor beside the old
stream-construction bridge.

Progress:

- Done: added a test-only `extract_annotated_block_units` foundation that emits
  attributed block units beside production stream construction.
- Done: factored stream claim construction into a shared
  `attributed_block_claims` helper so the foundation and current stream builder
  can be compared directly.
- Done: parity tests prove exact non-parbreak block payload preservation for
  inline-styled paragraphs, single-item list owners, table-cell owners,
  footnote-owned carriers, display equation origins, and quote empty carriers.
- Deferred to Phase 4: production stream construction still uses the
  realized-content owner/equation recovery bridge. The bridge remains active
  until attributed extraction becomes authoritative.

Problem today: Phase 1 made production block ops indexed and removed the owner
and equation cursor objects, but stream construction still recovers ownership
after block extraction by matching realized content. Phase 2 showed that this
same missing provenance keeps FB-007, FB-008, and FB-009 alive.

The pre-Phase-4 bridge was:

- `collect_block_owner_claims`
- `collect_equation_origin_block_claims`
- `find_annotated_block_owner`
- `find_single_block_semantic_owner`
- `collect_single_block_semantic_owners`
- `owned_block_matches`
- `BlockOwnerClaim`
- `EquationOriginBlockClaim`

After the Phase 4 slice, only `find_annotated_block_owner`,
`find_single_block_semantic_owner`, and `collect_single_block_semantic_owners`
remain active from this list.

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
  marker/body carriers, display-equation shells, opaque wrapper surfaces, quote
  carriers, and page-style propagation must all keep their current tested
  behavior.

Implementation steps:

1. Add focused tests around attributed block extraction cardinality and owner
   placement for:
   - plain paragraphs with inline styling,
   - list/enum/terms single-item carriers,
   - table/grid cells,
   - figures with body and caption edits,
   - footnotes near unchanged visible text,
   - display equations adjacent to empty blocks,
   - quotes with text-empty realized carriers,
   - opaque visual wrappers.
2. Build `extract_annotated_block_units` alongside `extract_block_units`.
3. Assert in tests that the emitted block payload sequence is identical to
   `extract_block_units(&root.realized)` after non-parbreak filtering.
4. Attach owner/equation/footnote provenance in the new extractor, but keep
   production stream construction unchanged in this phase.
5. Add debug-only or test-only comparison utilities that report mismatches
   between old claim recovery and new attributed extraction.

Exit criteria:

- Attributed extraction exists behind tests without changing production output.
- Parity tests prove exact block payload sequence equality for representative
  semantic owners and corpus-shaped fixtures.
- Owner/equation/footnote attribution mismatches are visible in focused tests,
  not hidden behind broad integration failures.
- Passing-corpus gate passes.

Estimated net production LOC: +50 to +150.

## Phase 4: Switch Streams To Attributed Extraction

Purpose: make retained attributed extraction authoritative, then delete the old
realized-content recovery bridge.

Progress:

- Done: `prepare_diff_inputs` now builds old/new attributed block vectors and
  uses their `DiffBlock` payloads for indexed block matching.
- Done: `AttributedBlockStream` is now constructed from retained claims on
  attributed block units, not by rebuilding stream claims in the main diff loop.
- Done: removed the old `BlockOwnerClaim` and `EquationOriginBlockClaim`
  structs, their collector names, and `owned_block_matches`; temporary
  owner/equation outputs now use `AttributedDiffBlock` directly.
- Done: deleted direct tests for the removed private `find_annotated_child`
  helper rather than keeping obsolete helper tests alive.
- Resolved by Phase 5: `find_annotated_block_owner`,
  `find_single_block_semantic_owner`, and
  `collect_single_block_semantic_owners` remained active after this phase
  because a deletion probe caused
  regressions in deleted table/grid structure, raw block line diffs, repeated
  same-text table cells, reused named tables, generated wrapper tables, and raw
  table cells. Phase 5 deletes them after retaining the missing table/grid/raw
  owner provenance directly.

Implementation steps:

1. Change `prepare_diff_inputs` / stream construction to use attributed block
   vectors as the source of both block matching and `AttributedBlockStream`.
2. Keep a short transition assertion in tests that attributed block payloads
   still match the legacy extraction payloads.
3. Delete the old realized-content recovery bridge where behavior proves it is
   obsolete:
   - Done: `collect_block_owner_claims`
   - Done: `collect_equation_origin_block_claims`
   - Done in Phase 5: `find_annotated_block_owner`
   - Done in Phase 5: `find_single_block_semantic_owner`
   - Done in Phase 5: `collect_single_block_semantic_owners`
   - Done: `owned_block_matches`
   - Done: `BlockOwnerClaim`
   - Done: `EquationOriginBlockClaim`
4. Delete tests that only cover those private recovery helpers, after replacing
   them with attributed-extraction behavior tests.
5. Retire or narrow FB-007, FB-008, and FB-009. These codes should disappear if
   retained block/slot/wrapper provenance fully replaces their container-owned
   bridges; otherwise each survivor must point to a precise unsupported
   boundary.
6. Update `TECHNICAL-DECISIONS.md` and any docs that still describe block
   attribution as recovered by content matching.

Exit criteria:

- No production hits for the deleted owner/equation claim recovery symbols.
- `AttributedBlockStream` is built from retained attributed extraction output,
  not by re-scanning the annotated tree for matching realized content.
- Existing table, figure, footnote, equation, quote, opaque-wrapper, and
  repeated-block integration tests pass.
- Passing corpus passes.

Estimated net production LOC: -300 to -650.

## Phase 5: Finish Owner-Retained Attributed Extraction

Clean abstraction to promote: attributed block extraction as the direct owner
of table/grid/raw/container block provenance.

Purpose: delete the remaining owner scan without weakening table/grid/raw
behavior. Phase 4 made attributed block units authoritative for block matching
and stream construction, but still needed a post-hoc owner scan when extraction
lost table/grid/raw provenance.

Progress:

- Done: table/grid owners may make variable-number direct block claims over the
  realized carrier blocks they emit, so deleted table/grid structures and
  repeated same-text table cells keep their semantic owner without a tree-wide
  owner scan.
- Done: raw blocks are direct block owners, so raw line diffs keep authored raw
  provenance through attributed extraction.
- Done: generated table/grid wrappers use a narrow retained effective-owner side
  channel: table/grid effective blocks are consumed by exact non-empty rendered
  text, in extraction order, only when the realized-carrier claim is ownerless.
- Done: variable-number direct block claims are limited to table/grid. Figures,
  equations, opaque wrappers, and repeated macro containers keep fixed
  single-carrier direct claims.
- Done: deleted `find_annotated_block_owner`,
  `find_single_block_semantic_owner`, and
  `collect_single_block_semantic_owners`.

Pre-Phase-5 bridge:

- `find_annotated_block_owner`
- `find_single_block_semantic_owner`
- `collect_single_block_semantic_owners`

A Phase 4 deletion probe showed that these helpers are still behaviorally
required for:

- deleted table and grid structure,
- raw block line diffs,
- repeated same-text table cells,
- reused named tables,
- generated wrapper tables,
- raw table cells,
- inserted and deleted table rows.

Target design:

- Attributed extraction emits the semantic owner at the same time it emits the
  block payload.
- Table/grid realized carriers keep their table/grid owner directly, even when
  their visible text is empty and their effective render content is used later
  for slot recursion.
- Raw block owners are retained directly through extraction, so raw line diffs
  do not depend on a later exact-content search.
- Repeated same-text owners are disambiguated by extraction order and semantic
  owner keys, not by normalized visible-text recovery.

Implementation steps:

1. Add focused regression coverage for the failed deletion probe if any case is
   not already directly covered by integration tests.
2. Teach owner claim extraction to claim realized table/grid/raw/container
   carriers directly when those carriers are the blocks used by indexed block
   matching.
3. Keep effective-render table/grid content available for slot recursion, but
   do not use it as a replacement for the stream block owner claim when it
   would misalign with the production block payload.
4. Delete:
   - `find_annotated_block_owner`
   - `find_single_block_semantic_owner`
   - `collect_single_block_semantic_owners`
5. Delete tests that only protect the private owner scan. Keep or add behavior
   tests that prove table/grid/raw ownership survives without it.
6. Retire or narrow any FB-007, FB-008, or FB-009 debt that becomes obsolete.
7. Update `TECHNICAL-DECISIONS.md` with the retained-owner extraction decision
   and the deletion probe result.

Exit criteria:

- No production hits for `find_annotated_block_owner`,
  `find_single_block_semantic_owner`, or
  `collect_single_block_semantic_owners`.
- Table/grid/raw probe regressions from Phase 4 still pass.
- `AttributedBlockStream` owner claims are sourced from attributed extraction,
  not from a post-hoc annotated tree scan.
- Passing-corpus gate passes.

Estimated net production LOC: -80 to -180.

## Phase 6: Tighten Equation And Footnote Provenance

Clean abstraction to promote: provenance carried by annotation and attributed
block extraction.

Problem today: equation provenance was improved, but duplicate block-level
carrier recovery remains until Phase 5 finishes retained owner extraction.
Footnote marker matching still uses the visible marker number when no stronger
marker provenance exists.

Progress:

- Done: equation-origin stream construction now uses retained annotated
  equation provenance carried on `AttributedDiffBlock`; the old dedicated
  block-level equation claim structs remain deleted.
- Done: removed the duplicate `diff.rs` equation carrier counter and predicate.
  Diff-side code now imports the centralized `annotated.rs`
  `realized_equation_carrier_count` / `is_realized_equation_carrier` helpers.
- Done: removed the duplicate `collect_annotated_equation_origins` traversal
  name. The remaining retained-origin traversal is local to attributed
  extraction and produces `AttributedDiffBlock` claims.
- Done: audited `annotate_footnote_markers`. No stronger Typst marker
  provenance is currently available at that boundary, so visible-number marker
  matching remains as the explicit `FB-012` debt. The visible `1` regression and
  nearby-footnote tests keep the fallback narrow.
- Done: the equation and footnote focused tests, full Rust suite, fallback
  ledger check, and passing corpus gate pass.

Target design:

- Equation origins are assigned once during annotation/extraction and consumed
  through attributed stream items.
- Diff construction does not re-scan annotated trees for equation-origin block
  claims.
- Footnote marker matching remains only if no Typst provenance is available,
  and it is narrow, ledgered, and tested.

Implementation steps:

1. Done: verify every equation-origin consumer reads from attributed extraction
   output or annotation directly.
2. Done: delete duplicate equation-origin helpers left in `diff.rs`.
3. Done: deduplicate equation carrier predicates between `annotated.rs` and
   `diff.rs`.
4. Done: audit `annotate_footnote_markers` for possible retained marker
   provenance.
   If no cleaner source exists, keep it but make the fallback boundary explicit
   and ensure the ledger describes it as the last footnote marker debt.
5. Done: delete or narrow:
   - Done: duplicate `realized_equation_carrier_count_for_diff`
   - Done: duplicate `collect_annotated_equation_origins`
   - Done: any block-level equation claim structs left after Phase 5

Tests to add or update:

- Covered: empty block adjacent to display equation does not take equation
  provenance.
- Covered: multiple equations in one paragraph keep the correct token order.
- Covered: a visible `1` before a footnote marker does not take the footnote
  body if a stronger marker signal is available. If no stronger signal exists,
  keep this as a documented failing/unsupported probe rather than guessing more
  broadly.

Exit criteria:

- Equation origins have one assignment/consumption path.
- Remaining footnote marker debt is explicit and tested.
- Passing-corpus gate passes.

Actual net production LOC: roughly neutral. The phase deleted duplicate
equation-carrier logic, but retained a small attributed-block alignment path so
inline equations split into adjacent empty carriers still feed the paragraph
word diff.

Not done:

- Did not make attributed extraction rich enough to merge inline equation
  origins directly into the paragraph block that later gets word-diffed. That
  would require changing the block extraction/provenance boundary so text blocks
  can carry embedded child-origin metadata, not just one owner/origin claim for
  the block as a whole.
- Did not remove retained equation alignment. Removing it now regresses inline
  equation tokenization because Typst can realize inline math as adjacent empty
  equation-carrier blocks while the changed text paragraph is matched as a
  separate block.

Further improvement opportunity:

- A later, dedicated attributed-block richness phase could make block units
  carry embedded token-level provenance for inline semantic children. If that
  exists, paragraph word diffs would receive equation origins directly from the
  block unit and the retained equation alignment path should be deleted.
- This is worth doing only if it also simplifies other embedded-inline
  provenance cases. It is not worth doing as a narrow equation-only refactor,
  because the current retained alignment is small, tested, and uses centralized
  carrier detection rather than a broad post-hoc guess.

## Phase 7: Promote content_tree For Render Path Editing

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

Progress:

- Done: `content_tree::insert_realized_content_at_path` now owns recursive
  insertion beside the existing realized-content replacement helper.
- Done: `annotate.rs` delegates replace and insert render-path edits to
  `content_tree` instead of carrying a local recursive path editor.
- Done: the local `PathEdit`, local `apply_path_edit`,
  `patchable_surface_for_index`, and the child-sequence fallback that synthesized
  a surface from annotated children were deleted.
- Done: failed replace/insert path application is observable in debug traces as
  `annotate/path-edit` with operation and path details.
- Done: shared path mechanics in `container_ops` now understand item-level
  realized children: list and enum items are transparent body wrappers, and term
  items expose term and description children. This fixed nested-list path editing
  without reintroducing the renderer-side synthetic surface fallback.
- Done: list and enum container replacement now normalizes patched `ListItem`
  and `EnumItem` values back to item bodies at the container boundary. This keeps
  recursive item-body path edits from nesting a list item marker inside another
  list item body.
- Verified: focused path and nested-list tests pass, `cargo check
  --all-targets` passes, `cargo test --all-targets` passes, and
  `tests/check_fallback_ledger.sh` passes.
- Verified: `tests/run_passing_corpus.sh` reports 99 passed and 0 failed. No
  corpus references were updated.

Implementation steps:

1. Done: add `content_tree::insert_realized_content_at_path`.
2. Done: move the generic path edit logic formerly in `annotate.rs` into
   `content_tree`, or replace it with calls to:
   - `realized_content_at_path`
   - `replace_realized_content_at_path`
   - `insert_realized_content_at_path`
3. Done: keep `patch_path_for_logical_path`, but make it the only patch-path
   translation.
4. Done: delete:
   - local `PathEdit`
   - local `apply_path_edit`
   - `patchable_surface_for_index`
   - the child-sequence fallback that creates
     `Content::sequence(node.children.iter().map(effective_render_content))`
5. Done: add a trace event when a render edit path fails to resolve.
   Do not silently invent a surface.

Tests to add or update:

- Covered: a valid `ReplaceAt` with a `patch_path` applies to the patch surface.
- Covered: a missing patch path does not create a synthetic child-sequence
  surface.
- Covered: insert-before and insert-after still work for direct list paths
  through `content_tree`.
- Covered indirectly: nested list item body replacement uses shared
  list/enum-item transparent body path mechanics instead of a local annotation
  fallback.
- Covered: nested list item edits preserve the normal rendered text positions
  for the phylum and class items, catching accidental nested marker insertion.
- Covered by behavior: missing path application leaves the patch surface
  unchanged instead of synthesizing a child sequence; the implementation also
  emits debug-trace diagnostics for unresolved paths.

Exit criteria:

- Met: no production hits for `patchable_surface_for_index` or local
  `apply_path_edit`.
- Met: path editing code lives in `content_tree` or delegates directly to it,
  with container-specific child replacement handled by `container_ops`.
- Met: passing-corpus gate passes.

Actual/estimated net production LOC: roughly neutral to slightly positive in
this phase. The local annotation fallback was removed, but the shared
`content_tree` insertion helper, observable path-failure trace, and explicit
item-level child semantics add a little code. The payoff is that later path
editing now has one reusable route instead of a private annotation copy.

Improvements not made:

- The public render-annotation API still returns rendered content rather than a
  structured diagnostics object. Failed path edits are trace-visible when debug
  tracing is enabled, but callers without a debug sink still observe a skipped
  patch rather than a typed failure. This was deferred to avoid changing the
  public annotation contract inside a deletion phase.
- List and enum item body transparency is centralized in `container_ops`, not
  yet recorded as explicit annotated path metadata. That is clean enough to
  remove the renderer-side fallback now, but a future richer-provenance phase
  could make the path convention visible earlier in annotation.

Further improvement opportunity:

- A later diagnostics cleanup could make path-edit failure a typed result that
  flows out of `build_annotated_content_from_tree` instead of relying on debug
  traces.
- A later provenance cleanup could encode list/enum transparent body path
  semantics in annotated path metadata. If that generalizes across wrappers, it
  may let `container_ops` become a pure structural editor while annotation owns
  the logical-to-rendered path contract.

## Phase 8: Promote content_key And Diff Surface Selection

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

Progress:

- Done: `diff_surface` now defines `DiffSelection<T>`, which carries
  `DiffAreaKind`, `DiffSurfaceKind`, and the selected edit payload together.
- Done: `select_modified_fragment_surface` was replaced by
  `select_modified_fragment(area, ...)`, so body blocks, equal-block
  presentation changes, semantic page-region replacements, slot/container
  replacements, and rendered page-region word/segment diffs all use the same
  area+surface vocabulary.
- Done: `word_or_opaque_replacement_edits` was deleted. The two callers now
  consume `DiffSelection<EditContent>` directly.
- Done: local block-context key helpers moved into `content_key`:
  `block_context_key_for`, `semantic_heading_context`, `block_context_key`, and
  `is_block_context`. The private annotated-context helper lives there too.
- Done: trace-only `_area` locals in semantic and rendered page-region code were
  removed; area travels with the selection object instead.
- Preserved: raw-line, word-token, equation-token, non-token display, and opaque
  visual behavior was kept unchanged.
- Verified: focused surface/key tests pass, `cargo check --all-targets` passes,
  `cargo test --all-targets` passes, `tests/check_fallback_ledger.sh` passes,
  and `tests/run_passing_corpus.sh` reports 99 passed and 0 failed.

Implementation steps:

1. Done: add `DiffSelection<T>` wrapping `DiffSurfaceEdit<T>` with
   `DiffAreaKind`.
2. Done: replace `select_modified_fragment_surface` with a function that
   returns the area+surface selection.
3. Done: route these callers through the same selection function:
   - block replacement fallback,
   - presentation-changed equal blocks,
   - `modified_fragment_edit_content`,
   - semantic page-region replacement,
   - rendered-region word/segment selection.
4. Done: move local context-key helpers into `content_key`:
   - `block_context_key_for`
   - `annotated_block_context_key`
   - `semantic_heading_context` if it becomes key comparison
   - `block_context_key`
   - `is_block_context` if only used for context classification
5. Done: delete:
   - `word_or_opaque_replacement_edits`
   - trace-only `_area` locals
   - duplicated wrapper functions that only unwrap `DiffSurfaceEdit`
   - remaining local context-key helpers after migration
6. Done: keep the actual raw-line, word-token, equation-token, non-token display, and
   opaque visual behavior initially unchanged. This phase consolidates the
   decision boundary before changing semantics.

Tests to add or update:

- Covered: same visible text with different presentation still selects the intended
  surface.
- Covered: raw block changes still select raw-line surface.
- Covered: equation-origin changes still select equation-token surface.
- Covered: semantic page-region text changes use the same selection path as body
  text.
- Covered: rendered-region segment changes record rendered-region surface kinds.

Exit criteria:

- Met: replacement selection has one area+surface return path.
- Met: no production hits for `word_or_opaque_replacement_edits`.
- Met: context-key logic lives in `content_key`.
- Partially met: `FB-010` is narrowed but not retired. The selected surface and
  area are now explicit, but the final body-block replacement ladder still emits
  the legacy warning when structural routes fail.
- Met: passing-corpus gate passes.

Actual/estimated net production LOC: about +50 in touched production files.
`diff.rs` shrank, but the new shared selection type and context-key home add
code in `diff_surface.rs` and `content_key.rs`. The value of this phase is
deleting a decision wrapper and consolidating the decision boundary; larger LOC
reduction depends on a later phase retiring `FB-010` and its final replacement
ladder.

Improvements not made:

- `FB-010` was not retired. Unsupported or low-confidence body-block
  replacements still use the final word/opaque replacement ladder after
  structural routes fail. Retiring it requires an explicit unsupported-surface
  diagnostic and a policy for when not to produce an edit.
- `DiffSurfaceEdit<T>` remains as the payload object inside `DiffSelection<T>`.
  That keeps the patch small and avoids churn in tests and callers that only
  care about surface payloads, but it means the old type name is still present
  as an implementation detail.
- Selection tracing for slot/container replacements is not yet as rich as
  body-block tracing. The area is available on the selection object, but the
  slot trace still reports the higher-level script decision rather than every
  selected surface.

Further improvement opportunity:

- A future cleanup can split the final replacement ladder into explicit
  supported surfaces and unsupported surfaces. That is the likely point where
  `FB-010` can be retired instead of merely narrowed.
- Once every selection consumer needs area+surface together, `DiffSurfaceEdit`
  can probably be folded into `DiffSelection` to remove one more wrapper type.

## Phase 9: Delete Rendered-Region Source Parsing

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

## Phase 10: Prune Debug, Ledger, And Docs

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
   - attributed block extraction,
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
   rg "collect_block_owner_claims|collect_equation_origin_block_claims|BlockOwnerClaim|EquationOriginBlockClaim" src docs tests
   rg "map_slot_parts|map_unique_partial_item_container|opaque_pre_surface" src docs tests
   rg "rendered_region_source_wrapper|authored_align_wrapper|parse_align_call_alignment" src docs tests
   rg "word_or_opaque_replacement_edits|patchable_surface_for_index" src docs tests
   ```

Exit criteria:

- Fallback ledger audit passes.
- Docs do not advertise deleted bridges as current architecture.
- Final production LOC is recorded.

Estimated net production LOC: -150 to -250.

## Phase 11: Retire FB-010 With Unsupported-Surface Diagnostics

Clean abstractions to promote: `DiffSelection`, `DiffSurfaceKind`,
`DiffAreaKind`, and explicit unsupported-surface diagnostics.

Problem today: Phase 8 made final replacement selection area/surface typed, but
the body-block fallback still uses the legacy word-or-opaque replacement ladder
after structural routes fail. That means `FB-010` is narrowed but still active:
the selected surface is explicit, but the policy for unsupported or
low-confidence replacement surfaces is not.

This phase is intentionally deferred until the end because it is a semantic
policy change, not a mechanical follow-up to Phase 8. It can change user-visible
diffs: some cases that currently get a broad word edit or opaque visual frame
may need to become explicit unsupported/no-op diagnostics instead. It is not a
dependency for rendered-region source parsing deletion in Phase 9.

Technical notes from Phase 8:

- `DiffSelection<T>` now gives every replacement-style decision a typed area and
  surface. That removes the old excuse for a monolithic "word or opaque"
  wrapper: callers can inspect the selected `DiffAreaKind` and
  `DiffSurfaceKind` directly.
- `FB-010` still fires only in the final body-block replacement path after
  structural routes have failed. It is therefore narrower than before, but still
  represents a real fallback.
- The ladder still preserves important behavior for low-similarity container
  replacements, opaque visuals, raw-line changes, equation-token changes, and
  same-visible-text presentation changes. Do not delete it by returning no edits
  globally.
- Retiring `FB-010` requires a policy split:
  - supported explicit surfaces keep producing edits,
  - unsupported structured surfaces produce diagnostics or a deliberately
    documented no-op boundary,
  - opaque visual surfaces remain edits only when the old/new surfaces are
    proven visual carriers or annotated owners known to be visual wrappers.
- `DiffSurfaceEdit<T>` currently remains as an implementation detail inside
  `DiffSelection<T>`. Once this phase makes all consumers area-aware, it may be
  possible to fold `DiffSurfaceEdit<T>` into `DiffSelection<T>` and delete the
  extra wrapper.
- Slot/container selection already has an area on the selected value, but its
  trace currently reports the higher-level script decision rather than every
  selected surface. If unsupported-surface diagnostics apply inside slot
  recursion, the trace should expose the exact area/surface there too.

Target design:

- Final replacement selection returns one of:
  - a supported `DiffSelection<EditContent>`,
  - an explicit unsupported-surface diagnostic carrying area, surface/context,
    and old/new previews,
  - no edit only when the no-op is intentional and documented.
- `FB-010` warning code is removed from production code and the fallback ledger.
- Broad word or opaque replacements are not used as a catch-all for structured
  content whose surface is unknown.
- Existing supported behavior remains covered:
  - raw blocks select `RawLines`,
  - equations select `EquationTokens`,
  - ordinary text selects `WordTokens`,
  - display/non-token containers select `NonTokenDisplay` when word tokens would
    be misleading,
  - proven visual carriers select `OpaqueVisual`.

Implementation steps:

1. Audit every current `FB-010`-triggering corpus or integration case and
   classify it as supported surface vs. unsupported surface.
2. Introduce an explicit unsupported replacement result, for example:

   ```rust
   enum ReplacementSelection<T> {
       Supported(DiffSelection<T>),
       Unsupported(UnsupportedSurface),
       NoChange,
   }
   ```

3. Move the final body-block replacement path from
   `Option<DiffSelection<EditContent>>` to that explicit result.
4. Keep supported raw-line, word-token, equation-token, non-token display, and
   proven opaque visual edits behaviorally unchanged.
5. For unsupported structured surfaces, emit a typed diagnostic/debug event
   instead of falling through to a broad word/opaque replacement.
6. Remove `FallbackCode::WordDiffOrOpaqueReplacementLadder`,
   `FB-010-word-diff-or-opaque-replacement-ladder`, and its fallback-ledger
   entry once no production path emits it.
7. If all consumers now use area+surface together, fold `DiffSurfaceEdit<T>`
   into `DiffSelection<T>` and delete the wrapper.

Tests to add or update:

- Low-similarity structured container replacement either remains a supported
  explicit surface or produces an unsupported-surface diagnostic; it must not
  silently become a misleading word edit.
- Opaque graphic replacements still produce `OpaqueVisual` edits.
- Raw block changes still produce raw-line edits.
- Equation-token changes still produce equation-token edits.
- Same-visible-text presentation changes still produce the intended explicit
  surface.
- Unsupported structured surfaces are observable in debug trace or diagnostics.
- `cli_emits_fallback_warning_by_default_and_quiet_suppresses_stderr_only` and
  related debug-trace tests are rewritten to assert the new diagnostic behavior,
  not the retired warning code.

Exit criteria:

- No production hits for `WordDiffOrOpaqueReplacementLadder` or
  `FB-010-word-diff-or-opaque-replacement-ladder`.
- The fallback ledger no longer lists `FB-010`.
- Unsupported replacement surfaces are explicit, tested, and documented.
- Passing-corpus gate passes, with any intentional visual/log changes reviewed
  as behavior changes rather than reference churn.
- `TECHNICAL-DECISIONS.md` records the final supported/unsupported replacement
  policy.

Estimated net production LOC: -80 to -180 if `DiffSurfaceEdit<T>` can also be
folded away; otherwise -40 to -100.

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
