# Phase B — Design rationale (controller-only notes)

> This document is for the controller/planner, not for implementing subagents.
> Implementing subagents should read only the plan
> ([`docs/superpowers/plans/2026-05-25-phase-b-slot-diff.md`](../plans/2026-05-25-phase-b-slot-diff.md))
> and their per-task briefs. The plan tells them WHAT to build and how to verify it.
> This document records WHY we chose particular designs, so the controller can
> answer review questions and judge edge cases without re-deriving the analysis.

---

## Q1 — Deleted slot children: rebuild the grid

**Problem.** Phase A's `apply_changed_descendants` patches cells in place
with `replace_slot`/`replace_realized_grid_cell`. Deleted children don't
exist in the new container, so there's nowhere to patch — they vanish
from the output (corpus 65: row-deleted table loses its strikethrough row).

**Chosen design.** When the realized container has a grid wrapper
(StyledElem? → BlockElem? → GridElem), **rebuild the grid** from the diff
children in their output order:

| DiffNode status         | Cell body                                              |
|-------------------------|--------------------------------------------------------|
| `Unchanged`             | original cell body                                     |
| `Modified(word_ops)`    | annotated cell (word-level red/green)                  |
| `HasChangedDescendants` | recursively patched cell body                          |
| `Inserted`              | annotated cell with green fill                         |
| `Deleted`               | annotated cell with red strikethrough                  |

The grid's columns/gutters/headers are preserved from the original;
only its cell list is replaced. Deletes are spliced in at their LCS
output position, so a deleted row in a 3-column table renders as 3
adjacent strikethrough cells filling one row.

**Why grid rebuild over cell-by-cell patching:**

- Patching can't insert positions for deletes — there's no "empty slot"
  in the new grid to put them in.
- The diff already produces an ordered sequence of cells (LCS output).
  Walking that sequence once and building cells is simpler than two
  passes (patch existing, append new) plus a separate delete-insertion pass.
- It handles same-shape and different-shape uniformly. The `_same_shape`
  path is just an LCS that happens to produce only Equal/Replace ops.

**Limitations accepted:**

- Header/footer cells in the original grid are preserved as-is. The
  diff's "cells" only replace body cells (`GridChild::Item(GridItem::Cell)`).
  This is fine for our corpus (35/64/65 use plain rows, no headers).
  If we add a corpus with `table.header(...)`, we'd need to extend
  the rebuilder.
- Non-grid containers (figures, footnotes, etc.) fall back to the
  content-matching path (see Q2) — which can't splice deletes back
  but those containers don't have insert/delete semantics in Phase B
  anyway.

## Q2 — Nested slot-bearing containers: multi-level descent at diff time

**Problem.** In corpus 69, outer list item 0's body is a
`SequenceElem([TextElem("Plan release"), ParbreakElem, ListElem(nested)])`.
`SequenceElem` has no `semantic_kind` and no slots, so
`can_recurse_via_slots(old_item0, new_item0)` returns false. Phase A
falls back to a flat word diff on the whole item — losing the per-item
detail in the nested list.

**Why the annotated tree is structurally correct as-is.** The slot system
points at positions in the realized output. In corpus 69, the entire item-0
body (text + parbreak + sublist) lives in a single grid cell. There is no
separate realized position for "just the nested list." The annotated tree
encodes the inner ListElem two levels deep:

```
outer ListElem ─ annotation: { kind: List, slots: [ListItem(0..1)] }
├── child[0]: SequenceElem ─ annotation: { kind: None, slots: [] }     ← item 0 body
│   ├── inner[0]: TextElem("Plan release")
│   ├── inner[1]: ParbreakElem
│   └── inner[2]: ListElem (nested) ─ annotation: { kind: List, slots: [...] }
└── child[1]: TextElem("Ship release")
```

The mapping exists; the diff just has to walk further to find it.

**Chosen design.** Add `find_slot_bearing_descendant_pair(old, new)`:
walks both subtrees in parallel, collecting the **first** slot-bearing
descendant along each branch (stops descending once one is found).
Returns `Some(pair)` only if exactly one descendant exists on each side
AND `can_recurse_via_slots` agrees on the pair. Otherwise `None` → word diff.

In `diff_slot_children_same_shape`, when direct `can_recurse_via_slots`
fails on the slot child pair, try multi-level descent. If it succeeds,
emit a wrapper DiffNode:

```
DiffNode {
  node: outer_slot_child,           // unchanged at this level
  status: HasChangedDescendants,
  children: [
    DiffNode {                      // the inner descent result
      node: inner_slot_container,
      status: HasChangedDescendants,
      children: [slot diff of inner container],
    }
  ],
}
```

**Why this wrapper shape.** `apply_changed_descendants` needs to splice
the patched inner container *back into the outer slot child* — not replace
the entire outer cell with the inner container. The wrapper makes the
hierarchy explicit: the outer cell's content is unchanged structurally
except at the position where `inner_slot_container.realized` lives.

`apply_changed_descendants` then runs the rebuild-grid path on the outer
container. For the outer slot child cell, it calls
`annotate_single_node(wrapper_DiffNode, compact)` → which is
HasChangedDescendants → which recursively calls `apply_changed_descendants`
on the outer slot child. The outer slot child (a `SequenceElem`) has no
grid wrapper, so the rebuild path fails; we fall through to the
**content-matching path**: find `inner.node.realized` inside the outer
slot child's realized content (via `replace_subtree`) and replace it with
the recursively-patched inner content.

**Why "exactly one descendant pair" (vs "first" or "any"):**

- Multiple descendants on either side mean we can't be sure which pair
  the user's edit corresponds to. Word diff is the safer fallback.
- This is conservative; we can relax later if real-world content needs it.

**Why limit search to first slot-bearing per branch (vs collect all):**

- A nested list's children may themselves have semantic_kind=List (slot-bearing).
  We don't want to descend through them and report the inner-inner items.
- Stopping at the first slot-bearing descendant gives the "outermost relevant
  recursable container," which is what we want to recurse INTO (and then it
  recurses further on its own).

**Limitations accepted:**

- If item 0's body contains TWO nested lists, both differ, we fall back to
  word diff. (Could be revisited if a corpus test demands it.)
- If the wrapper-with-inner descent succeeds but the inner diff comes back
  with `Unchanged` everywhere (no real change found), we still emit
  HasChangedDescendants. The cell content will be byte-identical to original.
  Acceptable — no visual harm.

## Q3 — Table tests: tree-level only, not visual

User instruction: corpus tests already validate visuals; the new integration
tests should validate the DiffNode tree shape and per-cell statuses, not
render-and-compare.

For corpus 35 (same-shape body cells + inserted rows), 64 (row inserted),
65 (row deleted): test that the table block is `HasChangedDescendants`,
count Unchanged/Modified/Inserted/Deleted cells, and verify deleted cells
in 65 give the splice-back path actual exercise.

---

## Design ordering / dependencies

Tasks in the plan are ordered so each builds on previous:

1. Clone derives — required for storing AnnotatedContent in DiffNode
2. annotate_realized root fix — required for find_annotated_child to find anything
3. page_styles plumbing — required for correct output grouping (orthogonal but small)
4. Helpers (find_annotated_child, can_recurse_via_slots, find_slot_bearing_descendant_pair) — pure functions, unit-testable
5. diff_slot_children_same_shape with nested descent — uses helpers from 4
6. diff_annotated rewrite — uses #5
7. rebuild_realized_grid_with_cells — pure tree manipulation, unit-testable
8. apply_changed_descendants rewrite — uses #7 and falls back to content matching for nested
9. diff_slot_children_lcs — produces ordered cells with deletes; validated by corpus 19
10. corpus 65 integration test — validates deletes spliced back via #8
11. corpus 69 integration test — validates nested descent via #5
12. corpus 35/64 tests — validate same-shape table cells (35) and LCS path (64)
13. Re-baseline visual PDFs — manual inspection plus update reference files

## Memory aids for review

- The OLD `replace_slot`-based flow is kept for `build_annotated_content`
  (DiffResultFlat path) — don't remove it; the tests using DiffResultFlat
  still exercise it.
- `Annotation::default()`'s footnote field requires FootnoteInfo: Clone
  — Task 1 is load-bearing for *everything* else; the codebase currently
  doesn't even compile because someone added `#[derive(Clone)]` to
  Annotation without giving FootnoteInfo the same.
- For the rebuild path, `BlockElem.body` setter syntax is `body.set(Some(BlockBody::Content(...)))`
  — uses NativeField API not direct assignment.
