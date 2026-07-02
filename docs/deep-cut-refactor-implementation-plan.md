# Deep-Cut Refactor Implementation Plan

## Purpose

This plan merges the invariant-driven cleanup plan, the deeper generalization
plan for reducing `typst-diff`'s production code size, and the fallback-debt
plan. The goal is not to win a line-count contest by moving complexity around.
The goal is to delete redesign remnants by making the core pipeline carry the
information it already had earlier: provenance, ownership, style context,
patch surfaces, and diffable regions.

Current rough scale:

- `src/` is about 17.9k lines including unit tests.
- Production `src/` is about 14.3k lines excluding unit-test modules.
- The largest simplification targets are `src/diff.rs`,
  `src/container_ops.rs`, `src/annotate.rs`, and `src/annotated.rs`.

The desired direction is below 10k production lines if the new abstractions pay
for themselves. A final result closer to 11k-12k is acceptable if the remaining
code is well tested and the deleted paths are real fallbacks rather than useful
behavior.

## Non-Negotiable Constraints

- Do not change the Typst renderer. Fix provenance, diff construction, and
  annotation inputs instead.
- Do not update corpus references. The user updates reference PNGs manually.
- Preserve the public CLI and library behavior unless a later explicit product
  decision changes it.
- Prefer retained provenance over post-hoc guessing from visible text,
  rendered coordinates, or unique descendants.
- Avoid new broad fallbacks. Unsupported cases should become explicit typed
  diagnostics or documented limitations.
- When the program must still guess, emit a default-on fallback warning. The
  warning may be suppressed with `--quiet`, but it must still be recorded in
  `--debug` and/or `--debug-trace`.
- Keep decision metadata extremely lightweight: use small enums, static codes,
  owner/path IDs, bounded previews, and counters. Do not store cloned Typst
  `Content` just to explain a decision.
- Maintain a fallback debt ledger. Any new fallback, heuristic, or post-hoc
  guess must either be eliminated immediately or added to the ledger with a
  warning code, debug/trace coverage, tests, and removal criteria.
- After every major implementation phase, update `TECHNICAL-DECISIONS.md` with
  the technical decision, justification, and tradeoff.
- If a bug investigation takes more than two minutes, isolate it into a
  minimal failing test before continuing with the fix.

## Success Criteria

- All unit and integration tests that pass before the refactor still pass.
- Every corpus case that passes before the refactor still passes after every
  major refactor phase.
- `plain_text()` remains available for human-readable logs and similarity, but
  is no longer used as semantic ownership or structural identity.
- Patch surfaces are explicit, named, and justified by invariants.
- Duplicate edit suppression is structural, not based on rendered or visible
  text signatures.
- Fallback warnings are default-on at the CLI, suppressible with `--quiet`, and
  visible in debug artifacts.
- Every fallback warning code has a corresponding fallback-debt ledger entry,
  test coverage, and explicit removal criteria.
- Equation, footnote, context, wrapper, container, page-style, and rendered
  region behavior uses retained provenance where available.
- The cleaned architecture is understandable from the module boundaries and
  docs without reading historical redesign notes first.

## Fallback Policy

Fallbacks are not normal control flow. They are temporary debt markers for
places where the current generalization lacks enough provenance or structural
information to make a clean decision.

Implementation rules:

- A fallback must have a stable warning code, such as
  `FB-001-positional-sequence-pairing`.
- A fallback must emit a warning by default when it is exercised in normal CLI
  execution.
- `--quiet` suppresses fallback warnings on stderr only. It does not suppress
  hard errors, modification logs, `--debug`, or `--debug-trace`.
- `--debug` records aggregate fallback counts and a bounded number of examples
  in the debug bundle.
- `--debug-trace` records each fallback decision as a JSONL event.
- A fallback warning should identify the pipeline phase, warning code, short
  explanation, and bounded context such as owner ID, path, block index, or text
  preview. It must not retain large `Content` trees for diagnostics.
- A fallback is acceptable only when the alternative would be worse: silent
  data loss, misleading structural annotation, panic, or a known unsupported
  case. Whole-surface opaque replacement is acceptable when it is an explicit
  `OpaqueVisual` or unsupported-structured-surface decision, not a hidden
  rescue path.

## Fallback Debt Ledger

Add `docs/fallback-debt-ledger.md` as the human ledger for known fallback debt.
It is not a substitute for tests or runtime warnings; it is the map that keeps
the warning codes and removal work organized.

Each ledger entry must contain:

- Stable ID and warning code.
- Status: `active`, `instrumented`, `partially-replaced`, or `removed`.
- Current source sites.
- Why the behavior is a guess rather than a proven decision.
- User-visible risk.
- Runtime warning behavior.
- Debug/debug-trace event names.
- Tests that exercise the fallback.
- Replacement abstraction.
- Removal criteria.

Seed entries:

- Positional sequence pairing.
- Context visible-text pairing.
- Visible-text owner/block matching.
- Unique changed slot pair.
- Slot-bearing descendant pair.
- Duplicate edit pruning by text signature.
- Unique wrapper/body recovery by visible text.
- Unique partial item container mapping.
- Anonymous opaque pre-surface grafting.
- Word-diff-or-opaque replacement ladder.
- Broad empty-block equation carrier recognition.
- Footnote marker matching by visible number.
- Rendered-region source-string `align(...)` parsing.
- Generated Typst snippet panic path.

Update `AGENTS.md` with this rule:

- Any new fallback, heuristic, or post-hoc guess must either be eliminated
  immediately or added to `docs/fallback-debt-ledger.md` with a warning code,
  debug/trace emission, tests, and removal criteria.

Add a lightweight audit script or test, such as
`tests/check_fallback_ledger.sh`, that verifies:

- Every `FallbackCode` has a ledger entry.
- Every active ledger entry references a warning code.
- New semantic fallback terms in `src/` are either tied to a ledger ID or
  allowlisted as non-semantic, such as font fallback.

## Phase 0: Baseline And Inventory

Run this before functional refactoring. The point is to know which behavior is
currently protected, not to make the tree clean.

Commands:

```bash
cargo check --all-targets
cargo test --all-targets
bash tests/run_corpus.sh
```

Inventory to record in the implementation notes:

- Current `cargo check` result.
- Current `cargo test --all-targets` result.
- Current corpus pass/fail/new/skip counts.
- Current production line count, excluding unit-test modules.
- Any pre-existing dirty or untracked files that are unrelated to the refactor.

Exit criteria:

- The intended baseline is explicit.
- Existing failures are listed and not accidentally treated as refactor
  regressions.

## Phase 1: Add The Passing-Corpus Regression Gate

This is the test the user requested: every corpus test that passes now must
continue to pass after the refactor.

### Harness Changes

Extend `tests/run_corpus.sh` without changing its default behavior:

- Add `--exact NAME` to run exactly one corpus directory by basename. This
  avoids substring ambiguity from `--filter`.
- Add `--write-passing-list PATH` to write, at the end of a normal run, the
  sorted names whose final status is exactly `PASS`.
- Add `--require-passing-list PATH` to run only the listed names and fail if
  any listed test is missing, skipped, new, or failed.
- Keep `--update-refs` behavior unchanged and never enable it from the new
  gate.

Add `tests/run_passing_corpus.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec bash "$SCRIPT_DIR/run_corpus.sh" \
  --require-passing-list "$SCRIPT_DIR/corpus-passing-baseline.txt" \
  "$@"
```

Add `tests/corpus-passing-baseline.txt`:

- One corpus directory name per line.
- Blank lines and `#` comments are allowed.
- Generated once from the pre-refactor intended baseline.
- Updated only by deliberate user-approved baseline changes, not as part of
  ordinary refactor work.

Baseline recording command:

```bash
bash tests/run_corpus.sh --write-passing-list tests/corpus-passing-baseline.txt
```

The command may exit nonzero if some corpus cases currently fail; it must still
write the passing list before exiting.

Regression command after the baseline exists:

```bash
bash tests/run_passing_corpus.sh --no-build
```

Tests for the harness:

- `bash tests/run_corpus.sh --list` still prints all names.
- `bash tests/run_corpus.sh --exact 01-no-change --no-build` runs one case.
- `bash tests/run_passing_corpus.sh --no-build` fails if the baseline contains
  a nonexistent corpus name.
- `--write-passing-list` excludes `FAIL`, `NEW`, and `SKIP`.

Exit criteria:

- The baseline file is committed.
- The passing-corpus gate passes on the baseline code.
- This gate is run after every major phase below.

## Target Architecture

The refactor should introduce these internal modules. They are `pub(crate)`
unless an existing public API requires otherwise.

### `decision`

Single lightweight model for proof, fallback debt, and diagnostic warning
events.

Required types:

- `FallbackCode`: a small enum or static code table, ideally representable as
  `u16`, with stable string labels for user/debug output.
- `DecisionProof`: a small enum for proven decisions, such as `ExactPath`,
  `RecordedContext`, `SemanticOwner`, `ContainerSlot`, `StyleContext`,
  `RenderedTag`, `OpaqueVisualCarrier`, and `Unsupported`.
- `DecisionEvent`: a borrowed or compact event carrying phase, proof or
  fallback code, owner/path IDs, block or region index, and bounded previews.
- `DecisionSink` or `WarningSink`: a streaming sink used by CLI, debug, and
  trace paths.

Memory rules:

- Do not clone or retain full `Content` trees in decision metadata.
- Prefer static strings, small enums, indices, and bounded previews.
- Aggregate counts for `--debug`; emit individual events only when
  `--debug-trace` is enabled.
- Warnings should be emitted at the decision point, not reconstructed later by
  re-walking the document.

CLI/debug behavior:

- Add `--quiet` to suppress default stderr fallback warnings.
- Default execution prints compact fallback warnings to stderr.
- `--debug` writes `diff/fallback-warnings.yml` with counts and bounded
  examples.
- `--debug-trace` writes individual decision/fallback events into the existing
  `diff/pipeline-events.jsonl` stream.

### `content_tree`

Single owner for content-tree traversal and mutation.

Responsibilities:

- Child extraction for realized and semantic content.
- Style-aware materialization before reading children.
- Path lookup and path replacement.
- Insertion before/after/append at a content path.
- Transparent wrapper descent.
- Search for known wrapper descendants when Typst inserts scaffolding.

This module should absorb duplicated helpers such as:

- `realized_child_contents`
- `content_at_path`
- `replace_content_path`
- `replace_content_at_path`
- `maybe_map_realized_children`
- `map_transparent_children`

### `content_key`

Single API for content comparison keys, with the purpose named at the call
site.

Required key purposes:

- Block equality for LCS.
- Presentation key for human-facing output.
- Context key for context output reattachment.
- Slot matching key for child edit scripts.
- Visible similarity key for replacement pairing.
- Opaque structural signature for visual surfaces.

Rules:

- `plain_text()` may feed visible similarity and logs.
- `plain_text()` must not be used directly as ownership or identity.
- Ambiguous or unsupported identity must be represented explicitly rather than
  silently downgraded to visible text equality.

### `style_context`

Single owner for style partitioning and propagation.

Responsibilities:

- Split page styles from non-page styles.
- Maintain sticky page-style state for extracted blocks.
- Preserve old display styling while placing inert old deletions under the new
  page universe.
- Extract semantic page regions from page styles.
- Strip or retain page styles from edit payloads according to provenance.

This should absorb repeated page-style helpers currently spread through diff,
annotation, and evaluation code.

### `patch_surface`

Explicit model for edit surfaces that are not simply the realized node.

Required variants:

- `Realized`: edit the Typst-realized content.
- `PreContainer`: edit the authored semantic container.
- `GraftedBlockBody`: edit a wrapper body grafted into realized block
  scaffolding.
- `LayoutPreservingSequence`: edit through a sequence that preserves paragraph,
  list, or parbreak boundaries.
- `OpaqueVisual`: whole-surface replacement for real visual carriers.

Every patch surface must record the invariant that made it legal. Avoid
anonymous `Option<Content>` patch surfaces in new code.

### `diff_surface`

Single abstraction for how a diffable area tokenizes and renders.

Required surface kinds:

- Word tokens.
- Raw/code lines.
- Rendered page-region text.
- Equation tokens.
- Structured container regions.
- Opaque visual surfaces.

This should fold together ordinary modified fragments, raw block diffing,
rendered-region word ops, equation-origin token handling, and whole visual
replacement decisions.

### `diff_area`

Unified model for independently diffable document areas.

Required area kinds:

- Body block.
- Semantic page region.
- Rendered page region.
- Structured container region.

Each area carries:

- Base content or rendered text.
- Old/new provenance.
- Active style context.
- Owner ID.
- Patch surface.
- Surface kind.

### `attributed_block_stream`

Build one stream from `AnnotatedContent` before block matching.

Each attributed block carries:

- Realized content.
- Effective edit surface.
- Page style context.
- Semantic owner ID and path.
- Semantic kind.
- Slot labels and paths.
- Equation origins.
- Footnote marker/body provenance.
- Patch-surface variant.
- Opaque visual carrier classification.

This stream replaces post-hoc ownership recovery and sparse cursors.

### `edit_script`

Single child/slot edit-script builder.

Responsibilities:

- Same-shape slot diffing.
- LCS slot diffing.
- Child-sequence structural diffing.
- Recursive nested edits.
- Insert/delete/replace operations against explicit patch surfaces.

This should replace "exactly one changed descendant" locality rules.

## Phase 2: Add Warning And Ledger Infrastructure

Goal: make existing fallback debt visible before removing it. This phase should
not try to solve the fallbacks yet; it should make them observable and
auditable.

Implementation:

- Add `src/decision.rs` or equivalent lightweight infrastructure for
  `FallbackCode`, `DecisionProof`, compact `DecisionEvent`, and a streaming
  warning/decision sink.
- Add CLI `--quiet`. It suppresses fallback warnings on stderr only.
- Thread the sink through the existing CLI pipeline alongside the current debug
  event sink, keeping call-site plumbing lightweight.
- Add default stderr fallback warnings. Use one compact line per warning, plus
  a final count summary if there are many repeated warnings.
- Add `diff/fallback-warnings.yml` to the debug bundle with warning counts,
  first bounded examples, and links to trace event names when present.
- Add `--debug-trace` JSONL events for each fallback decision in the existing
  pipeline-events stream.
- Add `docs/fallback-debt-ledger.md` and seed it with the known active fallback
  mechanisms listed above.
- Update `AGENTS.md` with the fallback-ledger maintenance rule.
- Add `tests/check_fallback_ledger.sh` or an equivalent test that ensures every
  `FallbackCode` has a ledger entry and new semantic fallback terms are
  ledgered or allowlisted.

Initial instrumentation targets:

- Positional sequence pairing and context visible-text pairing.
- Visible-text owner/block matching.
- Unique changed slot pair and slot-bearing descendant pair.
- Duplicate edit pruning by text signature.
- Unique wrapper/body recovery and unique partial item container mapping.
- Opaque pre-surface grafting.
- Word-diff-or-opaque replacement selection.
- Broad empty-block equation carrier recognition.
- Footnote marker matching by visible number.
- Rendered-region source-string wrapper parsing and generated snippet
  construction.

Tests:

- CLI emits fallback warnings by default when an instrumented fallback is
  exercised.
- `--quiet` suppresses stderr fallback warnings but does not suppress hard
  errors, modification logs, `--debug`, or `--debug-trace`.
- `--debug` writes `diff/fallback-warnings.yml`.
- `--debug-trace` writes individual fallback events into pipeline JSONL.
- The ledger audit fails when a new `FallbackCode` lacks a ledger entry.

Commands:

```bash
cargo test --all-targets fallback
cargo test --all-targets debug
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- Existing fallback behavior is observable in ordinary CLI output unless
  `--quiet` is set.
- Debug and trace artifacts contain enough information to diagnose where the
  fallback fired without retaining large content trees.
- The fallback debt ledger and audit gate are in place.
- `TECHNICAL-DECISIONS.md` records the default-warning policy and lightweight
  decision metadata contract.

## Phase 3: Extract `content_tree` And `style_context`

Goal: move duplicated mechanics out of large files without behavior changes.

Implementation:

- Add `src/content_tree.rs` and move child extraction, path lookup, path
  replacement, style-aware materialization, and transparent descent there.
- Add `src/style_context.rs` and move page-style partitioning, sticky page-style
  propagation, and edit payload page-style handling there.
- Update `src/lib.rs` with private module declarations.
- Keep existing call sites semantically identical.
- Do not introduce new matching behavior in this phase.

Expected deletions:

- Local path replacement helpers in `src/annotated.rs`.
- Local path replacement helpers in `src/container_ops.rs`.
- Page-style helpers in `src/diff.rs` and related annotation code.

Tests:

```bash
cargo check --all-targets
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Focused tests to add or verify:

- Every emitted `SemanticSlot.path` resolves through `AnnotatedContent`.
- Every emitted `patch_path` resolves through the selected patch surface.
- Page styles remain separate from non-page styles.
- Inert old deletions inherit the new page context.

Exit criteria:

- Behavior is unchanged.
- The duplicated traversal/style helpers are gone or reduced to thin wrappers.
- `TECHNICAL-DECISIONS.md` records the new ownership of traversal and style
  mechanics.

## Phase 4: Introduce `content_key`

Goal: make equality, similarity, and identity explicit.

Implementation:

- Add `src/content_key.rs`.
- Define a small enum or set of constructors for the required key purposes:
  block equality, presentation, context, slot matching, visible similarity, and
  opaque signature.
- Replace scattered helpers such as `presentation_key`,
  `context_presentation_key`, `structural_child_key`,
  `slot_child_match_key`, `normalized_visible_text`, and `HashableContent`
  with named `ContentKey` calls.
- Search for raw `plain_text()` in ownership, block pairing, slot pairing, and
  duplicate suppression paths. Replace each ownership use with a key or an
  explicit unresolved state.
- For any identity decision that still cannot be proven, emit an existing
  fallback warning code and keep the ledger entry active instead of silently
  using visible text.
- Keep `plain_text()` in tests, debug summaries, and human-readable logs where
  it is only observation.

Tests:

- Same visible text with changed link target.
- Same visible text with changed label/reference target.
- Repeated same-text table/list/wrapper items where one occurrence changes.
- Existing equation-number reference tests.
- Negative tests where visible text equality must not imply identity.

Commands:

```bash
cargo test --all-targets content_key
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- All non-log identity uses go through `ContentKey`.
- Ambiguous identity is explicit and test-covered.
- The visible-text identity fallback entries are either removed from the ledger
  or remain instrumented with warnings and concrete removal criteria.
- `TECHNICAL-DECISIONS.md` documents the distinction between identity keys and
  similarity keys.

## Phase 5: Replace Anonymous Patch Surfaces

Goal: make every non-realized edit surface explicit and auditable.

Implementation:

- Add `src/patch_surface.rs`.
- Replace `Annotation.patch_surface: Option<Content>` with a typed internal
  patch-surface value. If changing the public struct shape is too disruptive,
  add a new internal field first and remove the old field after migration.
- Update `container_ops` and annotation construction to return named patch
  variants rather than raw content.
- Move leading/parbreak/list-boundary preservation into a general
  `LayoutPreservingSequence` rule.
- Make unsupported patch operations return typed diagnostics used in tests
  rather than silently returning `None`.
- Retire warning codes for anonymous opaque pre-surface grafting and
  nested-list-specific layout repair once the typed patch-surface variants
  explain those cases.

Expected deletions:

- Nested-list-specific parbreak repair paths.
- Generic fallback patching that does not explain which invariant it preserves.
- Repeated local "try realized, then patch, then children" edit application.

Tests:

- Paragraph/list boundary preservation.
- Nested list insertion/deletion.
- Wrapper body edits.
- Opaque realization where semantic slots outnumber realized children.
- Cases where realized content should remain the edit surface.
- Unsupported patch target reports a typed diagnostic in tests.

Commands:

```bash
cargo test --all-targets patch
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- New code no longer passes anonymous `Option<Content>` patch surfaces around.
- Each patch variant has a narrow contract and tests.
- Patch-surface fallbacks that remain are explicitly warned and ledgered; no
  anonymous patch rescue path remains.
- `TECHNICAL-DECISIONS.md` records the patch-surface contract.

## Phase 6: Build `diff_surface` And `diff_area`

Goal: collapse the separate diff paths for text, raw blocks, equations,
containers, opaque visuals, semantic page regions, and rendered page regions.

Implementation:

- Add `src/diff_surface.rs` for tokenization and rendering contracts.
- Add `src/diff_area.rs` for body blocks, semantic page regions, rendered page
  regions, and structured container regions.
- Make each area select one surface kind.
- Route ordinary word diffs, raw/code line diffs, equation-token diffs, and
  opaque visual replacements through the same `DiffSurface` result model.
- Keep rendered page-region extraction behavior equivalent at first; only move
  it behind the shared surface contract in this phase.
- Replace the "word diff or opaque fallback" ladder with explicit surface
  selection. If no surface can be selected cleanly, emit an unsupported-surface
  warning rather than guessing.

Expected deletions:

- Separate raw-block modified-content path.
- Equation-specific token replacement plumbing outside surface selection.
- Rendered-region word-op duplication.
- Whole-surface visual replacement branches that duplicate ordinary replace
  logic.

Tests:

- Raw/code block line edits.
- Inline and display equation changes.
- Deleted equation tokens preserve math structure through styles.
- Opaque graphic replacements.
- Page header/footer text diffs.

Commands:

```bash
cargo test --all-targets equation
cargo test --all-targets rendered
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- Adding a new diffable surface would not require a parallel diff pipeline.
- Rendered page-region behavior is still preserved by tests and corpus.
- Whole-surface opaque replacement is a chosen surface kind, not a catch-all
  fallback.
- `TECHNICAL-DECISIONS.md` records the surface/area contract.

## Phase 7: Add `attributed_block_stream`

Goal: carry semantic ownership forward instead of rediscovering it after block
matching.

Implementation:

- Add `src/attributed_block_stream.rs`.
- Build the stream from the annotated tree before block LCS.
- Each stream item carries realized content, effective edit surface,
  `StyleContext`, owner ID, owner path, semantic kind, slot metadata, equation
  origins, footnote provenance, and patch-surface variant.
- Replace `BlockOwnerCursor` and `EquationOriginBlockCursor` with stream item
  fields.
- Replace `find_annotated_block_owner` style lookup with owner IDs from the
  stream.
- Keep old recovery code behind tests only until equivalent stream behavior is
  proven, then delete it.
- Retire warning codes for visible-text owner matching, deferred owner handoff,
  and sparse equation-origin cursor recovery as the stream takes ownership of
  those decisions.

Expected deletions:

- `BlockOwnerCursor`.
- `EquationOriginBlockCursor`.
- Deferred owner handoff code.
- Effective-render duplicate suppression keyed by visible text.
- Owner lookup based on "the next matching block".

Tests:

- Nested container inside a table cell remains localized.
- Two changed nested containers in the same outer block both produce edits.
- Repeated macro containers with one edit select the correct owner.
- Empty equation shells stay live.
- Opaque visual owners claim only their own realized carrier.

Commands:

```bash
cargo test --all-targets owner
cargo test --all-targets equation
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- Block edits use stream ownership, not post-hoc owner search.
- Duplicate prevention is tied to owner IDs.
- Owner-related fallback warnings either disappear or point to explicit
  unsupported provenance gaps in the ledger.
- `TECHNICAL-DECISIONS.md` records the attributed block stream invariant.

## Phase 8: Unify Slot And Child Edit Scripts

Goal: replace several local child-diff strategies with one edit-script builder.

Implementation:

- Add `src/edit_script.rs`.
- Make `ContainerOps` expose stable children/slots as edit-script inputs.
- Use one builder for same-shape slots, LCS slots, child-sequence diffs, and
  recursive nested edits.
- Remove `find_unique_changed_slot_pair` and
  `find_slot_bearing_descendant_pair`.
- Remove generic leaf-block zipping for structured containers when stable
  slots exist.
- Make table/grid row and cell changes use explicit addressing rather than
  falling back to flat word diffs.
- Retire warning codes for unique changed slot pairs, slot-bearing descendant
  pairs, exactly-one changed child rules, and duplicate pruning by text
  signature.

Expected deletions:

- Unique-descendant locality rules.
- Duplicated same-shape versus LCS slot diffing.
- Generic leaf child path collection for containers with known semantics.
- Silent failed insert/replace attempts.

Tests:

- List, enum, terms, figure, footnote, quote, wrapper mappings.
- Table/grid row insertion and deletion.
- Table/grid cell insertion, deletion, and replacement.
- Table/grid header/footer changes if supported; otherwise typed unsupported
  diagnostics.
- Low-similarity container replacement remains structural.
- Repeated identical cells with one changed cell.

Commands:

```bash
cargo test --all-targets container
cargo test --all-targets table
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- No diff path depends on "exactly one changed descendant" to find locality.
- Structured containers preserve structure on shape changes.
- Repeated identical edits are preserved by owner IDs, not removed by broad
  pruning.
- `TECHNICAL-DECISIONS.md` records the edit-script contract.

## Phase 9: Normalize Footnotes And Equations Into Provenance

Goal: make footnotes and equations ordinary users of the provenance model.

Implementation:

- Represent equation origins on attributed stream items and `DiffSurface`
  equation surfaces.
- Treat display equation empty shells as live semantic carriers only when
  stream provenance says they are equation carriers.
- Represent footnote marker and body provenance explicitly on attributed stream
  items or areas.
- Remove broad marker matching by visible number.
- Remove broad "empty block adjacent to equation" matching.
- Retire warning codes for broad empty-block equation carrier recognition and
  visible-number footnote marker matching.

Tests:

- Empty blocks adjacent to equations do not consume equation origins.
- Multiple equations in one paragraph retain correct tokenization.
- Display equations with numbering stay live and styled.
- Visible `1` before a footnote marker does not receive footnote body metadata.
- Added, changed, and deleted footnotes near existing footnotes remain
  localized.

Commands:

```bash
cargo test --all-targets footnote
cargo test --all-targets equation
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- Equation and footnote handling no longer needs broad post-hoc recognition.
- Existing corpus cases 22, 23, 37, 38, 39, 50, 74, and 101 remain passing if
  they were in the baseline.
- Remaining equation/footnote warnings indicate real missing provenance, not
  ordinary expected control flow.
- `TECHNICAL-DECISIONS.md` records remaining limitations.

## Phase 10: Generalize Page Regions

Goal: bring semantic and rendered page regions under the same area/surface
model as body content.

Implementation:

- Represent page headers, footers, background/watermark changes, and rendered
  text regions as `DiffArea` values.
- Prefer content-tree wrapper analysis over source-string scanning for
  `align(...)` and related constructs.
- If generated Typst source snippets remain temporarily, make construction
  return `Result<Content>` and test that special characters cannot panic.
- Represent inserted/deleted page-region instances explicitly for pages present
  only on one side.
- Remove unrelated coordinate-band logic where tagged rendered text can provide
  provenance.
- Retire warning codes for source-string `align(...)` parsing and generated
  Typst snippet panic paths.

Expected deletions:

- Source-string `align(...)` parsing.
- Generated Typst snippet `expect`/panic paths.
- Rendered-region-only diff result shapes that duplicate body edit logic.

Tests:

- Contextual total-page footer.
- Alternating headers.
- Inserted pages with new header/footer text.
- Aligned footer/header helper functions.
- Header/footer content containing brackets, hashes, backslashes, quotes, and
  non-ASCII text cannot panic.

Commands:

```bash
cargo test --all-targets rendered
cargo test --all-targets page
cargo test --all-targets
bash tests/run_passing_corpus.sh --no-build
```

Exit criteria:

- Page-region logic is no longer an unrelated second pipeline except where
  layout context truly requires rendered extraction.
- Existing corpus cases 32, 41, 42, 43, 80, 81, 82, 83, 84, and 85 remain
  passing if they were in the baseline.
- Rendered-region fallbacks that remain are warned and ledgered as layout
  provenance gaps.
- `TECHNICAL-DECISIONS.md` records the semantic versus rendered page-region
  contract.

## Phase 11: Delete Legacy Paths And Consolidate Docs

Goal: remove code that is now unreachable or redundant.

Implementation:

- Delete obsolete fallback helpers after tests prove replacement behavior:
  owner cursors, equation cursors, unique-descendant search, duplicate text
  pruning, anonymous patch-surface fallback, and direct identity uses of
  `plain_text()`.
- Remove or mark ledger entries as `removed` only when their warning code no
  longer fires in targeted tests and the replacement invariant is documented.
- Re-run line counts and search for removed symbol names.
- Update `docs/system-walkthrough-annotated-tree.md`,
  `docs/code-consistency-review.md`, and stale sections of `docs/technical.md`.
- Update `docs/fallback-debt-ledger.md` so active entries match active warning
  codes exactly.
- Mark older plans as historical or superseded where appropriate. Do not delete
  useful diagnosis notes unless the user asks.
- Make module-level comments describe the current pipeline.

Searches:

```bash
rg "BlockOwnerCursor|EquationOriginBlockCursor|find_unique_changed_slot_pair|find_slot_bearing_descendant_pair|prune_duplicate_empty_container_edits" src docs tests
rg "plain_text\\(\\)" src
rg "patch_surface: Option<Content>|Option<Content>.*patch" src
rg "fallback|fall back|guess|heuristic|unique changed" src
```

Final commands:

```bash
cargo check --all-targets
cargo test --all-targets
bash tests/check_fallback_ledger.sh
bash tests/run_passing_corpus.sh
bash tests/run_corpus.sh --only-failures
```

Exit criteria:

- The passing-corpus gate passes.
- All unit/integration tests pass or pre-existing failures are documented.
- The final full corpus run has no regression among baseline-passing tests.
- The fallback-ledger audit passes.
- All remaining active fallback warnings have ledger entries, tests, and
  removal criteria.
- Production line count is recorded.
- `TECHNICAL-DECISIONS.md` summarizes the final architecture and tradeoffs.

## Review Checklist For Every Phase

- Did the phase delete or shrink at least one fallback, duplicate helper, or
  post-hoc guess?
- If a fallback remains, does it emit a warning by default, appear in debug
  artifacts, and have an active ledger entry?
- Did it add a general invariant or just rename code?
- Are new APIs narrow enough to prevent ad hoc call-site decisions?
- Are unsupported cases explicit rather than silently ignored?
- Are tests added before or alongside behavior changes?
- Does the passing-corpus baseline still pass?
- Does `tests/check_fallback_ledger.sh` pass?
- Were docs and `TECHNICAL-DECISIONS.md` updated after a major technical
  decision?

## Expected Deep Cuts

High-confidence deletion targets:

- Owner recovery machinery in `src/diff.rs`.
- Equation-origin cursor machinery in `src/diff.rs`.
- Unique-descendant and slot-bearing-descendant search.
- Duplicate empty-container edit pruning by visible text.
- Unwarned semantic fallback paths and fallback-shaped guesses.
- Repeated content path lookup/replacement helpers.
- Repeated page-style partitioning and sticky propagation helpers.
- Raw/code/equation/rendered-region diff logic that duplicates ordinary
  surface diffing.
- Nested-list and wrapper patch fallbacks replaced by explicit patch surfaces.

Medium-confidence deletion targets:

- Some container-specific fallback mapping after `edit_script` and
  `PatchSurface` exist.
- Some rendered page-region coordinate-band logic after tagged rendered text is
  reliable.
- Some docs describing historical span-restoration and content-slot behavior.

Do not cut:

- Tests that protect retained behavior.
- Debug output that is still needed to diagnose provenance failures.
- Fallback warnings and ledger entries for debt that has not yet been removed.
- Conservative opaque visual replacement behavior for genuinely opaque
  Typst/package render surfaces.
- Renderer boundary code.
