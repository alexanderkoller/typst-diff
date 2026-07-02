# General Show-Rule-Aware Rendered Text Diffing

## Summary

- Keep `eval_to_realized_content` as the primary post-show-rule path.
- Add a general rendered-text side channel for semantic owners/slots whose visible text exists only after final layout.
- Use provenance tags around diffable owners/slots before layout, then collect rendered frame text by tag stack.
- Use this for corpus 89 so `Figure -> Exhibit` and `Measurements -> Updated measurements` are visible as edits, without a caption-specific pass or punctuation/spacing heuristics.

## Key Changes

- Introduce a `RenderedSemanticText` map keyed by semantic owner identity plus optional slot path.
  - Populate it by wrapping diffable semantic nodes/slots with internal tags before layout.
  - Extract text from laid-out frames under those tags.
  - Do not match by string uniqueness, coordinates, or caption-specific structure.

- Generalize slot diff surfaces into two concepts:
  - **patch surface**: where edits are grafted back, preserving existing stable paths such as `FigureBody -> [0]` and `FigureCaption -> [1]`.
  - **text diff surface**: the best available text for computing word ops, preferring realized content and using tagged rendered text when realized content is incomplete/lossy.

- For rendered-text-backed edits:
  - Diff the old/new tagged rendered strings.
  - Render the modified word ops as inline annotated content.
  - Graft that inline content at the existing slot/owner path, so figure body structure remains intact while the caption slot can display `Figure` deleted and `Exhibit` inserted.
  - Avoid changing the renderer and avoid local spacing fixes.

- Keep existing realized-content behavior for cases that already work, such as heading show rules where the prefix survives in realized content.

## Test Plan

- Add or extend corpus 89 integration coverage:
  - modification log contains deleted `Figure` and inserted `Exhibit`;
  - deleted text contains `Measurements`;
  - inserted text contains `Updated measurements`;
  - logs do not contain `MeasurementsFigure` or `measurementsFigure`;
  - annotated rendered plain text keeps deleted and inserted caption strings separated.

- Add a general show-rule rendered-text test using a non-caption owner, such as heading numbering/prefix changes, to prove the mechanism is not caption-specific.

- Preserve figure slot path tests:
  - figure body path remains `[0]`;
  - figure caption path remains `[1]`;
  - realized layout scaffolding does not become part of the public patch path.

- Run:
  - `cargo test figure_caption`
  - targeted corpus 89 test
  - targeted show-rule tests
  - `bash tests/run_corpus.sh --filter figure`
  - inspect debug output with `--debug` or `--debug-trace` for 34, 63, 71, 72, 73, 89, 92, and 100.

## Assumptions

- This is intentionally broader than the earlier caption pass.
- If a rendered text segment cannot be attributed through internal tags, it is not used.
- No source show-rule parsing, renderer changes, punctuation heuristics, or caption-only fallback should be introduced.
