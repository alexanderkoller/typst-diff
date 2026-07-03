# Fallback Debt Ledger

This ledger tracks fallback, heuristic, and post-hoc guessing paths that remain
in production code. A ledger entry does not make a fallback desirable; it makes
the debt visible while the refactor replaces it with retained provenance,
explicit patch surfaces, or typed unsupported decisions.

Every `FallbackCode` in `src/decision.rs` must have exactly one entry here.
When a code is instrumented, default CLI execution should warn unless
`--quiet` is set, `--debug` should aggregate counts and bounded examples, and
`--debug-trace` should emit per-decision JSONL events.

## FB-001 Positional Sequence Pairing

- Warning code: `FB-001-positional-sequence-pairing`
- Status: `active`
- Current source sites: `src/annotated.rs` sequence pairing paths around positional pre/realized child pairing.
- Why this is a guess: source children and realized children are paired by order when span/provenance pairing is unavailable.
- User-visible risk: semantic ownership can attach to the wrong realized child when generated scaffolding changes the realized sequence.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/positional-sequence-pairing`.
- Tests: existing context and sequence-pairing regression tests in `tests/integration.rs`; add a warning assertion when instrumented.
- Replacement abstraction: `attributed_block_stream` with retained owner/path IDs for realized children.
- Removal criteria: every visible realized child either has retained provenance or becomes an explicit anonymous unsupported node.

## FB-002 Context Visible-Text Pairing

- Warning code: `FB-002-context-visible-text-pairing`
- Status: `active`
- Current source sites: `src/annotated.rs` context output reattachment.
- Why this is a guess: context output can be associated using visible text when Typst does not expose direct closure provenance.
- User-visible risk: repeated same-text context output can receive the wrong semantic owner.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/context-visible-text-pairing`.
- Tests: context table regressions in `tests/integration.rs`; add repeated same-text context cases for instrumentation.
- Replacement abstraction: recorded context output IDs attached before lowering to body-less visual content.
- Removal criteria: context output is reattached by recorded evaluation provenance rather than visible text.

## FB-003 Visible-Text Owner/Block Matching

- Warning code: `FB-003-visible-text-owner-block-matching`
- Status: `active`
- Current source sites: owner/block lookup and single-block semantic-owner fallback paths in `src/diff.rs`.
- Why this is a guess: visible text similarity is used where ownership identity is not explicitly retained.
- User-visible risk: repeated same-text blocks, labels, links, or references can be paired incorrectly.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/visible-text-owner-block-matching`.
- Tests: same-visible-text link/label tests and repeated table/list cases.
- Replacement abstraction: `content_key` and `attributed_block_stream` owner IDs.
- Removal criteria: visible text is used only for similarity/log presentation, never as ownership identity.

## FB-007 Unique Wrapper/Body Recovery

- Warning code: `FB-007-unique-wrapper-body-recovery`
- Status: `active`
- Current source sites: `src/container_ops.rs` `container_owned_unique_wrapper_path`.
- Why this is a guess: when direct wrapper-body paths and body-text wrapper paths are unavailable, wrapper mapping recovers a body through exactly one realized wrapper of the same kind.
- User-visible risk: a wrapper edit can degrade to opaque replacement if retained wrapper provenance is absent and more than one same-kind wrapper exists.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/unique-wrapper-body-recovery`.
- Tests: opaque wrapper, repeated wrapper, and show-wrapper context table integration tests.
- Replacement abstraction: retained wrapper path provenance from annotated block extraction, deferred to a later provenance cleanup.
- Removal criteria: wrapper descendants carry retained body paths, or ambiguous wrapper bodies report unsupported structure without unique-wrapper recovery.

## FB-008 Unique Partial Item Container Mapping

- Warning code: `FB-008-unique-partial-item-container-mapping`
- Status: `active`
- Current source sites: `src/container_ops.rs` `map_single_item_container_by_unique_text`.
- Why this is a guess: when block extraction presents only one realized list/enum/terms item body and span provenance is absent, the mapper recovers the source slot by requiring exactly one source item with the same visible text.
- User-visible risk: repeated equal item text remains unsupported by this route and can lose slot-level recursion instead of guessing.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/unique-partial-item-container-mapping`.
- Tests: list, enum, terms, and nested-list slot-recursion integration tests.
- Replacement abstraction: retained block-to-slot provenance from annotated block extraction, deferred to a later provenance cleanup.
- Removal criteria: single realized item blocks carry retained slot identity, or the block boundary reports unsupported structure without visible-text recovery.

## FB-009 Anonymous Opaque Pre-Surface Grafting

- Warning code: `FB-009-anonymous-opaque-pre-surface-grafting`
- Status: `active`
- Current source sites: `src/container_ops.rs` `map_realized_or_pre_container_patch_surface` and `container_owned_opaque_patch_surface`.
- Why this is a guess: when a realized block exposes fewer structural leaf paths than the source container slots, the mapper uses the pre container as a patch surface and grafts it into the realized wrapper if possible.
- User-visible risk: a missing retained carrier/slot proof can preserve layout while still attaching the replacement surface to a broad realized wrapper.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/anonymous-opaque-pre-surface-grafting`.
- Tests: list, nested-list, table, grid, boxed/shown table, and opaque-wrapper integration tests.
- Replacement abstraction: retained block-to-slot and carrier provenance from annotated block extraction, deferred to a later provenance cleanup.
- Removal criteria: container mappers receive proved patch-surface paths for single-block and opaque realized carriers, or report an explicit unsupported boundary.

## FB-010 Word-Diff-Or-Opaque Replacement Ladder

- Warning code: `FB-010-word-diff-or-opaque-replacement-ladder`
- Status: `partially-replaced`
- Current source sites: `DiffSelection` replacement selection in `src/diff.rs`.
- Why this is a guess: the algorithm now records the selected area and surface kind, but unsupported-surface cases still use the legacy final word/opaque replacement warning after prior structural routes fail.
- User-visible risk: structured changes can become misleading word edits or overly broad opaque frames.
- Runtime warning behavior: emits by default when the final replacement ladder is selected; suppressed on stderr by `--quiet`.
- Debug/debug-trace event names: `decision_event` with phase `diff/replace-block`.
- Tests: low-similarity container, table/grid, raw block, and opaque visual tests; `cli_emits_fallback_warning_by_default_and_quiet_suppresses_stderr_only`; `cli_debug_trace_records_pipeline_events_without_frame_trace_for_normal_text`.
- Replacement abstraction: `diff_surface::DiffSelection` and `diff_area` with typed surface kinds; unsupported-surface diagnostics are still pending.
- Removal criteria: replacement kind is selected from an explicit diff surface and unsupported structured content is diagnosed without the legacy word-or-opaque fallback warning.

## FB-012 Footnote Marker Matching By Visible Number

- Warning code: `FB-012-footnote-marker-matching-by-visible-number`
- Status: `active`
- Current source sites: `src/annotated.rs` `annotate_footnote_markers`.
- Why this is a guess: visible marker numbers are used when structural footnote marker provenance is insufficient.
- User-visible risk: nearby inserted/deleted footnotes can pair marker/body edits incorrectly.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/footnote-marker-matching-by-visible-number`.
- Tests: footnote body and nearby-footnote insertion tests.
- Replacement abstraction: footnote marker/body provenance in `attributed_block_stream`.
- Removal criteria: marker/body matching uses retained marker IDs or explicit unsupported diagnostics.

## FB-013 Rendered-Region Source-String Align Parsing

- Warning code: `FB-013-rendered-region-source-string-align-parsing`
- Status: `active`
- Current source sites: `src/diff.rs` `rendered_region_source_wrapper`.
- Why this is a guess: when a contextual page-region expression hides its authored wrapper behind a `ContextElem`, the diff recovers `align(...)` by reading the source span instead of using retained wrapper provenance.
- User-visible risk: ordinary source text or unusual formatting can be mistaken for wrapper structure.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/rendered-region-source-string-align-parsing`.
- Tests: contextual page header/footer and running-header corpus tests.
- Replacement abstraction: retained page-region/context wrapper provenance from `context_recording` or annotated page-style extraction.
- Removal criteria: rendered-region wrapper decisions come from retained structural wrapper provenance or explicit unsupported diagnostics, never source-string parsing.

## FB-014 Rendered-Region Layout Alignment Fallback

- Warning code: `FB-014-rendered-region-layout-alignment-fallback`
- Status: `active`
- Current source sites: `src/diff.rs` `rendered_region_layout_wrapper` and `rendered_region_page_layout_alignment`.
- Why this is a guess: when no retained wrapper is available, the diff infers alignment from rendered text geometry after layout.
- User-visible risk: page geometry, margins, or coincidental text placement can be mistaken for an authored wrapper.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/rendered-region-layout-alignment-fallback`.
- Tests: contextual footer total-pages and running-header corpus tests.
- Replacement abstraction: retained page-region/context wrapper provenance that records the wrapper used to produce each rendered page-region instance.
- Removal criteria: rendered-region wrappers are retained from semantic/context provenance, or missing wrapper provenance is reported as unsupported without geometry inference.
