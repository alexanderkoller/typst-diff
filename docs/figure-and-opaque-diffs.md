# Figure and Opaque Diffs

This note explains the historical figure/caption regression cluster: corpus
`34`, `71`, `72`, `73`, `90`, `91`, and `92`. These cases are not all current
failures. In the maintained 99-case corpus gate, the figure/caption cases are
green: corpus 72 was fixed by retaining the realized `FigureCaption` display
surface, and the current 71/73 outputs were accepted as better references.

The fix is not figure-specific. Figures are the clearest instance of a broader
rule:

> Authored semantic structure owns the diff; realized layout structure is only a
> rendering surface.

## Vocabulary

- **Semantic owner:** the authored element that owns an edit, such as a
  `FigureElem`, table, list, wrapper, or equation.
- **Patch surface:** the `Content` tree that receives edit paths. It may be the
  authored container rather than Typst's realized layout tree.
- **Slot mapping:** the container-owned map from semantic slots to patch-surface
  paths.
- **Realized layout scaffolding:** layout-only nodes such as `tag`, `v`,
  `parbreak`, and wrappers introduced by Typst realization.
- **Ownership noise:** scaffolding or misleading visible text that would claim
  or duplicate a semantic edit if the diff trusted it.
- **Opaque replacement:** a structurally changed, text-empty edit rendered as
  old visual content framed red plus new visual content framed green.

## Figure Slot Contract

Typst 0.14 has `FigureElem` as the caption-bearing container. Tables, images,
raw graphics, SVGs, and shapes become captioned by being placed inside a
figure.

For figures, `FigureOps` supplies the patch surface:

```text
FigureElem
├─ body    path [0]
└─ caption path [1] when present
```

The realized tree may contain body, vertical spacing, caption, tags, and a
paragraph break. Those nodes are layout scaffolding. They do not change the
figure's semantic slot paths.

## Corpus Cluster

`34-figure-with-caption`

The caption text changed in a captioned figure. The old behavior followed
realized child order and applied the caption edit to the `v` spacer. The actual
caption remained present, producing duplicated or misplaced content. The fixed
behavior resolves `FigureCaption` to authored path `[1]`.

`71-figure-caption-added`

The old figure has misleading visible text: its realized block is text-empty.
The new figure's visible text is the caption. Text similarity therefore wants a
whole-block delete plus insert. The fixed behavior pairs the two blocks by
semantic owner key and emits one figure edit:

```text
ReplaceAt [1] Inserted("Distribution of measurements")
```

`72-figure-caption-deleted`

The inverse of `71`. The new figure has only the body slot, so the deleted
caption is anchored after body path `[0]` on the figure patch surface. It is not
a deleted whole figure.

`73-figure-body-changed-caption-added`

The figure body is text-empty but structurally different, and the caption is
added. Slot LCS matches the body slot and inserts the caption slot. The matched
body slot still has to be checked for non-text structural change, producing:

```text
ReplaceAt [0] OpaqueReplacement { old: old_body, new: new_body }
ReplaceAt [1] Inserted(caption)
```

Both edits live in one figure owner block.

`90-opaque-graphic-replaced`

The block has no meaningful text, but the visual structure changed. The old
behavior produced no edit. The fixed behavior emits `OpaqueReplacement`.

`91-raw-svg-graphic-replaced`

Same class as `90`, with raw/SVG visual payload. The diff does not try to
understand SVG geometry; it shows old and new visual payloads side by side as
an opaque replacement.

`92-diagram-caption-and-opaque-body-changed`

Both figure body and caption change. The body is `OpaqueReplacement` at `[0]`;
the caption is a normal word-level `Modified` edit at `[1]`. The caption edit is
not applied to realized spacer/scaffolding.

## General Rule

When realized surface structure and semantic structure compete:

1. Pair semantic owners first.
2. Ask the owner for a patch surface.
3. Resolve slot paths against that patch surface.
4. Apply textual edits where text exists.
5. Use `OpaqueReplacement` when content changed structurally but has no textual
   diff surface.

This applies beyond figures. Any wrapper or container whose realized output is
opaque, text-empty, or scaffolded should go through the same owner and patch
surface contracts rather than relying on nth realized child order.

## Guard Tests

The integration tests assert the abstraction directly:

- figure caption paths are `[1]`, not realized spacer paths;
- caption add/delete produce one figure owner edit block;
- text-empty structural changes produce `OpaqueReplacement`;
- opaque body plus caption changes remain in one figure owner block;
- ownership noise does not leave both a plain new copy and a patched copy.
