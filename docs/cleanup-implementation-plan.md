# Invariant-Driven Cleanup Plan For typst-diff

## Summary

This is the implementation roadmap for cleaning up `typst-diff` around explicit
invariants. The work should be phased: first document and probe current
behavior, then replace fallback paths with general ownership/provenance
abstractions, testing and reviewing after every step.

Primary goal: cleanup and stronger generalizations. Improved Typst-element
diffing is welcome only when it follows from a stronger general rule.

## Global Rules For Every Step

- Before changing code in a step, write or update tests that expose the
  invariant being strengthened.
- After code changes in a step, run the smallest relevant tests first, then the
  broader integration/corpus checks affected by the change.
- After each step, perform a focused code review pass for new fallbacks,
  duplicated logic, hidden `plain_text()` identity use, and silently ignored
  edit failures.
- After each step, update current documentation if behavior, invariants, or
  debugging workflow changed.
- Do not add new special-case logic unless it catches an exception to a strong
  general rule and is documented as such.
- Prefer explicit ownership/provenance over uniqueness, positional matching,
  source-string parsing, or rendered text recognition.

## Step 1: Establish Executable Invariant Probes

Add tests and small debug helpers for the invariants already documented.

Key changes:

- Add annotation tests proving:
  - `AnnotatedContent.realized` remains byte/content-identical to Typst
    realization.
  - every emitted `SemanticSlot.path` resolves through `children`.
  - slot-bearing nodes recurse only when `semantic_kind` matches.
- Add edit-script tests proving:
  - every `ReplaceAt`, `InsertBefore`, and `InsertAfter` path can be applied to
    the base or patch surface.
  - inserted/deleted/modified edit content appears in rendered annotated
    `Content`.
- Add page-style tests proving page styles remain separate from non-page
  styles.
- Add negative tests for ambiguous same-text structures where visible text must
  not imply identity.

Documentation obligation:

- Add a short "Invariant Test Map" section to the current walkthrough or this
  plan listing each invariant and its test coverage.

Review checkpoint:

- Confirm these tests characterize current behavior without requiring cleanup
  refactors yet.

## Step 2: Introduce Explicit Diff Identity Keys

Separate equality, similarity, and ownership concepts.

Key changes:

- Define internal comparison concepts for:
  - structural equality,
  - visible-text similarity,
  - block ownership,
  - slot identity.
- Keep `plain_text()` available only for similarity and human-readable logs.
- Replace ad hoc `plain_text()` identity uses in block pairing, slot LCS keys,
  layout matching, and partial container matching with named key functions.
- Where a true structural key is not yet available, use an explicit
  `UnresolvedIdentity` or equivalent internal state rather than silently
  treating visible text as identity.

Tests:

- Same visible text with changed link target.
- Same visible text with changed label/reference target.
- Repeated same-text table/list/wrapper items where only one occurrence
  changes.
- Existing corpus cases for link/label/equation changes.

Documentation obligation:

- Document the difference between identity keys and similarity keys in the
  walkthrough.

Review checkpoint:

- Search for new raw `plain_text()` calls in diff ownership/matching code and
  justify or remove each one.

## Step 3: Replace Positional Annotation Fallbacks With Provenance-Aware Pairing

Clean up pre/realized sequence alignment.

Key changes:

- Refactor `pair_sequence_by_span` so semantic annotation is attached only when
  the relationship is proven by:
  - exact pairwise shape,
  - reliable span alignment,
  - container-owned mapping,
  - or another explicit provenance rule.
- When no proof exists, produce anonymous realized leaves and preserve necessary
  pre-side structure only through a named patch-surface variant.
- Remove generic positional fallback for mismatched pre/realized sequences
  unless it is guarded by a documented invariant.

Tests:

- Mismatched sequence lengths with swapped realized text must not attach
  semantic kinds to the wrong realized node.
- Repeated function expansions with shared/detached spans still map in document
  order only when the invariant is explicit and tested.
- Show-rule expansions that add siblings produce anonymous extras unless
  provenance is clear.

Documentation obligation:

- Update annotated-tree construction docs to describe the exact allowed pairing
  rules.

Review checkpoint:

- Verify no semantic kind is assigned merely because a realized child was
  "next."

## Step 4: Make Patch Surfaces A First-Class Abstraction

Replace implicit patch-surface fallbacks with named, documented variants.

Key changes:

- Introduce an internal patch-surface model with explicit cases such as:
  - realized surface,
  - pre-container surface,
  - grafted block-body surface,
  - layout-preserving sequence.
- Move duplicated leading-`ParbreakElem` logic out of container mapping and
  edit application into one invariant-driven helper.
- Ensure every patch surface records why it exists and what invariant it
  preserves.
- Remove nested-list-specific patch behavior where a general layout-boundary
  rule can replace it.

Tests:

- Paragraph/list boundary preservation.
- Nested list insertion/deletion.
- Wrapper body edits.
- Opaque realization where semantic slots outnumber realized children.
- Cases where no patch surface is needed must render from realized content
  unchanged.

Documentation obligation:

- Add a "Patch Surface Contract" section explaining when patch surfaces are
  legal.

Review checkpoint:

- Confirm no patch surface is introduced solely for a corpus-specific visual
  repair.

## Step 5: Move Container Mapping Fully Behind ContainerOps

Make each structured container own its semantic-to-realized mapping.

Key changes:

- Extend `ContainerOps` so each container can map semantic parts to realized
  paths directly.
- Remove or narrow generic `collect_leaf_block_child_paths` use for containers
  where semantics are known.
- Add explicit table/grid cell addressing sufficient for ordinary cells,
  headers, and footers.
- Ensure insert/replace operations for table/grid header/footer cells either
  work structurally or return a typed unsupported edit diagnostic in tests, not
  silent `None`.

Tests:

- List, enum, terms, figure, footnote, quote, wrapper mappings still pass.
- Table/grid row insertion/deletion.
- Table/grid header/footer cell change, insertion, and deletion.
- Repeated identical cell text with one changed cell.

Documentation obligation:

- Update container documentation to describe each supported container's slot
  model and unsupported cases.

Review checkpoint:

- Verify slot paths resolve and point to intended semantic children for every
  supported container.

## Step 6: Replace Unique-Descendant Fallbacks With Block Ownership

Make ownership flow forward instead of being rediscovered after matching.

Key changes:

- During annotation or block extraction, attach enough owner information for
  each block to identify the semantic owner node and path.
- Refactor `diff_annotated` to use this ownership map instead of
  `find_unique_changed_slot_pair` and `find_slot_bearing_descendant_pair`.
- Keep unique-descendant behavior only temporarily behind tests until
  equivalent ownership-based behavior exists, then remove it.
- Remove duplicate edit claims at the ownership source instead of pruning later.

Tests:

- Nested list change inside an outer list item remains localized.
- Nested container inside a table cell remains localized.
- Two changed nested containers in the same outer block both produce edits.
- Repeated macro containers with one edit select the correct owner.

Documentation obligation:

- Document block ownership as part of the layered diff invariant.

Review checkpoint:

- Confirm no diff path depends on "exactly one changed descendant" to find
  locality.

## Step 7: Replace Duplicate Text-Signature Pruning With Structural Edit Ownership

Remove broad post-hoc duplicate suppression.

Key changes:

- Add structural edit owner IDs derived from annotated owner paths or block
  ownership.
- Use owner IDs to prevent the same semantic edit from being emitted twice.
- Delete `prune_duplicate_empty_container_edits` once ownership prevents
  duplicate claims.
- If duplicate suppression is still needed, scope it to identical owner IDs
  only.

Tests:

- Two identical wrapper/list/table edits both appear.
- Empty-text containers with real independent edits are not dropped.
- Existing "opaque wrapper changes are reported once" behavior remains correct
  because ownership prevents double-claiming.

Documentation obligation:

- Document the edit ownership invariant and how duplicate claims are prevented.

Review checkpoint:

- Verify no rendered-text signature is used to decide whether an edit is
  legitimate.

## Step 8: Tighten Equation And Footnote Provenance

Replace broad recognition with provenance where possible.

Key changes:

- Deduplicate equation-carrier detection into one internal helper.
- Replace broad empty-`BlockElem` equation matching with a more specific
  provenance or realized-shape rule.
- Attach equation origins as close as possible to the pre/realized walk rather
  than a broad post-pass when feasible.
- Replace footnote marker matching by bare visible number with marker
  provenance, span/tag evidence, or a stricter marker predicate.

Tests:

- Empty blocks adjacent to equations do not consume equation origins.
- Multiple equations in one paragraph retain correct tokenization.
- Visible `1` before a footnote marker does not receive footnote body metadata.
- Added/changed/deleted footnotes near existing footnotes remain localized.

Documentation obligation:

- Document equation and footnote provenance rules and remaining limitations.

Review checkpoint:

- Confirm no broad "empty block means equation" or "text equals next number
  means footnote" rule remains unless explicitly guarded and documented.

## Step 9: Generalize Rendered Page Regions

Bring contextual headers/footers closer to the main edit model.

Key changes:

- Model rendered page regions as first-class region edits with provenance and
  render contracts.
- Replace source-string `align(...)` scanning with content-tree wrapper analysis
  where possible.
- Avoid generated Typst source for region content if direct `Content`
  construction is practical.
- If generated Typst remains temporarily, return `Result<Content>` instead of
  panicking with `expect`.
- Represent inserted/deleted page-region instances explicitly, including pages
  present only on one side.

Tests:

- Contextual total-page footer still works.
- Alternating headers still work.
- Inserted pages with new footer/header text are detected.
- Aligned footers created through helper functions preserve alignment.
- Header/footer text outside current coordinate bands is characterized or
  supported.
- Region text containing brackets, hashes, backslashes, quotes, and non-ASCII
  characters cannot panic.

Documentation obligation:

- Document semantic page-region diffing versus rendered page-region diffing and
  their invariants.

Review checkpoint:

- Confirm page-region logic no longer acts as an unrelated second diff pipeline
  except where layout context truly requires it.

## Step 10: Update Public And Internal Documentation

Make the docs match the cleaned code.

Key changes:

- Keep this plan current while the cleanup is in progress.
- Update `docs/system-walkthrough-annotated-tree.md` after each completed
  cleanup step.
- Update `docs/code-consistency-review.md` by marking resolved findings and
  adding new findings only when they indicate general design debt.
- Mark `docs/walkthrough.md` and stale parts of `docs/technical.md` as
  historical or update them to the current pipeline.
- Update module-level comments in code, especially where comments mention
  removed span-restoration or fallback behavior.

Tests:

- Documentation-only checks: links resolve, stale symbol names are removed or
  clearly marked historical.
- Search docs for removed APIs such as `content_slots`, `diff_content`, and
  obsolete span-restoration descriptions.

Review checkpoint:

- A new contributor should be able to understand the current pipeline from the
  docs without reading historical plans first.

## Final Acceptance Criteria

- All semantic slots resolve or are not emitted.
- Structural recursion is gated by explicit semantic kind and ownership.
- `plain_text()` is not used as ownership or identity.
- Patch surfaces are named, documented, and invariant-driven.
- Duplicate edit suppression is structural, not text-signature based.
- Equation, footnote, and page-region handling use provenance where available.
- Existing corpus behavior is preserved except where tests are deliberately
  updated to reflect a cleaner invariant.
- Each implementation step includes tests, a focused review, and documentation
  updates.
