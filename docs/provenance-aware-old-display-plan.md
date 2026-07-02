# Provenance-Aware Old Display Pipeline

## Summary

Make diff carry provenance instead of letting annotation infer it. New-side content remains live Typst content. Old-side deleted material is converted during diff into an inert **old display surface** produced from the old realized tree after show rules, then spliced into the new document only as visible red/struck display.

This applies uniformly in running text, nested semantic slots, static page headers/footers, and rendered/contextual header regions.

## Key Changes

- Replace `ContentOrigin` and annotation-time old guessing with explicit edit payload types:
  - new payloads: live `Content`;
  - old payloads: `OldDisplaySurface`;
  - word tokens carry the same distinction, so `Delete` uses old display tokens while `Equal` and `Insert` use new live tokens.
- Keep the existing nested edit model:
  - `EditContent::Nested` continues to patch a new-side `AnnotatedContent` base at semantic slot paths;
  - deleted old children inside nested/list/table/figure-caption edits are inserted before/after anchors as old display payloads;
  - pure deleted blocks may keep old bases for placement/debug, but `WholeBlock` replaces them with old display before rendering.
- Define old display by a retain-list, not a drop-list:
  - retain only visible text primitives, math/raw display primitives, pure visual wrappers, and presentation styles explicitly known not to create labels, state, counters, metadata, page config, refs, links, or context;
  - semantic elements such as headings, figures, labels, metadata, state updates, refs, context expressions, links, and page-style configuration are not retained live;
  - shape-sensitive containers used by nested patching keep only the minimal inert wrapper needed by the parent container, with children recursively converted to old display.
- Extract old display from old `AnnotatedContent.realized`/`effective_render_content`, after Typst realization/show rules have run with the old document's introspector.
- For header-style environments:
  - static header/footer/background/foreground diffs use the same old-display/new-live edit payloads as body text;
  - contextual/running headers that depend on page layout continue using rendered-region frame extraction, but their deleted runs become old display tokens and inserted/equal runs remain new-side display/live as appropriate;
  - old page styles are never spliced into normal content containers.

## Cleanup

- Remove or replace the tactical changes from this chat, including:
  - `ContentOrigin`, `contains_old_dynamic_content`, and `inert_old_content` in annotation;
  - annotation-time old sanitizing and old-base dynamic special cases;
  - the fuzzy deleted-figure-caption line enrichment path;
  - tactical label/ref/state/context tests whose assertions encode the old heuristic model;
  - temporary inspector/example/debug artifacts created during this investigation.
- Keep only orthogonal debug-writer flushing if it remains clean and independently useful.

## Known Problems Covered

- Duplicate old labels/state pollution: old labels, metadata, and state updates are outside the old-display retain-list, so they cannot execute in the annotated document.
- Missing labels such as `<feit-steimle>` in the SFB document: old refs that compiled because of `show ref` are realized in the old world first; the diff retains the show-rule-produced visible text, not the old `RefElem` or old link target.
- New missing refs handled by new `show ref`: new content remains live, so new-side show rules still run in the new universe.
- Corpus #82/header deletion: deleted header content is represented as inert display or rendered-region text, not as old page configuration inside a container.

## Tests

- Add targeted running-text and header-style regressions for:
  - old missing reference compiled by `show ref`, then deleted or replaced, still rendering in the diff without a live old ref;
  - new missing reference handled by new `show ref` remaining live;
  - inserted/new labels and state updates retained and executed;
  - deleted old labels, metadata, refs, links, state updates, and contexts absent from live annotated content;
  - old `context state.get/final`, page counters, `here`, and `query` rendered as inert old display;
  - nested old label/context inside list/table/figure caption inert while visible deletion text remains;
  - static and contextual headers/footers producing the same old-display behavior as body text.
- Run targeted tests first to confirm the pre-clean model fails at least one, then run full `cargo test`, rebuild release, and rerun the shortened SFB document with `--debug`/`--debug-trace`.

## Assumptions

- "Old display" means display content after old-world show rules and realization, not old source content.
- If an old `RefElem`/`ContextElem` survives old realization, it is not allowed into the annotated document; the display builder keeps only already-realized visible text if available and emits a debug event so the case gets a focused regression.
- No renderer changes and no corpus reference updates.
