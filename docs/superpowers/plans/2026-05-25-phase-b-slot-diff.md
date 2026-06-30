# Phase B: Slot-level Diff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable `diff_annotated` to recurse into list/table/container slots so that per-item statuses (`Unchanged`, `Modified`, `Inserted`, `Deleted`, `HasChangedDescendants`) are produced instead of flat `Modified` on the whole block. Additionally:

- **Deletes inside containers** are spliced back into the realized grid as red strikethrough cells (corpus 65: a deleted table row appears as 3 strikethrough cells in its original position).
- **Nested slot-bearing containers** inside non-slot-bearing slot children are reached via multi-level descent (corpus 69: an inner list inside an outer list item's `SequenceElem` body is diff'd per-item).
- **Tables** use the same code path as lists (both realize to `BlockElem(GridElem(cells))`); per-cell statuses are exercised by new integration tests for corpus 35, 64, 65.

**Architecture.** Four coordinated changes:
1. Fix `annotate_realized` root-level mismatch so block-level annotated nodes actually carry semantic annotations and children.
2. Rewrite `diff_annotated` to operate at `BlockOp` level and call slot recursion for same-shape and different-shape containers.
3. Add **multi-level descent** in `diff_slot_children_same_shape` so a slot child without `semantic_kind` can still recurse into a unique slot-bearing descendant.
4. Rewrite `apply_changed_descendants` to **rebuild** the realized grid from the diff children (preserving columns/gutters), placing Unchanged/Modified/HasChangedDescendants/Inserted/Deleted in their LCS output order. Fall back to **content-matching subtree replacement** when the realized container has no grid wrapper (this handles the nested-descent case where the patched inner container needs to be spliced into a `SequenceElem` item body).

**Tech Stack:** Rust, Typst internals (`Content`, `GridElem`, `BlockElem`, `StyledElem`, `SequenceElem`), `similar` Myers LCS

---

## File Structure

| File | Change |
|---|---|
| `src/annotated.rs` | Add `#[derive(Clone)]` to `FootnoteInfo`; fix root-level mismatch in `annotate_realized` |
| `src/diff.rs` | Add `page_styles` to `DiffNode`; add `find_annotated_child`, `can_recurse_via_slots`, `find_slot_bearing_descendant_pair`, `diff_slot_children`, `diff_slot_children_same_shape`, `diff_slot_children_lcs`; rewrite `diff_annotated` |
| `src/content_slots.rs` | Add public `rebuild_realized_grid_with_cells` that descends through StyledElem/BlockElem wrappers; add public `replace_subtree` for content-matching subtree replacement |
| `src/annotate.rs` | Rewrite `apply_changed_descendants` to use `rebuild_realized_grid_with_cells` with a `replace_subtree` fallback for non-grid containers |
| `tests/integration.rs` | Add Phase B integration tests for corpus 18, 19, 35, 64, 65, 69 |

---

## Task 1: Add `#[derive(Clone)]` to `FootnoteInfo`

`AnnotatedContent` and `Annotation` already have `#[derive(Clone)]`, but the codebase doesn't currently compile because `Annotation` contains `Option<FootnoteInfo>` and `FootnoteInfo` lacks the derive. Phase B's `diff_annotated` stores annotated nodes inside `DiffNode`, which requires the full Clone chain to actually work.

**Files:**
- Modify: `src/annotated.rs`

- [ ] **Step 1: Confirm the codebase currently fails to compile**

```bash
cargo build 2>&1 | head -20
```

Expected: error `the trait bound 'FootnoteInfo: Clone' is not satisfied` at `src/annotated.rs:56:5` (inside the Annotation struct).

- [ ] **Step 2: Add `#[derive(Clone)]` to `FootnoteInfo`**

In `src/annotated.rs`, find the `FootnoteInfo` struct (around line 105) and add the derive:

```rust
#[derive(Clone)]
pub struct FootnoteInfo {
    pub body: Content,
}
```

Do NOT touch `AnnotatedContent` or `Annotation` — they already have the derive.

- [ ] **Step 3: Verify compilation**

```bash
cargo build 2>&1 | tail -10
```

Expected: compiles without errors. (`SemanticKind` and `SemanticSlot` already derive Clone.)

- [ ] **Step 4: Commit**

```bash
git add src/annotated.rs
git commit -m "refactor: derive Clone for FootnoteInfo so Annotation's Clone derive compiles"
```

---

## Task 2: Fix root-level mismatch in `annotate_realized`

**The bug:** `eval_to_realized_content` calls `annotate_realized(pre_content, realized_content)` where:
- `pre_content` = `SequenceElem([block0, block1, ...])` (output of normalize_list_item_runs)
- `realized_content` = `StyledElem(root_page_styles, SequenceElem([StyledElem(non_page_styles, block0), ...]))` (output of realize_to_content)

`annotate_realized` has no branch for `(SequenceElem, StyledElem)` — it falls through to the leaf fallback, producing an `AnnotatedContent` with no children. All block-level semantic annotations are lost.

**The fix:** Before any other check, detect this mismatch and peel the outer StyledElem.

**Files:**
- Modify: `src/annotated.rs`
- Modify: `src/eval.rs` (add unit test)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `src/eval.rs`:

```rust
#[test]
fn annotate_realized_root_has_semantic_children() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("main.typ"), "- Item A\n- Item B").unwrap();
    let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
    let annotated = eval_to_realized_content(&world).unwrap();

    assert!(
        !annotated.children.is_empty(),
        "root annotated node must have children; got none"
    );
    let has_list = annotated.children.iter().any(|c| {
        matches!(c.annotation.semantic_kind, Some(crate::annotated::SemanticKind::List))
    });
    assert!(
        has_list,
        "expected a child with SemanticKind::List, children had kinds: {:?}",
        annotated.children.iter().map(|c| &c.annotation.semantic_kind).collect::<Vec<_>>()
    );
}
```

- [ ] **Step 2: Run test to confirm it fails**

```bash
cargo test annotate_realized_root_has_semantic_children 2>&1 | tail -20
```

Expected: FAIL — "root annotated node must have children; got none"

- [ ] **Step 3: Add the fix to `annotate_realized`**

In `src/annotated.rs`, at the very top of `annotate_realized` (before all other branches), add:

```rust
// Root-level mismatch: pre is a bare SequenceElem but realized has been
// wrapped with root page styles (StyledElem → SequenceElem). Peel the
// wrapper and recurse with the same pre so the inner SequenceElem matches.
if pre.to_packed::<SequenceElem>().is_some() {
    if let Some(styled) = realized.to_packed::<StyledElem>() {
        let inner = annotate_realized(pre, &styled.child);
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation { span: pre.span(), ..Annotation::default() },
            children: inner.children,
        };
    }
}
```

This must come before the `if pre.is::<ListElem>()` check. After peeling, `annotate_realized(SequenceElem_pre, SequenceElem_realized)` hits the existing `(SequenceElem, SequenceElem)` branch and pairs children correctly.

- [ ] **Step 4: Run test to confirm it passes**

```bash
cargo test annotate_realized_root_has_semantic_children 2>&1 | tail -10
```

Expected: PASS

- [ ] **Step 5: Run full test suite to check for regressions**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests that passed before still pass. (The corpus 18 integration test `list_item_change_produces_has_changed_descendants_not_flat_modified` will still fail — that's expected, it's fixed by Task 6.)

- [ ] **Step 6: Commit**

```bash
git add src/annotated.rs src/eval.rs
git commit -m "fix(annotate): handle root-level SequenceElem/StyledElem mismatch in annotate_realized"
```

---

## Task 3: Add `page_styles` to `DiffNode`

Currently `annotate_single_node` uses `Default::default()` for `page_styles`. Plumb the actual page styles through `DiffNode` so block-level page-style grouping works correctly in `build_annotated_content_from_tree`.

**Files:**
- Modify: `src/diff.rs`
- Modify: `src/annotate.rs`

- [ ] **Step 1: Add `page_styles` field to `DiffNode`**

In `src/diff.rs`, update the struct:

```rust
pub struct DiffNode {
    pub node: AnnotatedContent,
    pub status: NodeStatus,
    /// Per-slot children, populated when status is `HasChangedDescendants`.
    pub children: Vec<DiffNode>,
    /// Page styles active at this block's position (for output grouping).
    pub page_styles: Styles,
}
```

You will likely need to add `use typst::foundations::Styles;` to the imports.

- [ ] **Step 2: Update `diff_result_op_to_node` to carry page styles**

Update each arm to include `page_styles`:

```rust
fn diff_result_op_to_node(op: DiffResultOp, new_ac: &AnnotatedContent) -> DiffNode {
    match op {
        DiffResultOp::Equal(block) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Unchanged,
            children: vec![],
            page_styles: block.page_styles,
        },
        DiffResultOp::Deleted(block) => DiffNode {
            node: AnnotatedContent {
                realized: block.content.clone(),
                annotation: Annotation::default(),
                children: vec![],
            },
            status: NodeStatus::Deleted,
            children: vec![],
            page_styles: block.page_styles,
        },
        DiffResultOp::Inserted(block) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Inserted,
            children: vec![],
            page_styles: block.page_styles,
        },
        DiffResultOp::Modified(block, word_ops) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Modified(word_ops),
            children: vec![],
            page_styles: block.page_styles,
        },
        DiffResultOp::ModifiedSlots(block, slot_diffs) => {
            let node = find_or_wrap_annotated(&block.content, new_ac);
            let children = slot_diffs
                .into_iter()
                .map(|sd| DiffNode {
                    node: AnnotatedContent {
                        realized: sd
                            .ops
                            .iter()
                            .find_map(|op| match op {
                                DiffResultOp::Modified(b, _) => Some(b.content.clone()),
                                DiffResultOp::Equal(b) => Some(b.content.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| TextElem::packed("")),
                        annotation: Annotation::default(),
                        children: vec![],
                    },
                    status: NodeStatus::HasChangedDescendants,
                    children: vec![],
                    page_styles: block.page_styles.clone(),
                })
                .collect();
            DiffNode {
                node,
                status: NodeStatus::HasChangedDescendants,
                children,
                page_styles: block.page_styles,
            }
        }
    }
}
```

- [ ] **Step 3: Update `annotate_single_node` in `src/annotate.rs`**

Replace the `let page_styles = Default::default();` line:

```rust
fn annotate_single_node(node: &DiffNode, compact: bool) -> DiffBlock {
    let page_styles = node.page_styles.clone();
    // ... rest unchanged
```

- [ ] **Step 4: Verify compilation**

```bash
cargo build 2>&1 | tail -20
```

Expected: compiles. Fix any DiffNode construction sites that need the new field (likely test fixtures in `src/annotate.rs#[cfg(test)]`).

- [ ] **Step 5: Run tests**

```bash
cargo test 2>&1 | tail -10
```

Expected: same set of tests passes as before. (corpus 18 integration test still fails until Task 6.)

- [ ] **Step 6: Commit**

```bash
git add src/diff.rs src/annotate.rs
git commit -m "refactor: plumb page_styles through DiffNode for correct output grouping"
```

---

## Task 4: Add `find_annotated_child`, `can_recurse_via_slots`, `find_slot_bearing_descendant_pair`

These three helpers are the bridge between the flat block-level diff and the annotated tree.

- **`find_annotated_child`** — After the Task 2 fix, `root.children[i].realized` structurally equals `extract_block_units(root.realized)[i].content`. So a linear search by structural equality is correct and O(n).
- **`can_recurse_via_slots`** — Returns true when both annotated nodes have the same non-None, non-Equation `SemanticKind` and at least one slot.
- **`find_slot_bearing_descendant_pair`** — When a paired slot child has no `semantic_kind` (e.g. corpus 69's `SequenceElem` item body), walks both subtrees to find the unique pair of slot-bearing descendants. Returns `Some(pair)` only if exactly one matching pair exists on each side AND `can_recurse_via_slots` agrees on the pair.

**Files:**
- Modify: `src/diff.rs`

- [ ] **Step 1: Write unit tests for the three helpers**

Add to the `#[cfg(test)]` block in `src/diff.rs`:

```rust
#[test]
fn find_annotated_child_returns_child_with_matching_realized() {
    use crate::annotated::{Annotation, AnnotatedContent};
    use typst::text::TextElem;

    let target = TextElem::packed("hello");
    let other = TextElem::packed("world");
    let root = AnnotatedContent {
        realized: TextElem::packed("root"),
        annotation: Annotation::default(),
        children: vec![
            AnnotatedContent {
                realized: other.clone(),
                annotation: Annotation::default(),
                children: vec![],
            },
            AnnotatedContent {
                realized: target.clone(),
                annotation: Annotation::default(),
                children: vec![],
            },
        ],
    };
    let found = find_annotated_child(&root, &target);
    assert!(found.is_some());
    assert!(found.unwrap().realized == target);
}

#[test]
fn find_annotated_child_returns_none_when_no_match() {
    use crate::annotated::{Annotation, AnnotatedContent};
    use typst::text::TextElem;

    let root = AnnotatedContent {
        realized: TextElem::packed("root"),
        annotation: Annotation::default(),
        children: vec![AnnotatedContent {
            realized: TextElem::packed("child"),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };
    assert!(find_annotated_child(&root, &TextElem::packed("missing")).is_none());
}

#[test]
fn can_recurse_via_slots_true_for_matching_list_kinds() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let make = |kind: SemanticKind| AnnotatedContent {
        realized: TextElem::packed("x"),
        annotation: Annotation {
            semantic_kind: Some(kind),
            slots: vec![SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 }],
            ..Annotation::default()
        },
        children: vec![AnnotatedContent {
            realized: TextElem::packed("item"),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };
    let old = make(SemanticKind::List);
    let new = make(SemanticKind::List);
    assert!(can_recurse_via_slots(&old, &new));
}

#[test]
fn can_recurse_via_slots_false_for_equation() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let eq = AnnotatedContent {
        realized: TextElem::packed("x"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::Equation),
            slots: vec![SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 }],
            ..Annotation::default()
        },
        children: vec![],
    };
    assert!(!can_recurse_via_slots(&eq, &eq));
}

#[test]
fn find_slot_bearing_descendant_pair_finds_nested_list() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let make_inner_list = || AnnotatedContent {
        realized: TextElem::packed("inner-list"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: vec![SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 }],
            ..Annotation::default()
        },
        children: vec![AnnotatedContent {
            realized: TextElem::packed("inner-item"),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };

    // Outer: SequenceElem-like (no semantic_kind), children = [text, inner_list]
    let make_outer = || AnnotatedContent {
        realized: TextElem::packed("outer-body"),
        annotation: Annotation::default(),
        children: vec![
            AnnotatedContent {
                realized: TextElem::packed("Plan release"),
                annotation: Annotation::default(),
                children: vec![],
            },
            make_inner_list(),
        ],
    };

    let old = make_outer();
    let new = make_outer();
    let pair = find_slot_bearing_descendant_pair(&old, &new);
    assert!(pair.is_some(), "expected to find inner list pair");
    let (oi, ni) = pair.unwrap();
    assert_eq!(oi.annotation.semantic_kind, Some(SemanticKind::List));
    assert_eq!(ni.annotation.semantic_kind, Some(SemanticKind::List));
}

#[test]
fn find_slot_bearing_descendant_pair_returns_none_when_no_descendant() {
    use crate::annotated::{Annotation, AnnotatedContent};
    use typst::text::TextElem;

    let leaf = AnnotatedContent {
        realized: TextElem::packed("just text"),
        annotation: Annotation::default(),
        children: vec![AnnotatedContent {
            realized: TextElem::packed("inner"),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };
    assert!(find_slot_bearing_descendant_pair(&leaf, &leaf).is_none());
}

#[test]
fn find_slot_bearing_descendant_pair_returns_none_with_multiple_candidates() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let make_list = || AnnotatedContent {
        realized: TextElem::packed("list"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: vec![SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 }],
            ..Annotation::default()
        },
        children: vec![AnnotatedContent {
            realized: TextElem::packed("item"),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };

    // Two inner lists at the same level → ambiguous → return None
    let node = AnnotatedContent {
        realized: TextElem::packed("body"),
        annotation: Annotation::default(),
        children: vec![make_list(), make_list()],
    };
    assert!(find_slot_bearing_descendant_pair(&node, &node).is_none());
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test find_annotated_child can_recurse_via_slots find_slot_bearing_descendant_pair 2>&1 | tail -20
```

Expected: FAIL — functions not found.

- [ ] **Step 3: Implement the helpers in `src/diff.rs`**

Add `SemanticKind` to the import from `crate::annotated`:

```rust
use crate::annotated::{AnnotatedContent, Annotation, SemanticKind};
```

Add the three functions after the `find_or_wrap_annotated` function:

```rust
/// Find the direct child of `root` whose `realized` structurally equals `target`.
///
/// After the root-level mismatch fix in `annotate_realized`, each entry in
/// `root.children` corresponds to one block produced by `extract_block_units`.
/// Both have the form `StyledElem(non_page_styles, block_i)`, so structural
/// equality is a reliable lookup key.
fn find_annotated_child<'a>(root: &'a AnnotatedContent, target: &Content) -> Option<&'a AnnotatedContent> {
    root.children.iter().find(|child| child.realized == *target)
}

/// True if both annotated nodes have matching, slot-bearing, non-Equation kinds.
///
/// This is the guard for slot recursion. Equations are atomic (tier 1) and
/// fall through to word diff. Mismatched kinds (e.g. old was a List, new is a
/// Table) also fall through.
fn can_recurse_via_slots(old_ann: &AnnotatedContent, new_ann: &AnnotatedContent) -> bool {
    let Some(old_kind) = old_ann.annotation.semantic_kind.as_ref() else { return false; };
    let Some(new_kind) = new_ann.annotation.semantic_kind.as_ref() else { return false; };
    if old_kind != new_kind { return false; }
    if matches!(old_kind, SemanticKind::Equation) { return false; }
    !old_ann.annotation.slots.is_empty()
}

/// Find a unique pair of slot-bearing descendants in `old_node` and `new_node`'s
/// children (transitively, stopping at the first slot-bearing node along each branch).
///
/// Used when a slot child has no `semantic_kind` of its own but might wrap a
/// nested slot-bearing container (e.g. corpus 69: outer list item body is a
/// `SequenceElem` whose children include a nested `ListElem`).
///
/// Returns `Some((old_inner, new_inner))` only when:
/// - exactly one slot-bearing descendant exists on each side, AND
/// - the pair passes `can_recurse_via_slots`.
///
/// Otherwise returns `None`, signalling the caller to fall back to word diff.
fn find_slot_bearing_descendant_pair<'a>(
    old_node: &'a AnnotatedContent,
    new_node: &'a AnnotatedContent,
) -> Option<(&'a AnnotatedContent, &'a AnnotatedContent)> {
    let old_descs = collect_slot_bearing_descendants(old_node);
    let new_descs = collect_slot_bearing_descendants(new_node);
    if old_descs.len() != 1 || new_descs.len() != 1 {
        return None;
    }
    if can_recurse_via_slots(old_descs[0], new_descs[0]) {
        Some((old_descs[0], new_descs[0]))
    } else {
        None
    }
}

fn collect_slot_bearing_descendants<'a>(node: &'a AnnotatedContent) -> Vec<&'a AnnotatedContent> {
    let mut out = Vec::new();
    collect_slot_bearing_descendants_into(node, &mut out);
    out
}

fn collect_slot_bearing_descendants_into<'a>(
    node: &'a AnnotatedContent,
    out: &mut Vec<&'a AnnotatedContent>,
) {
    for child in &node.children {
        if !child.annotation.slots.is_empty() {
            out.push(child);
        } else {
            collect_slot_bearing_descendants_into(child, out);
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test find_annotated_child can_recurse_via_slots find_slot_bearing_descendant_pair 2>&1 | tail -20
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat(diff): add find_annotated_child, can_recurse_via_slots, find_slot_bearing_descendant_pair helpers"
```

---

## Task 5: Implement `diff_slot_children_same_shape` with nested descent

`diff_slot_children_same_shape` handles the case where old and new containers have the same number of children. It zips them by position: equal realized content → `Unchanged`; differing → recurse (if both have slots directly OR via descendant), or word-diff → `Modified`.

When the slot child has no direct slots but contains a unique slot-bearing descendant, emit a "wrapper" HasChangedDescendants DiffNode whose `node` is the outer slot child but whose single child describes the inner descent. `apply_changed_descendants` (Task 8) handles splicing this back into the outer cell.

**Files:**
- Modify: `src/diff.rs`

- [ ] **Step 1: Write unit tests**

Add to `#[cfg(test)]` in `src/diff.rs`:

```rust
#[test]
fn diff_slot_children_same_shape_marks_changed_item_modified() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let make_child = |text: &str| AnnotatedContent {
        realized: TextElem::packed(text),
        annotation: Annotation::default(),
        children: vec![],
    };
    let make_list = |texts: &[&str]| AnnotatedContent {
        realized: TextElem::packed("list"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: texts
                .iter()
                .enumerate()
                .map(|(i, _)| SemanticSlot { label: SlotStep::ListItem(i), child_index: i })
                .collect(),
            ..Annotation::default()
        },
        children: texts.iter().map(|t| make_child(t)).collect(),
    };

    let old = make_list(&["Item A", "Old item", "Item C"]);
    let new = make_list(&["Item A", "New item", "Item C"]);
    let styles = Styles::new();

    let result = diff_slot_children_same_shape(&old, &new, &styles);

    assert_eq!(result.len(), 3);
    assert!(matches!(result[0].status, NodeStatus::Unchanged), "item 0 should be Unchanged");
    assert!(matches!(result[1].status, NodeStatus::Modified(_)), "item 1 should be Modified");
    assert!(matches!(result[2].status, NodeStatus::Unchanged), "item 2 should be Unchanged");
}

#[test]
fn diff_slot_children_same_shape_all_unchanged_when_equal() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    let make_list = |texts: &[&str]| AnnotatedContent {
        realized: TextElem::packed("list"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: texts
                .iter()
                .enumerate()
                .map(|(i, _)| SemanticSlot { label: SlotStep::ListItem(i), child_index: i })
                .collect(),
            ..Annotation::default()
        },
        children: texts.iter().map(|t| AnnotatedContent {
            realized: TextElem::packed(t),
            annotation: Annotation::default(),
            children: vec![],
        }).collect(),
    };

    let list = make_list(&["A", "B", "C"]);
    let result = diff_slot_children_same_shape(&list, &list, &Styles::new());

    assert_eq!(result.len(), 3);
    for child in &result {
        assert!(matches!(child.status, NodeStatus::Unchanged));
    }
}

#[test]
fn diff_slot_children_same_shape_recurses_into_nested_descendant() {
    use crate::annotated::{Annotation, AnnotatedContent, SemanticKind, SemanticSlot};
    use crate::content_slots::SlotStep;
    use typst::text::TextElem;

    // Build an outer list with 2 items.
    // Item 0's body is a SequenceElem-shape (no semantic_kind) containing a
    // nested list with 1 item that differs old vs new.
    // Item 1's body is plain text, unchanged.

    let make_inner_list = |item_text: &str| AnnotatedContent {
        realized: TextElem::packed(item_text), // pretend the realized form differs
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: vec![SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 }],
            ..Annotation::default()
        },
        children: vec![AnnotatedContent {
            realized: TextElem::packed(item_text),
            annotation: Annotation::default(),
            children: vec![],
        }],
    };

    let make_item_body = |inner_text: &str, body_id: &str| AnnotatedContent {
        // SequenceElem-shape: no semantic_kind, has the inner list as a child.
        realized: TextElem::packed(body_id),
        annotation: Annotation::default(),
        children: vec![
            AnnotatedContent {
                realized: TextElem::packed("Plan release"),
                annotation: Annotation::default(),
                children: vec![],
            },
            make_inner_list(inner_text),
        ],
    };

    let outer_old = AnnotatedContent {
        realized: TextElem::packed("outer-list-old"),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::List),
            slots: vec![
                SemanticSlot { label: SlotStep::ListItem(0), child_index: 0 },
                SemanticSlot { label: SlotStep::ListItem(1), child_index: 1 },
            ],
            ..Annotation::default()
        },
        children: vec![
            make_item_body("Old inner", "item-0-body-old"),
            AnnotatedContent {
                realized: TextElem::packed("Ship release"),
                annotation: Annotation::default(),
                children: vec![],
            },
        ],
    };
    let outer_new = AnnotatedContent {
        realized: TextElem::packed("outer-list-new"),
        annotation: outer_old.annotation.clone(),
        children: vec![
            make_item_body("New inner", "item-0-body-new"),
            AnnotatedContent {
                realized: TextElem::packed("Ship release"),
                annotation: Annotation::default(),
                children: vec![],
            },
        ],
    };

    let result = diff_slot_children_same_shape(&outer_old, &outer_new, &Styles::new());
    assert_eq!(result.len(), 2);
    assert!(
        matches!(result[0].status, NodeStatus::HasChangedDescendants),
        "item 0 should be HasChangedDescendants (nested recursion succeeded), got {:?}",
        result[0].status
    );
    assert_eq!(result[0].children.len(), 1, "outer wrapper has one inner descent DiffNode");
    assert!(
        matches!(result[0].children[0].status, NodeStatus::HasChangedDescendants),
        "inner descent should be HasChangedDescendants on the nested list"
    );
    assert!(
        matches!(result[1].status, NodeStatus::Unchanged),
        "item 1 should be Unchanged"
    );
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test diff_slot_children_same_shape 2>&1 | tail -20
```

Expected: FAIL — function not found.

- [ ] **Step 3: Implement `diff_slot_children_same_shape` and `diff_slot_children`**

Add to `src/diff.rs`:

```rust
/// Diff slot children when old and new containers have the same child count.
///
/// Children are compared positionally. Equal realized content → `Unchanged`.
/// Differing content tries:
///   1. Direct recursive slot descent (`can_recurse_via_slots`).
///   2. Multi-level descent into a unique slot-bearing descendant
///      (`find_slot_bearing_descendant_pair`). Emits a HasChangedDescendants
///      wrapper whose `node` is the outer slot child and whose single child
///      describes the inner descent — `apply_changed_descendants` splices the
///      patched inner container back into the outer cell via content matching.
///   3. Word diff fallback.
fn diff_slot_children_same_shape(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
    page_styles: &Styles,
) -> Vec<DiffNode> {
    old_ann
        .children
        .iter()
        .zip(new_ann.children.iter())
        .map(|(old_child, new_child)| {
            if old_child.realized == new_child.realized {
                return DiffNode {
                    node: new_child.clone(),
                    status: NodeStatus::Unchanged,
                    children: vec![],
                    page_styles: page_styles.clone(),
                };
            }

            // 1. Try direct slot descent for slot-bearing containers (e.g. nested lists).
            if can_recurse_via_slots(old_child, new_child) {
                let grandchildren = diff_slot_children(old_child, new_child, page_styles);
                let any_changed = grandchildren
                    .iter()
                    .any(|n| !matches!(n.status, NodeStatus::Unchanged));
                if any_changed {
                    return DiffNode {
                        node: new_child.clone(),
                        status: NodeStatus::HasChangedDescendants,
                        children: grandchildren,
                        page_styles: page_styles.clone(),
                    };
                }
                return DiffNode {
                    node: new_child.clone(),
                    status: NodeStatus::Unchanged,
                    children: vec![],
                    page_styles: page_styles.clone(),
                };
            }

            // 2. Try multi-level descent into a slot-bearing descendant.
            if let Some((old_inner, new_inner)) =
                find_slot_bearing_descendant_pair(old_child, new_child)
            {
                let inner_grandchildren = diff_slot_children(old_inner, new_inner, page_styles);
                let any_changed = inner_grandchildren
                    .iter()
                    .any(|n| !matches!(n.status, NodeStatus::Unchanged));
                if any_changed {
                    let inner_diff = DiffNode {
                        node: new_inner.clone(),
                        status: NodeStatus::HasChangedDescendants,
                        children: inner_grandchildren,
                        page_styles: page_styles.clone(),
                    };
                    return DiffNode {
                        node: new_child.clone(),
                        status: NodeStatus::HasChangedDescendants,
                        children: vec![inner_diff],
                        page_styles: page_styles.clone(),
                    };
                }
            }

            // 3. Leaf: word diff.
            let old_tokens = extract_words(&old_child.realized);
            let new_tokens = extract_words(&new_child.realized);
            let word_ops = diff_words(&old_tokens, &new_tokens);
            let status = if has_textual_word_change(&word_ops) {
                NodeStatus::Modified(word_ops)
            } else {
                NodeStatus::Unchanged
            };
            DiffNode {
                node: new_child.clone(),
                status,
                children: vec![],
                page_styles: page_styles.clone(),
            }
        })
        .collect()
}

/// Dispatch slot diffing to same-shape or LCS based on child count.
fn diff_slot_children(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
    page_styles: &Styles,
) -> Vec<DiffNode> {
    if old_ann.children.len() == new_ann.children.len() {
        diff_slot_children_same_shape(old_ann, new_ann, page_styles)
    } else {
        diff_slot_children_lcs(old_ann, new_ann, page_styles)
    }
}
```

Add a stub for `diff_slot_children_lcs` (implemented in Task 9):

```rust
/// Diff slot children when old and new containers have different child counts.
///
/// Runs Myers LCS on the slot children's realized content to pair unchanged
/// children across the insertion/deletion. Implemented in Task 9.
fn diff_slot_children_lcs(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
    page_styles: &Styles,
) -> Vec<DiffNode> {
    // Stub: fall back to word diff on the whole block until Task 9.
    let old_tokens = extract_words(&old_ann.realized);
    let new_tokens = extract_words(&new_ann.realized);
    let word_ops = diff_words(&old_tokens, &new_tokens);
    let status = if has_textual_word_change(&word_ops) {
        NodeStatus::Modified(word_ops)
    } else {
        NodeStatus::Unchanged
    };
    vec![DiffNode {
        node: new_ann.clone(),
        status,
        children: vec![],
        page_styles: page_styles.clone(),
    }]
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test diff_slot_children_same_shape 2>&1 | tail -20
```

Expected: all three PASS.

- [ ] **Step 5: Run full suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: previously-passing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/diff.rs
git commit -m "feat(diff): implement diff_slot_children_same_shape with direct and nested recursion"
```

---

## Task 6: Rewrite `diff_annotated` to use slot recursion

Rewrite `diff_annotated` to call `extract_block_units` / `diff_block_units_raw` / `match_edit_zones` directly (instead of going through `diff_content`), so that `BlockOp::Replace` is still available when we need to look up annotated nodes and decide whether to recurse.

**Files:**
- Modify: `src/diff.rs`

- [ ] **Step 1: Confirm the existing corpus 18 integration test still fails**

```bash
cargo test list_item_change_produces_has_changed_descendants_not_flat_modified -- --nocapture 2>&1 | tail -30
```

Note the exact failure message. After this task it should PASS.

- [ ] **Step 2: Rewrite `diff_annotated`**

Replace the existing `diff_annotated` function in `src/diff.rs`:

```rust
pub fn diff_annotated(old: &AnnotatedContent, new: &AnnotatedContent) -> DiffResult {
    let old_blocks = extract_block_units(&old.realized);
    let new_blocks = extract_block_units(&new.realized);
    let raw = diff_block_units_raw(&old_blocks, &new_blocks);
    let matched = match_edit_zones(raw);
    let root_styles = root_page_styles(&new.realized);

    let blocks = matched
        .into_iter()
        .map(|op| match op {
            BlockOp::Equal(_, new_block) => DiffNode {
                node: find_or_wrap_annotated(&new_block.content, new),
                status: NodeStatus::Unchanged,
                children: vec![],
                page_styles: new_block.page_styles,
            },
            BlockOp::Delete(old_block) => DiffNode {
                node: AnnotatedContent {
                    realized: old_block.content.clone(),
                    annotation: Annotation::default(),
                    children: vec![],
                },
                status: NodeStatus::Deleted,
                children: vec![],
                page_styles: old_block.page_styles,
            },
            BlockOp::Insert(new_block) => DiffNode {
                node: find_or_wrap_annotated(&new_block.content, new),
                status: NodeStatus::Inserted,
                children: vec![],
                page_styles: new_block.page_styles,
            },
            BlockOp::Replace(old_block, new_block) => {
                let page_styles = new_block.page_styles.clone();
                let old_ann = find_annotated_child(old, &old_block.content);
                let new_ann = find_annotated_child(new, &new_block.content);

                if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann) {
                    if can_recurse_via_slots(old_ann, new_ann) {
                        let slot_children =
                            diff_slot_children(old_ann, new_ann, &page_styles);
                        let any_changed = slot_children
                            .iter()
                            .any(|c| !matches!(c.status, NodeStatus::Unchanged));
                        if any_changed {
                            return DiffNode {
                                node: AnnotatedContent {
                                    realized: new_block.content.clone(),
                                    annotation: new_ann.annotation.clone(),
                                    children: new_ann.children.clone(),
                                },
                                status: NodeStatus::HasChangedDescendants,
                                children: slot_children,
                                page_styles,
                            };
                        } else {
                            return DiffNode {
                                node: find_or_wrap_annotated(&new_block.content, new),
                                status: NodeStatus::Unchanged,
                                children: vec![],
                                page_styles,
                            };
                        }
                    }
                }

                // Fall back to word diff.
                let old_tokens = extract_words(&old_block.content);
                let new_tokens = extract_words(&new_block.content);
                let word_ops = diff_words(&old_tokens, &new_tokens);
                let status = if has_textual_word_change(&word_ops) {
                    NodeStatus::Modified(word_ops)
                } else {
                    NodeStatus::Unchanged
                };
                DiffNode {
                    node: find_or_wrap_annotated(&new_block.content, new),
                    status,
                    children: vec![],
                    page_styles,
                }
            }
        })
        .collect();

    DiffResult { blocks, root_styles }
}
```

- [ ] **Step 3: Run the corpus 18 integration test**

```bash
cargo test list_item_change_produces_has_changed_descendants_not_flat_modified -- --nocapture 2>&1 | tail -30
```

Expected: PASS — list block is `HasChangedDescendants`, 3 Unchanged children, 1 Modified child.

- [ ] **Step 4: Run full test suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests that passed before still pass. (Corpus 19/65/69 tests don't exist yet; they're added in later tasks.) Note: the visual corpus output for affected tests (18, 19, 35, 64, 65, 69) may now diverge from the reference PDFs — that's expected and gets re-baselined in Task 13.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat(diff): rewrite diff_annotated with slot recursion for same-shape containers"
```

---

## Task 7: Add `rebuild_realized_grid_with_cells` and `replace_subtree` in `src/content_slots.rs`

The patched-container builder needs two primitives:

- **`rebuild_realized_grid_with_cells`** — Take a realized container (possibly wrapped in `StyledElem`/`BlockElem`) and replace its inner `GridElem`'s **body cell bodies** with a supplied list, in order. Extra cells (more cells than the original grid had) are appended as new `GridItem::Cell` entries. Header/footer cells are preserved as-is. Returns `None` if no `GridElem` is reachable.

- **`replace_subtree`** — Find a specific `Content` subtree inside another by structural equality and replace it. Walks `SequenceElem` and `StyledElem` wrappers. Used by `apply_changed_descendants` (Task 8) to splice patched nested containers back into a non-grid item body (the nested-descent case for corpus 69).

**Files:**
- Modify: `src/content_slots.rs`

- [ ] **Step 1: Write unit tests for `rebuild_realized_grid_with_cells`**

Add to `#[cfg(test)]` in `src/content_slots.rs`:

```rust
#[test]
fn rebuild_realized_grid_replaces_each_cell_body_in_order() {
    use typst::foundations::Packed;
    use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};

    let cells = vec![
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("A"))))),
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("B"))))),
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("C"))))),
    ];
    let grid = Content::new(GridElem::new(cells));
    let container = Content::new(
        BlockElem::new().with_body(Some(BlockBody::Content(grid))),
    );

    let new_cells = vec![text("X"), text("Y"), text("Z")];
    let rebuilt = rebuild_realized_grid_with_cells(&container, new_cells).unwrap();

    let plain = rebuilt.plain_text();
    assert!(plain.contains('X'), "expected X in {plain}");
    assert!(plain.contains('Y'), "expected Y in {plain}");
    assert!(plain.contains('Z'), "expected Z in {plain}");
    assert!(!plain.contains('A'), "A should be replaced");
    assert!(!plain.contains('B'), "B should be replaced");
    assert!(!plain.contains('C'), "C should be replaced");
}

#[test]
fn rebuild_realized_grid_appends_extra_cells_at_end() {
    use typst::foundations::Packed;
    use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};

    let cells = vec![
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("A"))))),
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("B"))))),
    ];
    let grid = Content::new(GridElem::new(cells));
    let container = Content::new(
        BlockElem::new().with_body(Some(BlockBody::Content(grid))),
    );

    let new_cells = vec![text("X"), text("Y"), text("Z"), text("W")];
    let rebuilt = rebuild_realized_grid_with_cells(&container, new_cells).unwrap();

    let plain = rebuilt.plain_text();
    assert!(plain.contains('X'));
    assert!(plain.contains('Y'));
    assert!(plain.contains('Z'));
    assert!(plain.contains('W'));
}

#[test]
fn rebuild_realized_grid_descends_through_styled_block_wrappers() {
    use typst::foundations::Packed;
    use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};
    use typst::text::TextElem;
    use typst::visualize::Color;

    let cells = vec![
        GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("A"))))),
    ];
    let grid = Content::new(GridElem::new(cells));
    let block = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(grid))));
    let container = block.styled(TextElem::fill.set(Color::from_u8(0, 0, 0, 255).into()));

    let rebuilt = rebuild_realized_grid_with_cells(&container, vec![text("Z")]).unwrap();
    assert!(rebuilt.plain_text().contains('Z'));
}

#[test]
fn rebuild_realized_grid_returns_none_when_no_grid_present() {
    let container = text("just text");
    assert!(rebuild_realized_grid_with_cells(&container, vec![text("X")]).is_none());
}

#[test]
fn replace_subtree_swaps_matching_node_inside_sequence() {
    let needle = text("inner");
    let haystack = Content::sequence([text("before"), needle.clone(), text("after")]);
    let replacement = text("REPLACED");
    let patched = replace_subtree(&haystack, &needle, replacement).unwrap();

    assert!(patched.plain_text().contains("REPLACED"));
    assert!(!patched.plain_text().contains("inner"));
    assert!(patched.plain_text().contains("before"));
    assert!(patched.plain_text().contains("after"));
}

#[test]
fn replace_subtree_returns_none_when_needle_not_found() {
    let haystack = Content::sequence([text("a"), text("b")]);
    let needle = text("missing");
    assert!(replace_subtree(&haystack, &needle, text("Z")).is_none());
}

#[test]
fn replace_subtree_walks_through_styled_wrapper() {
    use typst::text::TextElem;
    use typst::visualize::Color;

    let needle = text("inner");
    let haystack = Content::sequence([text("a"), needle.clone()]).styled(
        TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()),
    );
    let patched = replace_subtree(&haystack, &needle, text("Z")).unwrap();
    assert!(patched.plain_text().contains('Z'));
    assert!(!patched.plain_text().contains("inner"));
}

#[test]
fn replace_subtree_at_root_matches_haystack_directly() {
    let needle = text("whole");
    let patched = replace_subtree(&needle, &needle, text("Z")).unwrap();
    assert_eq!(patched.plain_text(), "Z");
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test rebuild_realized_grid replace_subtree 2>&1 | tail -20
```

Expected: FAIL — functions not found.

- [ ] **Step 3: Implement the two helpers**

Add to `src/content_slots.rs`. Add `use typst::foundations::StyleChain;` if not already imported.

```rust
/// Rebuild the realized grid inside a container by replacing its **body cell bodies**
/// in order with the supplied list. Extra cells are appended as new
/// `GridItem::Cell` entries; header/footer cells are preserved as-is.
///
/// Descends through `StyledElem` and `BlockElem` wrappers to reach the `GridElem`.
/// Returns `None` if no `GridElem` is reachable.
///
/// This is the workhorse for `apply_changed_descendants` in the rebuild path:
/// the diff produces an ordered list of cell contents (one per `DiffNode` child,
/// in output order including deletes-as-strikethrough), and this function
/// splices them into the original realized grid wrapper.
pub fn rebuild_realized_grid_with_cells(
    container: &Content,
    cell_bodies: Vec<Content>,
) -> Option<Content> {
    let mut c = container.clone();
    rebuild_grid_in_realized(&mut c, &cell_bodies)?;
    Some(c)
}

fn rebuild_grid_in_realized(content: &mut Content, cell_bodies: &[Content]) -> Option<()> {
    if content.to_packed::<GridElem>().is_some() {
        return rebuild_grid_body_cells(content, cell_bodies);
    }
    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        return rebuild_grid_in_realized(&mut styled.child, cell_bodies);
    }
    if content.to_packed::<BlockElem>().is_some() {
        let body = content
            .to_packed::<BlockElem>()
            .and_then(|b| b.body.get_cloned(StyleChain::default()));
        let Some(BlockBody::Content(body)) = body else { return None; };
        let mut body = body;
        rebuild_grid_in_realized(&mut body, cell_bodies)?;
        content
            .to_packed_mut::<BlockElem>()
            .unwrap()
            .body
            .set(Some(BlockBody::Content(body)));
        return Some(());
    }
    None
}

fn rebuild_grid_body_cells(content: &mut Content, cell_bodies: &[Content]) -> Option<()> {
    use typst::foundations::Packed;
    use typst::layout::GridCell;
    let grid = content.to_packed_mut::<GridElem>()?;
    let mut new_children: Vec<GridChild> = Vec::with_capacity(grid.children.len());
    let mut idx: usize = 0;

    for child in &grid.children {
        match child {
            GridChild::Item(GridItem::Cell(cell)) => {
                if idx < cell_bodies.len() {
                    let mut new_cell = (**cell).clone();
                    new_cell.body = cell_bodies[idx].clone();
                    new_children.push(GridChild::Item(GridItem::Cell(Packed::new(new_cell))));
                    idx += 1;
                } else {
                    // No replacement for this cell — preserve original.
                    new_children.push(child.clone());
                }
            }
            other => {
                // Headers, footers, hlines, vlines, non-cell items: preserve.
                new_children.push(other.clone());
            }
        }
    }

    // Extra cells (e.g. inserts beyond original size) — append as fresh cells.
    while idx < cell_bodies.len() {
        let cell = GridCell::new(cell_bodies[idx].clone());
        new_children.push(GridChild::Item(GridItem::Cell(Packed::new(cell))));
        idx += 1;
    }

    grid.children = new_children;
    Some(())
}

/// Find a content subtree inside `haystack` matching `needle` by structural
/// equality, and return `haystack` with that subtree replaced by `replacement`.
///
/// Walks `SequenceElem` and `StyledElem` wrappers. Returns `None` if `needle`
/// is not found.
///
/// Used by `apply_changed_descendants` to splice a patched nested container
/// back into a non-grid item body (e.g. a `SequenceElem` body containing text
/// + parbreak + nested list).
pub fn replace_subtree(
    haystack: &Content,
    needle: &Content,
    replacement: Content,
) -> Option<Content> {
    if haystack == needle {
        return Some(replacement);
    }
    if let Some(seq) = haystack.to_packed::<SequenceElem>() {
        let mut new_children: Vec<Content> = seq.children.clone();
        let mut replacement_slot = Some(replacement);
        let mut found = false;
        for child in new_children.iter_mut() {
            if let Some(rep) = replacement_slot.take() {
                if let Some(patched) = replace_subtree(child, needle, rep.clone()) {
                    *child = patched;
                    found = true;
                    break;
                } else {
                    replacement_slot = Some(rep);
                }
            }
        }
        if found {
            let mut new_haystack = haystack.clone();
            new_haystack.to_packed_mut::<SequenceElem>().unwrap().children = new_children;
            return Some(new_haystack);
        }
        return None;
    }
    if let Some(styled) = haystack.to_packed::<StyledElem>() {
        if let Some(patched) = replace_subtree(&styled.child, needle, replacement) {
            let mut new_haystack = haystack.clone();
            new_haystack.to_packed_mut::<StyledElem>().unwrap().child = patched;
            return Some(new_haystack);
        }
        return None;
    }
    None
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test rebuild_realized_grid replace_subtree 2>&1 | tail -20
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src/content_slots.rs
git commit -m "feat(content_slots): add rebuild_realized_grid_with_cells and replace_subtree"
```

---

## Task 8: Rewrite `apply_changed_descendants` to use rebuild + content-matching

**The problem:** The existing `apply_changed_descendants` calls `replace_slot(&result, &[s.label.clone()], new_content)` where `result` is the post-realization block content. `replace_slot` expects a `ListElem` at the top level and returns `None`. The patch is silently dropped; the output shows the unmodified new block. It also has no way to splice deleted cells back into the grid.

**The fix:** Use a two-path strategy.

1. **Rebuild path** — When the realized container has a grid wrapper, build the new cell list from the diff children (in order) and call `rebuild_realized_grid_with_cells`. This handles Unchanged/Modified/HasChangedDescendants/Inserted naturally and splices `Deleted` cells back in their LCS output position (as red strikethrough cells).
2. **Content-matching path** — Otherwise (e.g. the outer slot child is a `SequenceElem` from corpus 69's nested case), walk the diff children and for each non-Unchanged one, find `diff_child.node.realized` inside the result and replace it via `replace_subtree`.

**Files:**
- Modify: `src/annotate.rs`

- [ ] **Step 1: Rewrite `apply_changed_descendants` in `src/annotate.rs`**

Replace the existing function:

```rust
fn apply_changed_descendants(
    node: &crate::annotated::AnnotatedContent,
    diff_children: &[DiffNode],
    compact: bool,
) -> Content {
    use crate::content_slots::{rebuild_realized_grid_with_cells, replace_subtree};

    // Compute the patched cell content for each diff child.
    let cell_bodies: Vec<Content> = diff_children
        .iter()
        .map(|child| annotate_single_node(child, compact).content)
        .collect();

    // Path 1: Grid rebuild. Works for any container whose realized form has a
    // GridElem inside (lists, enums, tables — all use BlockElem(GridElem(cells))).
    if let Some(rebuilt) =
        rebuild_realized_grid_with_cells(&node.realized, cell_bodies.clone())
    {
        return rebuilt;
    }

    // Path 2: Content-matching subtree replacement. Used when the container
    // has no grid (e.g. a SequenceElem item body wrapping a nested container —
    // the nested-descent case from `diff_slot_children_same_shape`). For each
    // non-Unchanged child, find its original realized subtree inside `result`
    // and splice in the patched version.
    let mut result = node.realized.clone();
    for (i, diff_child) in diff_children.iter().enumerate() {
        if matches!(diff_child.status, NodeStatus::Unchanged) {
            continue;
        }
        let new_content = cell_bodies[i].clone();
        if let Some(patched) = replace_subtree(&result, &diff_child.node.realized, new_content) {
            result = patched;
        }
    }
    result
}
```

You can drop the `use crate::content_slots::replace_slot;` import line if it becomes unused in this file — the old DiffResultFlat path still uses `replace_slot` via `replace_modified_slots`, so leave it imported if `replace_modified_slots` still calls it.

- [ ] **Step 2: Run tests including the corpus 18 integration test**

```bash
cargo test 2>&1 | tail -20
```

Expected: all previously-passing tests still pass; `list_item_change_produces_has_changed_descendants_not_flat_modified` passes.

- [ ] **Step 3: Visual smoke test — run corpus 18 to confirm only the changed list item is colored**

```bash
tests/run_corpus.sh --filter 18 --verbose 2>&1 | tail -20
```

The diff PDF may now disagree with the reference (that's OK — we re-baseline in Task 13). What matters here is no panics and a valid PDF was produced.

Open the generated PDF and check:
- "Fast and reliable performance", "Cross-platform compatibility", "Comprehensive documentation" appear in normal black (unchanged).
- "Old feature that is being replaced" appears with red strikethrough; "New feature that replaces the old one" appears in green.

If the result looks wrong (e.g., the whole list is green), stop and investigate before continuing.

- [ ] **Step 4: Commit**

```bash
git add src/annotate.rs
git commit -m "fix(annotate): rebuild realized grid in apply_changed_descendants; fall back to subtree replacement"
```

---

## Task 9: Implement `diff_slot_children_lcs` + corpus 19 integration test

`diff_slot_children_lcs` handles the different-shape case: old container has N items, new has M items (N ≠ M). It runs Myers LCS on the slot children's realized content to pair unchanged children across the gap. Insertions, deletions, and replacements are all produced in document order. Deletes go through `apply_changed_descendants`'s grid-rebuild path and appear as red strikethrough cells (Task 8).

**Files:**
- Modify: `src/diff.rs`
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write the failing integration test for corpus 19**

Add to `tests/integration.rs`:

```rust
// Corpus #19: 3-item list → 4-item list (one item appended).
// diff_annotated must use slot-level LCS to produce:
//   list block       → HasChangedDescendants
//   items 0, 1, 2    → Unchanged
//   item 3           → Inserted
#[test]
fn list_item_added_produces_per_item_statuses() {
    use typst_diff::diff::NodeStatus;

    let result = diff_annotated_corpus("19-list-item-added");

    let list_block = result
        .blocks
        .iter()
        .find(|b| !matches!(b.status, NodeStatus::Unchanged))
        .expect("expected at least one changed block");

    assert!(
        matches!(list_block.status, NodeStatus::HasChangedDescendants),
        "list block should be HasChangedDescendants, got {:?}",
        list_block.status
    );

    let unchanged_count = list_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Unchanged))
        .count();
    assert_eq!(
        unchanged_count,
        3,
        "expected 3 unchanged list items, got {unchanged_count}"
    );

    let inserted: Vec<_> = list_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Inserted))
        .collect();
    assert_eq!(
        inserted.len(),
        1,
        "expected exactly 1 inserted list item, got {}",
        inserted.len()
    );
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test list_item_added_produces_per_item_statuses -- --nocapture 2>&1 | tail -20
```

Expected: FAIL — list block has `Modified` status (from the LCS stub in Task 5).

- [ ] **Step 3: Implement `diff_slot_children_lcs`**

Replace the stub in `src/diff.rs`:

```rust
/// Diff slot children when old and new containers have different child counts.
///
/// Runs Myers LCS on the slot children's realized content to match unchanged
/// children across insertions/deletions. Paired children that differ become
/// `Modified` (or `HasChangedDescendants` if they have their own slots).
/// Unmatched old children become `Deleted`; unmatched new children become `Inserted`.
///
/// `Deleted` children are included in the result and spliced back into the
/// realized output by `apply_changed_descendants`'s grid-rebuild path
/// (they appear as red strikethrough cells in their LCS output position).
fn diff_slot_children_lcs(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
    page_styles: &Styles,
) -> Vec<DiffNode> {
    let old_blocks: Vec<DiffBlock> = old_ann
        .children
        .iter()
        .map(|c| DiffBlock {
            content: c.realized.clone(),
            page_styles: page_styles.clone(),
        })
        .collect();
    let new_blocks: Vec<DiffBlock> = new_ann
        .children
        .iter()
        .map(|c| DiffBlock {
            content: c.realized.clone(),
            page_styles: page_styles.clone(),
        })
        .collect();

    let raw = diff_block_units_raw(&old_blocks, &new_blocks);
    let matched = match_edit_zones(raw);

    matched
        .into_iter()
        .map(|op| match op {
            BlockOp::Equal(_, new_block) => {
                let new_child = new_ann
                    .children
                    .iter()
                    .find(|c| c.realized == new_block.content)
                    .cloned()
                    .unwrap_or_else(|| AnnotatedContent {
                        realized: new_block.content.clone(),
                        annotation: Annotation::default(),
                        children: vec![],
                    });
                DiffNode {
                    node: new_child,
                    status: NodeStatus::Unchanged,
                    children: vec![],
                    page_styles: page_styles.clone(),
                }
            }
            BlockOp::Delete(old_block) => {
                let old_child = old_ann
                    .children
                    .iter()
                    .find(|c| c.realized == old_block.content)
                    .cloned()
                    .unwrap_or_else(|| AnnotatedContent {
                        realized: old_block.content.clone(),
                        annotation: Annotation::default(),
                        children: vec![],
                    });
                DiffNode {
                    node: old_child,
                    status: NodeStatus::Deleted,
                    children: vec![],
                    page_styles: page_styles.clone(),
                }
            }
            BlockOp::Insert(new_block) => {
                let new_child = new_ann
                    .children
                    .iter()
                    .find(|c| c.realized == new_block.content)
                    .cloned()
                    .unwrap_or_else(|| AnnotatedContent {
                        realized: new_block.content.clone(),
                        annotation: Annotation::default(),
                        children: vec![],
                    });
                DiffNode {
                    node: new_child,
                    status: NodeStatus::Inserted,
                    children: vec![],
                    page_styles: page_styles.clone(),
                }
            }
            BlockOp::Replace(old_block, new_block) => {
                let new_child = new_ann
                    .children
                    .iter()
                    .find(|c| c.realized == new_block.content)
                    .cloned()
                    .unwrap_or_else(|| AnnotatedContent {
                        realized: new_block.content.clone(),
                        annotation: Annotation::default(),
                        children: vec![],
                    });
                let old_child = old_ann
                    .children
                    .iter()
                    .find(|c| c.realized == old_block.content)
                    .cloned();

                // Try recursive slot descent for nested containers.
                if let Some(ref old_c) = old_child {
                    if can_recurse_via_slots(old_c, &new_child) {
                        let grandchildren =
                            diff_slot_children(old_c, &new_child, page_styles);
                        let any_changed = grandchildren
                            .iter()
                            .any(|n| !matches!(n.status, NodeStatus::Unchanged));
                        if any_changed {
                            return DiffNode {
                                node: AnnotatedContent {
                                    realized: new_child.realized.clone(),
                                    annotation: new_child.annotation.clone(),
                                    children: new_child.children.clone(),
                                },
                                status: NodeStatus::HasChangedDescendants,
                                children: grandchildren,
                                page_styles: page_styles.clone(),
                            };
                        }
                    }

                    // Multi-level descent: same fallback as same-shape path.
                    if let Some((old_inner, new_inner)) =
                        find_slot_bearing_descendant_pair(old_c, &new_child)
                    {
                        let inner_grandchildren =
                            diff_slot_children(old_inner, new_inner, page_styles);
                        let any_changed = inner_grandchildren
                            .iter()
                            .any(|n| !matches!(n.status, NodeStatus::Unchanged));
                        if any_changed {
                            let inner_diff = DiffNode {
                                node: new_inner.clone(),
                                status: NodeStatus::HasChangedDescendants,
                                children: inner_grandchildren,
                                page_styles: page_styles.clone(),
                            };
                            return DiffNode {
                                node: new_child.clone(),
                                status: NodeStatus::HasChangedDescendants,
                                children: vec![inner_diff],
                                page_styles: page_styles.clone(),
                            };
                        }
                    }
                }

                // Leaf: word diff.
                let old_tokens = extract_words(&old_block.content);
                let new_tokens = extract_words(&new_block.content);
                let word_ops = diff_words(&old_tokens, &new_tokens);
                let status = if has_textual_word_change(&word_ops) {
                    NodeStatus::Modified(word_ops)
                } else {
                    NodeStatus::Unchanged
                };
                DiffNode {
                    node: new_child,
                    status,
                    children: vec![],
                    page_styles: page_styles.clone(),
                }
            }
        })
        .collect()
}
```

- [ ] **Step 4: Run the corpus 19 integration test**

```bash
cargo test list_item_added_produces_per_item_statuses -- --nocapture 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 5: Run full test suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: all previously-passing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/diff.rs tests/integration.rs
git commit -m "feat(diff): implement diff_slot_children_lcs for different-shape containers"
```

---

## Task 10: Add corpus 65 integration test (delete splice-back)

Corpus 65 is a 4-row table that loses its middle row in the new version (3 cells deleted). After Task 8's rebuild path, those 3 cells should appear in the `DiffNode` tree as `NodeStatus::Deleted` children of the table's `HasChangedDescendants` block. This validates that the splice-back works.

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write the integration test**

```rust
// Corpus #65: table row deleted from the middle (3 cells removed).
// Validates Task 8's grid-rebuild path produces Deleted DiffNodes that get
// spliced back into the realized table as red strikethrough cells.
#[test]
fn table_row_deleted_produces_deleted_cells_in_tree() {
    use typst_diff::diff::NodeStatus;

    let result = diff_annotated_corpus("65-table-row-deleted-middle");

    let table_block = result
        .blocks
        .iter()
        .find(|b| !matches!(b.status, NodeStatus::Unchanged))
        .expect("expected at least one changed block");

    assert!(
        matches!(table_block.status, NodeStatus::HasChangedDescendants),
        "table block should be HasChangedDescendants, got {:?}",
        table_block.status
    );

    let deleted_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Deleted))
        .count();
    assert_eq!(
        deleted_count, 3,
        "expected 3 deleted cells (one full row of 3 columns), got {deleted_count}"
    );

    // The other 12 cells (4 rows × 3 cols in the new table) should be unchanged.
    let unchanged_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Unchanged))
        .count();
    assert_eq!(
        unchanged_count, 12,
        "expected 12 unchanged cells, got {unchanged_count}"
    );
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test table_row_deleted_produces_deleted_cells_in_tree -- --nocapture 2>&1 | tail -20
```

Expected: PASS. If FAIL, inspect the output and consider whether the table is being picked up as a different SemanticKind, or whether `extract_block_units` is segmenting differently.

- [ ] **Step 3: Visual smoke test — run corpus 65 and inspect**

```bash
tests/run_corpus.sh --filter 65 --verbose 2>&1 | tail -20
```

Open the generated PDF and confirm the deleted Ablation row (3 cells) appears with red strikethrough in its original middle-row position.

If the row is missing from the output, check `apply_changed_descendants` and `rebuild_realized_grid_with_cells` — the splice-back path failed.

- [ ] **Step 4: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add corpus 65 integration test validating deleted cell splice-back"
```

---

## Task 11: Add corpus 69 integration test (nested list)

Corpus 69 is a nested list where an inner item is inserted. The outer list is same-shape (2 items → 2 items), but item 0's body contains a nested list that changed. The outer list should produce `HasChangedDescendants`; item 0 should be `HasChangedDescendants` (with its child being the nested list descent); item 1 ("Ship release") should be `Unchanged`.

This validates Task 5's multi-level descent (via `find_slot_bearing_descendant_pair`) and Task 8's content-matching fallback (splicing the patched inner list back into the outer item's `SequenceElem` body).

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write the integration test**

```rust
// Corpus #69: nested list with one inner item inserted.
// Outer list: 2 items (same shape). Item 0 body contains a nested 2→3 item list.
// Validates Task 5 multi-level descent + Task 8 content-matching splice-back.
// Expected:
//   outer list block  → HasChangedDescendants
//   item 0            → HasChangedDescendants (nested descent succeeded)
//   item 0.children[0] (the inner list descent) → HasChangedDescendants
//   item 1            → Unchanged
#[test]
fn nested_list_item_inserted_produces_has_changed_descendants() {
    use typst_diff::diff::NodeStatus;

    let result = diff_annotated_corpus("69-nested-list-item-inserted");

    let list_block = result
        .blocks
        .iter()
        .find(|b| !matches!(b.status, NodeStatus::Unchanged))
        .expect("expected at least one changed block");

    assert!(
        matches!(list_block.status, NodeStatus::HasChangedDescendants),
        "outer list block should be HasChangedDescendants, got {:?}",
        list_block.status
    );
    assert_eq!(
        list_block.children.len(),
        2,
        "outer list should have 2 slot children"
    );

    let item_0 = &list_block.children[0];
    assert!(
        matches!(item_0.status, NodeStatus::HasChangedDescendants),
        "item 0 ('Plan release' + nested list) should be HasChangedDescendants via multi-level descent, got {:?}",
        item_0.status
    );
    assert_eq!(
        item_0.children.len(),
        1,
        "item 0's wrapper should have one inner descent DiffNode"
    );
    assert!(
        matches!(item_0.children[0].status, NodeStatus::HasChangedDescendants),
        "inner descent should be HasChangedDescendants on the nested list"
    );

    let item_1 = &list_block.children[1];
    assert!(
        matches!(item_1.status, NodeStatus::Unchanged),
        "item 1 ('Ship release') should be Unchanged, got {:?}",
        item_1.status
    );
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test nested_list_item_inserted_produces_has_changed_descendants -- --nocapture 2>&1 | tail -20
```

Expected: PASS.

- [ ] **Step 3: Visual smoke test — run corpus 69 and inspect**

```bash
tests/run_corpus.sh --filter 69 --verbose 2>&1 | tail -20
```

Open the generated PDF. Expected:
- "Ship release" item: normal black (unchanged).
- "Plan release" item: text normal black, nested sublist shows "Review risks" in green (inserted), other inner items in black.

- [ ] **Step 4: Run full test suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add corpus 69 nested list integration test"
```

---

## Task 12: Add corpus 35 and 64 table integration tests

Tables realize to `BlockElem(GridElem(cells))` just like lists, so the same code paths apply. Two new integration tests:

- **Corpus 35**: same-shape part + inserted rows. Validates per-cell statuses (Modified, Inserted) in a table.
- **Corpus 64**: row inserted in the middle (3 cells inserted, 12 cells unchanged). Validates LCS path on tables.

These tests check the `DiffNode` tree shape only — visual correctness is handled by `tests/run_corpus.sh`.

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write the integration tests**

```rust
// Corpus #35: 3-row × 3-col table (9 body cells + 3 header cells) → 4-row × 3-col
// table (12 body cells + 3 header cells). Cells "Proposed" → "Proposed v1" modified;
// new "Proposed v2" row inserted; "0.85" cells modified to differ on numbers.
// Validates per-cell statuses and that headers stay unchanged.
#[test]
fn table_changed_produces_per_cell_statuses() {
    use typst_diff::diff::NodeStatus;

    let result = diff_annotated_corpus("35-table-changed");

    let table_block = result
        .blocks
        .iter()
        .find(|b| !matches!(b.status, NodeStatus::Unchanged))
        .expect("expected at least one changed block");

    assert!(
        matches!(table_block.status, NodeStatus::HasChangedDescendants),
        "table block should be HasChangedDescendants, got {:?}",
        table_block.status
    );

    // Table has at least one Inserted (the v2 row's 3 cells) and at least one
    // Modified or Unchanged. Don't pin exact counts here because LCS pairing
    // depends on cell content hashes; just verify the categories exist.
    let inserted_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Inserted))
        .count();
    assert!(
        inserted_count >= 3,
        "expected at least 3 inserted cells (new v2 row), got {inserted_count}"
    );

    let unchanged_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Unchanged))
        .count();
    assert!(
        unchanged_count >= 3,
        "expected at least 3 unchanged cells (e.g. headers), got {unchanged_count}"
    );
}

// Corpus #64: 4-row × 3-col table (12 cells) → 5-row × 3-col table (15 cells).
// Validates LCS path on a table — 12 unchanged cells, 3 inserted cells for new row.
#[test]
fn table_row_inserted_produces_per_cell_statuses() {
    use typst_diff::diff::NodeStatus;

    let result = diff_annotated_corpus("64-table-row-inserted-middle");

    let table_block = result
        .blocks
        .iter()
        .find(|b| !matches!(b.status, NodeStatus::Unchanged))
        .expect("expected at least one changed block");

    assert!(
        matches!(table_block.status, NodeStatus::HasChangedDescendants),
        "table block should be HasChangedDescendants, got {:?}",
        table_block.status
    );

    let inserted_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Inserted))
        .count();
    assert_eq!(
        inserted_count, 3,
        "expected exactly 3 inserted cells (new Ablation row), got {inserted_count}"
    );

    let unchanged_count = table_block
        .children
        .iter()
        .filter(|c| matches!(c.status, NodeStatus::Unchanged))
        .count();
    assert_eq!(
        unchanged_count, 12,
        "expected 12 unchanged cells (original 4 rows × 3 cols), got {unchanged_count}"
    );
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test table_changed_produces_per_cell_statuses table_row_inserted_produces_per_cell_statuses -- --nocapture 2>&1 | tail -40
```

Expected: both PASS.

If FAIL, inspect actual statuses. Likely causes:
- Tables don't realize to `GridElem` directly — needs investigation in `eval.rs` / `annotated.rs`.
- The table's `semantic_kind` is something other than `Table` — adjust `can_recurse_via_slots` if needed (currently it accepts any kind except Equation, so Tables should be fine).

- [ ] **Step 3: Run full test suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add corpus 35 and 64 table integration tests"
```

---

## Task 13: Re-baseline visual corpus references

Corpus 18, 19, 35, 64, 65, 69 now produce different visual output than the Phase A baseline (per-item/per-cell colors instead of whole-block colors; deleted cells spliced back with strikethrough). The reference PNGs in each corpus's `ref/` directory need to be regenerated and **visually inspected** before committing.

`tests/run_corpus.sh` writes per-page PNGs to each corpus's `ref/` when invoked with `--update-refs`. Without `--update-refs`, it compares fresh PNGs against the existing `ref/page-*.png` files.

**Files:**
- Modify: `tests/corpus/18-list-item-changed/ref/`
- Modify: `tests/corpus/19-list-item-added/ref/`
- Modify: `tests/corpus/35-table-changed/ref/`
- Modify: `tests/corpus/64-table-row-inserted-middle/ref/`
- Modify: `tests/corpus/65-table-row-deleted-middle/ref/`
- Modify: `tests/corpus/69-nested-list-item-inserted/ref/`

- [ ] **Step 1: Run the affected corpora and check what currently fails**

```bash
for n in 18 19 35 64 65 69; do
  tests/run_corpus.sh --filter $n 2>&1 | tail -5
done
```

Note which ones FAIL (some may PASS by coincidence if visual output happens to match — unlikely but possible).

- [ ] **Step 2: Generate fresh output PDFs for visual inspection (do NOT --update-refs yet)**

Inspect the generated PDF for each corpus. `run_corpus.sh` writes the diff PDF to a working directory — check the script output for the exact path (look for "wrote PDF:" or similar). You can also re-derive the path: the script invokes the `typst-diff` binary and the PDF path is shown in stdout when `--verbose` is on.

```bash
tests/run_corpus.sh --filter 18 --verbose 2>&1 | grep -i 'pdf\|out\|wrote' | head -5
```

For each corpus visually verify the diff PDF:
- **18 (list item changed):** items 0, 2, 3 in normal black; item 1 shows "Old feature…" red strikethrough + "New feature…" green.
- **19 (list item added):** items 0, 1, 2 in normal black; item 3 ("Stable internet connection for updates") in green.
- **35 (table changed):** unchanged cells in black; modified "Proposed v1" + numerical cells in red/green; entire "Proposed v2" row in green.
- **64 (table row inserted):** unchanged 12 cells in black; entire "Ablation" row (3 cells) in green between Baseline and Proposed.
- **65 (table row deleted):** unchanged 12 cells in black; entire "Ablation" row (3 cells) in red strikethrough in its original middle-row position.
- **69 (nested list item inserted):** outer items both in black; nested list under "Plan release" has "Review risks" in green.

If any output looks wrong, STOP and investigate before re-baselining.

- [ ] **Step 3: Update references for the 6 corpora**

```bash
for n in 18 19 35 64 65 69; do
  tests/run_corpus.sh --filter $n --update-refs 2>&1 | tail -3
done
```

This rewrites `tests/corpus/NN-…/ref/page-*.png` for each.

- [ ] **Step 4: Run the full corpus suite to confirm no regressions in other corpora**

```bash
tests/run_corpus.sh 2>&1 | tail -20
```

Expected: all corpus tests report PASS (including the newly baselined 18, 19, 35, 64, 65, 69). Investigate any new failures — Phase B may have regressed other corpora.

- [ ] **Step 5: Commit**

```bash
git add tests/corpus/18-list-item-changed/ref \
        tests/corpus/19-list-item-added/ref \
        tests/corpus/35-table-changed/ref \
        tests/corpus/64-table-row-inserted-middle/ref \
        tests/corpus/65-table-row-deleted-middle/ref \
        tests/corpus/69-nested-list-item-inserted/ref
git commit -m "test(corpus): re-baseline corpus 18, 19, 35, 64, 65, 69 for per-cell slot diff output"
```

---

## Self-Review

### Spec coverage

| Spec requirement | Task |
|---|---|
| Tier-2 same-shape container → `HasChangedDescendants` | Tasks 5, 6 |
| Tier-3 different-shape → slot-level LCS → `HasChangedDescendants` | Task 9 |
| Deleted slot children spliced back as strikethrough cells | Tasks 7, 8, 10 |
| Nested slot-bearing containers reached via multi-level descent | Tasks 4, 5, 8, 11 |
| Table cells use same code path as list items | Tasks 7, 8, 12 |
| Corpus 18 passes (same-shape list, one item changed) | Task 6 |
| Corpus 19 passes (list with item added) | Task 9 |
| Corpus 65 passes (table row deleted — deletes spliced back) | Tasks 8, 10 |
| Corpus 69 passes (nested list — multi-level descent) | Tasks 5, 8, 11 |
| Corpus 35/64 pass (table per-cell statuses) | Task 12 |
| `apply_changed_descendants` patches realized container correctly | Tasks 7, 8 |
| `page_styles` plumbed through `DiffNode` | Task 3 |

### Known limitations

- **Grids with explicit `table.header(...)` / `table.footer(...)`**: the rebuild path preserves header/footer cells verbatim from the original grid and only rewrites body-cell bodies. Cell edits inside an explicit header/footer are not annotated. None of our current corpus uses explicit headers, so this isn't exercised.
- **Multiple slot-bearing descendants in one item body**: `find_slot_bearing_descendant_pair` requires exactly one descendant on each side. If an item body contains two nested lists and both change, we fall back to word diff. Conservative for now.
- **Equations**: still atomic — `can_recurse_via_slots` returns false for `SemanticKind::Equation`. Math inside equations gets a CancelElem / green coloring at the whole-equation level, not per-sub-expression. (Out of Phase B scope.)
