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
- Current source sites: `src/container_ops.rs` `unique_realized_wrapper_path`.
- Why this is a guess: wrapper body recovery uses uniqueness in the realized subtree when direct paths are absent.
- User-visible risk: repeated wrappers with identical text can map to the wrong body.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/unique-wrapper-body-recovery`.
- Tests: wrapper body edit tests and paragraph-split-inside-wrapper corpus case.
- Replacement abstraction: `content_tree` path mapping with retained wrapper provenance.
- Removal criteria: wrapper descendants are addressed by retained path or explicit unsupported mapping.

## FB-008 Unique Partial Item Container Mapping

- Warning code: `FB-008-unique-partial-item-container-mapping`
- Status: `active`
- Current source sites: `src/container_ops.rs` `map_unique_partial_item_container`.
- Why this is a guess: partial item correspondence is recovered from a unique structural-looking match.
- User-visible risk: list/terms edits can move into the wrong repeated item.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/unique-partial-item-container-mapping`.
- Tests: nested list, enum, and terms insertion/deletion tests.
- Replacement abstraction: `edit_script` over explicit item IDs or stable slot paths.
- Removal criteria: partial item edits use retained container slots or report unsupported structure.

## FB-009 Anonymous Opaque Pre-Surface Grafting

- Warning code: `FB-009-anonymous-opaque-pre-surface-grafting`
- Status: `partially-replaced`
- Current source sites: opaque patch-surface selection in `src/container_ops.rs`.
- Why this is a guess: opaque visual and grafted block-body patch surfaces are now named, but the carrier association can still depend on a recovered opaque realized surface rather than a retained owner/carrier ID.
- User-visible risk: whole-surface visual replacement can obscure finer structural edits or attach to the wrong carrier.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/anonymous-opaque-pre-surface-grafting`.
- Tests: opaque graphic and figure body tests.
- Replacement abstraction: `patch_surface::PatchSurface::OpaqueVisual` and `PatchSurface::GraftedBlockBody` with explicit carrier provenance from an attributed block stream.
- Removal criteria: every opaque graft records its patch-surface variant and a retained owner/carrier proof, and remaining opaque-graft decisions are instrumented or removed.

## FB-010 Word-Diff-Or-Opaque Replacement Ladder

- Warning code: `FB-010-word-diff-or-opaque-replacement-ladder`
- Status: `partially-replaced`
- Current source sites: `DiffSurfaceEdit` replacement selection in `src/diff.rs`.
- Why this is a guess: the algorithm now records the selected surface kind, but unsupported-surface cases still use the legacy final word/opaque replacement warning after prior structural routes fail.
- User-visible risk: structured changes can become misleading word edits or overly broad opaque frames.
- Runtime warning behavior: emits by default when the final replacement ladder is selected; suppressed on stderr by `--quiet`.
- Debug/debug-trace event names: `decision_event` with phase `diff/replace-block`.
- Tests: low-similarity container, table/grid, raw block, and opaque visual tests; `cli_emits_fallback_warning_by_default_and_quiet_suppresses_stderr_only`; `cli_debug_trace_records_pipeline_events_without_frame_trace_for_normal_text`.
- Replacement abstraction: `diff_surface` and `diff_area` with typed surface kinds and unsupported-surface diagnostics.
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
- Current source sites: rendered page-region wrapper parsing in `src/diff.rs`.
- Why this is a guess: generated Typst snippets are inferred from source strings for alignment wrappers.
- User-visible risk: unusual source formatting or nested wrappers can produce inaccurate page-region annotation.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/rendered-region-source-string-align-parsing`.
- Tests: page header/footer and rendered-region alignment tests.
- Replacement abstraction: rendered-region source spans and semantic wrapper provenance.
- Removal criteria: wrapper decisions come from retained semantic page-region structure, not source-string parsing.

## FB-014 Generated Typst Snippet Panic Path

- Warning code: `FB-014-generated-typst-snippet-panic-path`
- Status: `active`
- Current source sites: generated snippet construction in rendered-region handling.
- Why this is a guess: dynamically generated Typst is parsed/compiled after escaping visible text.
- User-visible risk: malformed generated snippets can panic or fail late instead of producing a typed diagnostic.
- Runtime warning behavior: not yet instrumented.
- Debug/debug-trace event names: planned `fallback/generated-typst-snippet-panic-path`.
- Tests: add malformed/edge escaping tests when instrumented.
- Replacement abstraction: direct `Content` construction for rendered-region edits.
- Removal criteria: generated Typst snippets are removed or failures return typed diagnostics.
