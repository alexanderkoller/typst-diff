# Technical Decisions

## Annotated Block Extraction Is Deferred Phase 8

- Phase 1 is complete only to the safe indexed-stream boundary: production block
  ops are indexed, block-op consumption reads attributed streams by index, and
  the owner/equation cursor objects are gone.
- The remaining owner/equation claim helpers are still production-useful:
  `collect_block_owner_claims`, `collect_equation_origin_block_claims`, and
  `find_annotated_block_owner` preserve table, figure, footnote, equation, and
  opaque-wrapper owner placement during stream construction.
- A direct replacement attempt that walked the annotated tree and emitted stream
  claims separately from block extraction broke integration tests by shifting
  owner placement. The failures clustered around table/grid recursion, figure
  body/caption ownership, footnote body recovery, display equation tokenization,
  and opaque wrapper deduplication.
- The correct follow-up is a new `extract_annotated_block_units`-style primitive
  that emits each `DiffBlock` together with its attribution in the same pass that
  decides block boundaries.
- Do not keep private helpers merely because they have direct tests or might be
  useful. If a helper stops serving production behavior after a refactor, delete
  it and delete or rewrite tests that only protect the obsolete private behavior.

Tradeoff: deferring attributed block extraction keeps later deletion phases
unblocked while avoiding a brittle half-step that loses provenance. The debt is
explicitly moved to Phase 8 rather than being treated as completed Phase 1 work.

## Block Ops Consume Attributed Streams By Index

- The lean deletion pass begins by promoting indexed block attribution inside
  the production diff pipeline.
- Internal block matching now carries old/new block indices through Myers and
  edit-zone replacement selection, while the existing public/debug `BlockOp`
  view remains content-rich for tests and diagnostics.
- Public `match_edit_zones` now converts `BlockOp` values to indexed ops, runs
  the same indexed matcher as production, and materializes `BlockOp` values only
  on return.
- The old content-rich `pair_edit_zone` implementation was deleted, so edit-zone
  replacement selection has one implementation rather than parallel production
  and compatibility paths.
- The main block-op loop reads `AttributedBlockStream` entries by block index
  instead of searching for an unconsumed stream item with matching realized
  content.
- `BlockOwnerCursor` and `EquationOriginBlockCursor` were removed. Stream
  attribution now builds owner and equation claim vectors locally and consumes
  them while constructing `AttributedBlockStream`.
- The unused `owner_path` field was removed from attributed stream items because
  no production code populated it yet; keeping an always-unknown debug field
  made the provenance boundary look stronger than it was.
- `AttributedBlock` no longer stores a duplicate realized block payload solely
  for content-equality lookup.
- `cargo check --all-targets`, `cargo test --all-targets`,
  `tests/check_fallback_ledger.sh`, and `tests/run_passing_corpus.sh` pass after
  the change.

Tradeoff: this removes the cursor objects and the block-op content-search
bridge, but not the underlying owner discovery debt. `collect_block_owner_claims`,
`collect_equation_origin_block_claims`, and `find_annotated_block_owner` still
recover attribution from realized content until annotation or block extraction
can retain owner/path provenance directly.

## Legacy Cleanup Keeps Only Proven Bridges

- Phase 11 records the current production Rust line count at 19,010 lines after the deep-cut refactor phases.
- The active consistency review now distinguishes resolved debt from remaining debt: unique changed slot pairing, unique slot-bearing descendant search, duplicate finished-edit pruning by text signature, broad empty-block equation carrier recognition, and the generated rendered-region snippet panic path are no longer described as live production paths.
- The technical reference module list now includes the refactor boundary modules: `content_tree`, `content_key`, `patch_surface`, `diff_surface`, `diff_area`, `edit_script`, `style_context`, and `attributed_block_stream`.
- The older invariant cleanup plan is marked historical so it remains useful design background without competing with the current deep-cut phase plan.
- The fallback ledger remains synchronized with active warning codes. `FB-013` stays active because opaque contextual page-region wrappers still need source-span `align(...)` parsing until context output carries wrapper provenance.
- `BlockOwnerCursor` and `EquationOriginBlockCursor` have now been deleted. The remaining owner/equation claim collectors still live inside attributed stream construction until retained owner/path IDs can replace realized-content recovery.

Tradeoff: this phase trims stale documentation and records the current architecture without pretending every bridge is gone. The remaining cursor bridge and visible-text/source-string debts are intentionally left explicit and ledgered instead of hidden behind a superficial cleanup.

## Page Regions Share Area And Surface Concepts

- Phase 10 keeps semantic page regions and rendered page regions under the `DiffAreaKind` boundary introduced earlier: semantic header/footer/background/foreground edits use `SemanticPageRegion`, while contextual rendered text changes use `RenderedPageRegion`.
- Rendered-region word and segment diffs continue to use explicit `DiffSurfaceKind::RenderedRegionText` and `RenderedRegionSegment` surfaces rather than anonymous word operations.
- Rendered-region alignment preservation now reads `AlignElem` from the page-region content tree and maps its horizontal alignment directly before considering older recovery paths.
- When an opaque `ContextElem` hides the authored wrapper body, rendered-region alignment may still use the source-span `align(...)` parser as a ledgered fallback. Extracted rendered text geometry is used only after structural and source-span wrapper recovery fail, and only when all non-empty rendered pages agree on the same left, center, or right placement.
- Generated rendered-region Typst snippets still exist for the page-number-dependent context expression, but construction is now fallible and returns an error instead of panicking with `expect`.
- Regression coverage includes rendered header text containing brackets, hashes, backslashes, quotes, and non-ASCII text, plus an assertion that alignment comes from the retained `AlignElem` wrapper.
- The generated-snippet panic warning code has been retired from the fallback ledger. The source-string align parsing code remains active but partially replaced because contextual page-region closures do not expose their bodies as `Content`.

Tradeoff: rendered page-region extraction remains layout-dependent because contextual headers and footers can differ per page only after layout. This phase removes the panic path and makes content-tree wrapper analysis the first route, but keeps the source parser visible in the ledger for opaque context output; direct `ContextElem` construction and inserted/deleted rendered-only page instances remain later cleanup work.

## Footnote And Equation Provenance In The Block Stream

- Phase 9 extends `attributed_block_stream` so stream items carry footnote body content alongside existing owner and equation-origin metadata.
- Deleted-footnote body recovery now reads from the attributed old/new streams instead of scanning the full annotated tree after block diffing.
- Display equation origins no longer transfer to arbitrary text-empty target blocks. The equation-origin cursor only hands an origin to the exact realized block claim and skips empty unmatched claims without treating an empty target as proof.
- The broad empty-block equation carrier warning code has been retired from the fallback ledger because empty equation shells are now represented through retained stream equation provenance.
- Footnote marker annotation no longer accepts any visible matching number nested anywhere in a sequence. It accepts marker text directly, including transparent style wrappers around that marker.
- Visible-number footnote marker matching remains an active ledger item because the remaining marker post-pass still uses the rendered marker number when no retained marker ID is available.

Tradeoff: this phase moves deleted footnote bodies and equation carrier decisions onto the stream boundary without rewriting Typst's footnote marker realization. That removes the broad post-hoc equation carrier guess and narrows marker matching, while leaving the explicit visible-number marker debt visible until marker IDs can be retained at realization time.

## Edit Scripts Drive Slot And Child Recursion

- Phase 8 introduces `edit_script` as the shared ordered edit-script builder for semantic children.
- Slot diffs now use the same script consumer for same-shape and LCS modes. Same-shape mode pairs by position, while LCS mode pairs by slot child match keys.
- Generic nested child-sequence diffs now use the same edit-script operation model as slot diffs instead of maintaining local Myers walking logic.
- Nested wrapper/body recursion no longer depends on finding exactly one slot-bearing descendant. When a paired child contains slot-bearing descendants, its meaningful children are diffed by script, so multiple nested container changes can produce multiple nested edits.
- The unique changed slot pair fallback, slot-bearing descendant pair fallback, and duplicate edit pruning by text signature have been removed from production code, and their warning codes have been retired from the fallback ledger.
- Duplicate prevention for the tested repeated-container and opaque-wrapper cases now comes from owner/slot/script selection rather than a late text-signature pruning pass.

Tradeoff: this phase unifies the script mechanics and removes the broad uniqueness/pruning fallbacks, but `ContainerOps` still exposes some stable children through existing slot mapping APIs rather than a fully generalized edit-script input trait. Table/grid shape changes remain covered by the current cell slot addressing and tests, with broader row/column addressing still a later cleanup target.

## Attributed Block Streams Carry Ownership Into Block Ops

- Phase 7 introduces `attributed_block_stream` as the carrier for semantic block attribution.
- Diff construction now builds old/new attributed streams from the same non-parbreak realized blocks used by block LCS before walking `BlockOp`s.
- Each stream item carries realized block content, the primary owner claim, a separate fallback owner, semantic owner key, owner metadata, equation origins, and footnote/patch-surface presence.
- The block-op loop claims each old/new stream item at most once by realized block content and reads owner/equation provenance from stream items instead of probing `BlockOwnerCursor` and `EquationOriginBlockCursor` inside each op arm.
- Delete/insert pairing can peek at a matching unconsumed new attributed item without consuming it, so semantic owner replacement remains available even when edit-zone matching pairs blocks out of side-local order.
- Primary owner claims remain separate from fallback owners so deferred empty-carrier handoff still wins for invisible wrapper/equation/opaque owners.
- The old exact/single-block owner recovery is still used inside stream construction as a compatibility bridge. It no longer appears at each edit-construction decision point, but it has not yet been fully replaced by retained owner paths.

Tradeoff: this phase moves ownership consumption to an explicit stream boundary without changing realized extraction or renderer behavior. The stream is now the single place where sparse owner/equation claims are attached to matched blocks, but owner paths are still not retained from annotation, so fallback owner recovery remains in the builder until provenance is strong enough to delete it safely.

## Diff Surfaces And Areas Name The Diff Boundary

- Phase 6 introduces `diff_surface` and `diff_area` as the first shared model for what is being diffed.
- Replacement selection now returns `DiffSurfaceEdit<EditContent>` instead of a bare `EditContent`, naming the chosen surface as word tokens, equation tokens, raw lines, non-token display content, or opaque visual content.
- Rendered page-region page and segment diffs also pass through `DiffSurfaceEdit`, with explicit rendered-region surface kinds while preserving the existing `WordOp` payloads.
- `DiffAreaKind` names body blocks, semantic page regions, rendered page regions, and structured container regions. The current phase uses these names for trace/selection plumbing without moving all area logic yet.
- The word-diff-or-opaque fallback warning remains active for compatibility and because unsupported-surface handling is not complete. The fallback now carries an explicit selected surface in debug trace reasons, rather than hiding the surface choice in the ladder.

Tradeoff: this phase introduces the surface/area contract before deleting the old specialized paths. Raw/code, equation, opaque, and rendered-region behavior is still implemented by existing functions, but their outputs now flow through a common selection wrapper that later phases can consolidate.

## Patch Surfaces Carry Their Invariant

- Phase 5 introduces `PatchSurface` as the typed value stored by `Annotation::patch_surface` and returned by container slot mapping.
- Non-realized edit surfaces now record why they are legal: authored pre-container surfaces, grafted block-body surfaces, layout-preserving sequences, opaque visual surfaces, or rendered edit surfaces produced by applying realized edits.
- Annotation and rendering code must read patch content through `PatchSurface::as_content`, keeping the reason attached until the last possible point.
- Container mapping no longer returns anonymous raw `Content` as a patch surface. Wrapper body grafts, explicit pre-container surfaces, opaque visual surfaces, and single-item layout-preserving repairs are named at their creation sites.
- Applying a `RealizedEdit` to an annotated tree produces `RenderedEditSurface`, separating edit-output surfaces from provenance surfaces created during realization annotation.
- The anonymous opaque pre-surface fallback is only partially replaced: the surface is now typed, but the remaining carrier association still needs retained owner/carrier provenance before the ledger entry can be removed.

Tradeoff: this phase changes the internal representation without redesigning patch selection or removing the remaining recovery paths. That keeps behavior stable while making future patch-surface cleanup auditable by variant instead of by raw `Option<Content>`.

## Content Keys Name The Comparison Purpose

- Phase 4 introduces `content_key` as the single home for comparison keys.
- Block LCS uses `BlockEqualityKey`: equality and hashing remain Typst structural content equality, while ordering uses visible text plus structural hash only to satisfy the `similar` API contract.
- Token, child-sequence, slot-LCS, context, visible-unit, normalized-visible-text, and opaque-replacement comparisons now call named key constructors instead of local ad hoc string helpers.
- Presentation keys intentionally include visible formatting identity such as emphasis, heading, equation blockness, raw/code, highlight, and visual block decoration, while ignoring metadata-only wrappers such as tags.
- Context presentation keys intentionally ignore ordinary text payload and focus on the surrounding structural context that can affect rendered references or headings.
- Normalized visible-text keys remain explicitly named fallback-style comparisons. They are still used only where the current pipeline lacks stronger provenance, so the visible-text ledger entries remain active rather than being treated as solved.

Tradeoff: this phase mostly centralizes existing semantics instead of eliminating every visible-text identity fallback. That makes later replacement work auditable: call sites now reveal whether they are asking for structural equality, presentation identity, context identity, similarity support, or an acknowledged normalized-visible-text match.

## Content Tree And Style Context Are Shared Mechanics

- Phase 3 extracts repeated content traversal and mutation mechanics into `content_tree`.
- Path lookup, realized-content replacement, annotated-tree replacement, realized-child mapping, and transparent-wrapper descent now have one implementation.
- The extraction is deliberately mechanical: container-specific child knowledge still stays in `container_ops`, while `content_tree` owns reusable tree navigation and rewrite operations.
- Page-style partitioning now lives in `style_context`, including the shared page-style predicate, page/non-page splits, marginal style selection, and the sticky page-style state transition.
- Diff construction, evaluation, and annotation call these helpers instead of maintaining local copies of the same filtering rules.
- Root page-style marginal sanitization remains in `diff.rs` because it constructs concrete annotation wrapper content, not just style-context bookkeeping.

Tradeoff: this phase reduces duplicated mechanics without introducing new matching behavior or new provenance rules. Some larger responsibilities from the target architecture, such as full child extraction ownership and page-region extraction, remain local to their current modules until they can move behind clearer data boundaries.

## Fallback Warnings Use Lightweight Decision Events

- Fallback warnings are represented as compact `DecisionEvent` records keyed by stable `FallbackCode` labels.
- The CLI records fallback decisions during diff construction, prints compact warnings on stderr by default, and suppresses only those warnings with `--quiet`.
- Debug bundles write aggregate fallback counts and bounded examples to `diff/fallback-warnings.yml`.
- Debug trace writes per-decision JSONL records as `decision_event` entries in `diff/pipeline-events.jsonl`.
- The first instrumented fallback codes are unique changed slot pair, duplicate edit pruning by text signature, and the word-diff-or-opaque replacement ladder.
- Decision metadata stores small labels, phases, indices/paths when available, and bounded previews; it does not retain cloned Typst content trees just for diagnostics.

Tradeoff: Phase 2 instrumentation starts with fallback sites that already have clear decision boundaries. Other ledger entries remain active and uninstrumented until their call sites can emit warnings without muddying provenance or turning broad code paths into noisy false positives.

## Passing Corpus Gate And Fallback Ledger

- Corpus regression protection now distinguishes the intended baseline from the full corpus state.
- `tests/run_corpus.sh` can write the current passing cases, run an exact corpus basename, and require a recorded passing list.
- `tests/run_passing_corpus.sh` uses `tests/corpus-passing-baseline.txt` so every currently passing corpus case must remain passing during refactors, while pre-existing failures stay visible but outside the gate.
- The corpus harness accepts test-only environment overrides for the corpus directory, output directory, manifest, and binary path; the default corpus behavior is unchanged.
- `tests/check_corpus_harness.sh` exercises list mode, exact mode, passing-list writes, and missing required entries using a miniature temporary corpus.
- Fallback debt has a stable code table in `src/decision.rs` and a human ledger in `docs/fallback-debt-ledger.md`.
- The ledger audit checks that code labels and ledger warning-code entries stay synchronized before fallback instrumentation is threaded through production call sites.
- The CLI accepts `--quiet` as the future fallback-warning suppression switch; ordinary progress logging and hard errors are intentionally unchanged.

Tradeoff: the fallback warning codes are introduced before runtime warning emission. This keeps the first phase behavior-preserving and gives later instrumentation a stable naming contract, but the ledger entries remain `active` and not yet observable at runtime until Phase 2 call-site plumbing is implemented.

## Inert Old Display Preserves Alignment Wrappers

- Deleted old content may still need non-page layout wrappers to render at the old visual position.
- `align` wrappers are retained in old display surfaces, with only their body sanitized recursively.
- The same retention rule applies to the other semantic wrapper bodies that typst-diff already recognizes: `pad`, `place`, `columns`, `box`, `block`, and body-bearing shape wrappers.
- Annotation also treats these wrappers as transparent child wrappers so delete/insert styling is applied inside the wrapper rather than replacing it with plain text.
- Page styles are still stripped from inert old display surfaces; preserving `align` does not make old headers, footers, counters, or page configuration live again.
- Explicit non-page styles inside deleted header content, such as `text(font: ...)[...]`, are retained. Ambient layout-time styles that Typst applies to a header from outside the header content are not available in the semantic page-region payload; preserving them requires retaining page-region style provenance or carrying rendered-run style information, not guessing from nearby body content.

Tradeoff: this preserves known source/layout structure for deleted content instead of inferring alignment from rendered coordinates. Other layout wrappers should be added only when their retained body can be sanitized cleanly and they do not reactivate old document state.

## Semantic Owner Edits Anchor To Realized Carriers

- Semantic owners may supply provenance, slots, and source equation origins for edit construction.
- They do not determine output position. `LayoutCursor` advances only to the realized target block, not to an owner's effective text or render surface.
- Invisible owner shells with visible wrapper/shape slots are deferred; their owner claim can be handed to the following empty replacement carrier instead of producing an early standalone edit.
- Inline equation origins are also deferred through invisible shells and consumed by the next realized block that actually contains equation carriers.
- Display equation owners are still allowed to emit their own empty visual carrier edits, because the display equation is the realized block.
- When a display equation owner edit has already rendered the old/new formula tokens, the following empty realized block shell for that same display equation is consumed as represented carrier output. This uses the existing equation-owner provenance and block order, rather than matching arbitrary empty blocks after the fact.
- Opaque visual owners such as shapes and images may also claim a single empty realized carrier when their retained effective render surface is the only available provenance for the visual. The owner supplies old/new opaque replacement payloads; the carrier still supplies layout position.
- Source visual leaves that realize to text-empty opaque carriers retain their source visual element as an annotation patch surface. This preserves image/SVG provenance before block diffing, instead of recovering it from an anonymous carrier later.
- Effective render surfaces remain duplicate-suppression keys only after an owner edit has been anchored; they are not alternate layout anchors.

Tradeoff: inline math now keeps equation provenance in the paragraph-level edit rather than emitting a separate empty equation block. Display math keeps a standalone semantic edit but consumes its companion realized carrier so the inserted formula is not rendered a second time. Empty graphic carriers can now produce opaque replacements only when an annotated owner already retained the visual surface, preserving document order without treating arbitrary empty scaffolding as graphics.

## Old/New Provenance In Annotation

- `DiffBlockEdit` carries explicit `BlockBaseProvenance`.
- Equal, inserted, and ordinary replacement bases are live new content.
- Pure deleted bases are inert old display content.
- Context-split replacement bases are mixed by construction: old inert display content beside live new content.
- Annotation should trust provenance recorded during diffing rather than infer old/new origin from element shape.

Tradeoff: this keeps old state, labels, and contexts from executing during annotated render, but it also exposes places where extraction has lost source-owner provenance and cannot yet make a clean visual-diff decision.

## Empty Structural Content Is Not Visible Diff Content

- Text-empty structural differences are not emitted as opaque replacement frames.
- Invisible scaffolding such as spacing, metadata, tags, page/style context, and state machinery must remain invisible unless it contributes to a retained live-new behavior.
- Opaque visual replacements are reserved for realized visual surfaces or annotated owners that are known visual wrappers, such as figure bodies containing rectangles.
- Old deletion payloads whose inert display surface is empty and non-visual are dropped during diff construction.

Tradeoff: top-level graphics that realize to anonymous empty blocks still require a cleaner provenance-carrying extraction change. They should not be recovered by treating arbitrary empty blocks as graphics.

## Renderer Boundary

- The renderer remains unchanged.
- Problems caused by old/new merging are fixed in diff construction and annotation inputs, not by teaching the renderer special cases.

## Equation Origin Provenance

- Source equation tokens are attached to annotated equation owners first.
- Anonymous text-empty blocks are not treated as equation carriers.
- The equation-origin block cursor is sparse: it consumes an origin claim only when the current diff block is the annotated block that owns the origin.
- If a semantic equation realizes to an empty display block, token extraction uses the stored source equation only after the realized owner produces no meaningful tokens.

Tradeoff: display equations still get useful modification-log and annotation tokens, but ordinary layout/scaffold blocks can no longer borrow unrelated formulas from later in the document.

## Inert Old Deletes Inherit The New Page Context

- Old display surfaces retain realized non-page styling so deleted headings and shown content still look like the old rendered document.
- Page styles are not part of the old display surface. A standalone inert-old deletion is placed in the annotated stream under the current output page style, rather than resetting to empty page styles.
- This keeps deleted old content in the new document's page universe while preserving its visible realized form.

Tradeoff: genuinely deleted old scaffold remains visible as red struck content. Suppressing it would require a document-level deletion policy, not a provenance or page-style fix.

## Non-Token Display Surfaces Are Not Token-Replacement Bases

- Old display sanitization preserves table and grid containers, recursively sanitizing their cells instead of extracting cells as a flat child sequence.
- Old display sanitization also preserves `box` wrappers whose bodies contain such non-token display surfaces.
- Deleted or inserted table/grid owners, and boxes that wrap them, are emitted as whole structural surfaces when the effective annotated owner carries that surface, even if the extracted block placeholder is text-empty.
- Replacements with matching semantic owners and stable slots recurse through those slots. This includes shown tables whose realized output is wrapped in a display container: the cell edits belong to the semantic table owner, not to a later anonymous realized surface.
- Replacements that cannot be handled through stable slots use opaque whole-surface replacement instead of word-token replacement when their effective surface contains table/grid content.
- Plain text boxes are not part of this guard: wrapper slots already support localized word edits for simple boxed labels.
- Lists and enums are not part of this guard either: they already have stable semantic slots, and nested-list tests rely on localized slot/word edits preserving the list hierarchy.

Tradeoff: table/grid changes without a clean semantic owner may still be whole-surface replacements, but semantically matched tables remain cell-local and the annotated tree no longer destroys matrix or boxed matrix layout by splicing a flat token sequence where a structured display surface is required.

## Recursed Semantic Owners Suppress Their Duplicate Realized Surfaces

- Some show rules produce both a semantic owner block and a later anonymous realized display surface for the same content.
- Once a semantic owner has been recursed through stable slots, its effective display surface is recorded as already represented.
- A later anonymous non-token display surface is skipped only when it matches one of these recorded surfaces. This is provenance-driven: the skip is tied to a specific owner that has already produced edits, rather than inferred from table shape alone.
- Pure old deletions are not skipped this way. With no live-new owner to recurse through, their inert old display surface is still the visible deletion payload.

Tradeoff: duplicate-surface suppression currently matches recorded surfaces by normalized visible text because the anonymous realized surface does not carry the owner key. The match is deliberately narrow: it can only consume surfaces that were recorded from an already-recursed semantic owner.

## Wrapper Slots May Realize Below Paragraph Scaffolding

- Authored wrappers such as `box(table(...))` do not always realize as the top-level block node.
- Realization can insert paragraph, sequence, tag, or style scaffolding around the wrapper while preserving the wrapper itself as a descendant.
- Wrapper slot mapping first uses the direct wrapper body path. If that is absent, it locates a realized wrapper descendant with the same wrapper kind and body text, then maps the authored wrapper body to that realized body path.
- This keeps table/grid/list semantics inside wrapper bodies available to diffing without treating the enclosing paragraph as the semantic owner.

Tradeoff: descendant wrapper lookup uses wrapper kind plus visible body text because the scaffolded wrapper descendant does not expose an explicit owner key. The lookup is confined to the container-mapping phase for an already-known authored wrapper.

## Context Output Can Carry Semantic Owners

- A `ContextElem` stores an executable closure, not an inspectable source body.
- When a context expression realizes to ordinary content, annotation treats the realized output as the semantic source for that context result. This lets generated structures such as `#context [#box(table(...))]` retain wrapper and table slots for diffing.
- Sequence pairing gives a context pre-child the realized run up to the next source-span match before ordinary span matching. Generated tags and scaffold nodes can share the context span, so letting them win the span match would detach the actual context output from its provenance.
- If a context expression remains a live `ContextElem` after realization, it stays an opaque context leaf. This avoids infinite self-annotation and preserves header/footer-style live context behavior.

Tradeoff: semantic recovery inside context output is based on Typst's evaluated result rather than the original closure body. That is the strongest available provenance after evaluation, because the closure body is not exposed as content.

## Compact Mode Suppresses Nested Deleted Inserts

- Compact substitution mode still leaves standalone whole-block deletions visible.
- Deleted payloads inserted into an existing live-new base through `InsertBefore`, `InsertAfter`, or `Append` are skipped in compact mode. This keeps structural replacements such as table row removals from rendering old-only rows when the user requested compact substitutions.
- Inserted and modified payloads remain visible; only pure deleted edit content is skipped in this nested insertion path.

Tradeoff: compact table diffs are cleaner for row/cell removals, but the edit model still records those deletions for logs and non-compact rendering.

## Context Output Keeps Its Visible Owner

- Sequence pairing may see invisible source children such as state updates, empty sequences, spaces, and parbreaks before a `ContextElem` whose evaluated output is visible.
- Invisible pre-realization children are no longer allowed to consume visible realized children during span pairing. They may consume invisible realized scaffolding, or remain as invisible live-new content.
- This preserves the evaluated visible run for the context element that produced it, letting context-generated wrappers and tables keep semantic slots.
- Some context output materializes style-dependent fields only after Typst applies a `StyledElem`; annotation now asks Typst to materialize those fields before descending through styled children.
- Block-owner cursors skip only invisible nonmatching claims. Visible nonmatches remain available for later blocks so invisible source scaffolding cannot steal the owner claim for a following visible table.

Tradeoff: context-generated content is still recovered from Typst's evaluated display content, not from the inaccessible context closure body. When Typst's display content contains a materializable wrapper/table, we recurse into it; if Typst produces a genuinely opaque visual callback with no materializable structure, that requires a deeper eval-stage design rather than owner-claim heuristics.

## Style-Aware Wrapper Child Recovery

- Some realized wrapper bodies are stored in Typst style fields rather than directly on the wrapper element under the default style chain.
- Child extraction and style-dependent materialization now thread the active realized style chain through `StyledElem` descent before reading wrapper bodies.
- This lets context-generated boxes whose evaluated result still contains a materializable table body expose that table to the normal slot diff.
- Compact substitution mode hides all deleted word runs inside modified content. Standalone deleted blocks remain visible, but modified table cells render only the new value in compact mode.

Tradeoff: when Typst or a package has already lowered a context-generated table to a display-only box with no recoverable body under the active style chain, typst-diff still cannot reconstruct table cells from that visual surface without retaining more provenance during evaluation.

## Body-Less Context Output Requires Evaluation Provenance

- The SFB funding overview tables expose a harder case than ordinary `table(...)`, `box(table(...))`, or recoverable `#context [#box(table(...))]` content.
- In the raw and normalized SFB trees, the relevant source node is a `ContextElem` with no inspectable children. During realization Typst executes its internal closure and then returns a visible `box` surrounded by locatable tags.
- For these specific tables, the realized `BoxElem` has visible text but no recoverable body under the default or active style chain. At that point, table cells are no longer present in the content tree that typst-diff receives.
- We therefore keep recursing into tables whenever a semantic table or materializable wrapper body is present, and we test those cases, but we do not reconstruct table cells from a flat body-less visual box.
- The fix for the SFB case is to retain the evaluated result of `ContextElem` before Typst lowers it to display-only content. typst-diff installs a local native context show rule wrapper that delegates to Typst's built-in context rule, records the returned structured `Content`, and returns it unchanged.
- The minimal reproducer is a top-level show wrapper that emits a `#context` overview table before the wrapper body, while the wrapper body later performs the state updates read with `state.final()`. A plain context table, even one using forward state updates, still recovers cells; the show-wrapper boundary is what leaves only a body-less realized box.
- `ContextElem` stores its closure in a private internal `func` field. The generic `Content::at("func")` path typechecks but does not expose this internal field at runtime, so the recording hook sits at realization time rather than trying to re-evaluate context bodies later.
- Context output created inside a show rule is not present in the original pre-realization tree. The recorded output is reattached through Typst's own generated context tags and the source child that contains the `ContextElem`, including the following generated visible run when the immediate source placeholder is invisible.
- Equal-length source/realized sequence zipping is disabled for context-bearing sequences, because equal child counts can be accidental: the visible context output may be a later generated child while the source context placeholder itself remains invisible.
- A recovered semantic owner may contain exactly one visible non-empty block plus invisible context scaffolding. Block-owner fallback therefore uses the existing normalized owner match for single-block semantic owners so the visible block gets the table owner rather than an anonymous opaque replacement.

Tradeoff: replacing Typst's native context rule requires a narrow unsafe replacement of one private `NativeRuleMap` entry because Typst exposes rule registration but not rule replacement. The wrapper does not change renderer behavior: it calls the built-in rule, records its returned `Content`, and returns that same `Content`.

## SFB Context Table Diagnosis Is Still Open

- The context-recording path now covers ordinary context-generated boxed tables, state-generated tables, and a regression where a context table appears after a generated page break and changes enough to become a block replacement.
- The real SFB overview tables still produce opaque replacements in the modification log after this work. The current evidence is that semantic grid owners with slots and patch surfaces exist earlier in the edit stream, but the later visible table carriers are still emitted as anonymous opaque replacements.
- Several attempted cleanups were kept because they match the broader design: table/grid owners can use non-empty effective surfaces when their realized context shell is empty, already-recursed table/grid owners record an effective surface for duplicate suppression, and owner lookup can compare retained table/grid effective surfaces directly.
- These changes are not sufficient for the SFB page-55 failure. The remaining bug is likely in the handoff from recorded context output to the later visible `box` carrier or in duplicate-surface consumption after semantic recursion.

Tradeoff: the retained changes improve tested context-table behavior without adding SFB-specific document heuristics, but the main SFB opaque-table report is not fixed yet.

## Empty Equation Shells Stay Live

- Display equations can realize as text-empty block shells whose visible formula and numbering are produced by Typst during layout.
- These shells are semantically visible even when their realized content has empty plain text, so owner and equation-origin cursors must not skip them as disposable scaffolding.
- Inserted empty equation shells remain the live rendered base. A `MarkBaseInserted` edit applies insertion styling to that live shell while carrying the inserted formula tokens for logs, so the equation and number render green without replacing Typst's counter/label carrier.
- Equal empty equation shells use the new-side equation owner as the live base instead of falling back to anonymous empty content.
- Rendered equation-reference number changes caused by these live equation insertions remain visible text diffs. For example, when an inserted equation changes a later reference from `Equation 1` to `Equation 2`, the `1 -> 2` substitution is still marked in the annotated main text.

Tradeoff: `MarkBaseInserted` separates diagnostic token reporting from content replacement for inserted empty equation shells. This preserves Typst's live counter/label machinery while still rendering the inserted equation in green, without modifying the renderer.

## Deleted Equation Tokens Preserve Math Structure Through Styles

- Deleted equation word tokens should render as math equations containing a red `CancelElem`, not as plain struck text.
- Inline equation tokens commonly carry transparent style wrappers around the `EquationElem`.
- Annotation now unwraps only those transparent style wrappers while looking for the equation token, then reapplies the same styles around the cancelled equation.
- Ordinary deleted text still uses `StrikeElem`; this change only affects tokens whose retained content still contains an equation node.

Tradeoff: the fix stays in token annotation because diffing already retained equation provenance and token content. We avoid guessing from rendered text such as `Ep = m g h`, and we do not change the Typst renderer.

## Text-Empty Layouter Blocks Can Be Opaque Visual Carriers

- Package-generated graphics such as CeTZ canvases may realize as text-empty `BlockElem` carriers whose body is a custom layouter rather than ordinary `Content`.
- These carriers retain concrete render provenance even though they expose no word tokens and no shape/image element in the content tree.
- Opaque fallback diffing now treats text-empty non-`Content` block bodies as visual surfaces when old and new carriers differ.
- General semantic-owner classification does not use this layouter-carrier predicate. Textual wrappers such as `pad` may also lower through custom layouter blocks, but their authored wrapper owners already retain textual patch surfaces and should keep word-level edits.

Tradeoff: CeTZ changes are currently represented as whole opaque old/new visual replacements rather than word-level label edits. This uses retained Typst layout-carrier provenance and avoids renderer changes or source-structure guesses.

## ContainerOps Owns Slot Mapping Decisions

- The global `map_slot_parts` fallback has been removed. `map_container` now
  either uses the selected container's `map_slots` implementation or returns an
  empty mapping.
- Container-specific mappers now choose slot paths and patch surfaces for list,
  enum, terms, table, grid, stack, quote, figure, and wrapper containers.
- The old global helper names for unique partial-item mapping, opaque pre-surface
  grafting, nested-list detection, and unique wrapper recovery were deleted.
- Three bridge behaviors remain active but narrowed and ledgered as
  container-owned debt: FB-007 unique wrapper recovery, FB-008 single realized
  item recovery by unique visible text after span provenance fails, and FB-009
  pre-container patch surfaces for realized carriers with fewer leaf paths than
  source slots.
- Quote owners now claim their text-empty realized carrier directly in block
  owner claim construction. A block quote's visible text lives inside the quote
  body slot even when the layout carrier itself has empty `plain_text`.

Tradeoff: this phase improves ownership boundaries but does not yet deliver the
intended LOC reduction. The remaining FB-007/008/009 bridges are behaviorally
required until Phase 8 retains block/slot/wrapper provenance during extraction,
at which point these container-owned debt paths should be deleted rather than
kept for their tests.
