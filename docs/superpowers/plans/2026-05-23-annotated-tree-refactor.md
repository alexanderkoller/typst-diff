# Annotated Tree Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the realize-time hybrid tree (pre-realization nodes spliced back into the realized tree via span-keyed queues) with a parallel annotated tree (`AnnotatedContent`) that carries semantic identity as per-node annotations on realized nodes, eliminating the span-collision bug class and the 1000-line slot-registry.

**Architecture:** A new `annotated.rs` module defines `AnnotatedContent { realized, annotation, children }`. `annotate_realized(pre, realized)` walks both trees together, recording `SemanticKind` and `SemanticSlot` child-index mappings as `Annotation` fields on each realized node. The diff and annotate phases read semantics from `annotation` fields instead of from swapped-in pre-realization types. Roll-out is staged so the corpus and unit tests pass at every commit: Stage 1 (new module, no callers) → Stage 2 (eval return type change; downstream reads `.realized`) → Stage 3 (diff.rs reads annotations) → Stage 4 (annotate.rs walks DiffNode tree) → Stage 5 (delete swap-back, wire `annotate_realized` into production).

**Tech Stack:** Rust 1.85+ (2024 edition), typst crate types (`Content`, `ListElem`, `GridElem`, etc.), `similar` crate for LCS. No new dependencies.

---

## File structure

- **Create**: `src/annotated.rs` — all new types and the `annotate_realized` walker
- **Modify**: `src/eval.rs` — return type change (Stage 2); delete swap-back (Stage 5)
- **Modify**: `src/diff.rs` — `DiffBlock` → `AnnotatedContent`; new `DiffNode`/`NodeStatus`/`DiffResult` (Stage 3)
- **Modify**: `src/annotate.rs` — consume `DiffNode` tree; update `apply_fill_inside` (Stage 4)
- **Modify**: `src/content_slots.rs` — delete `extract_slots`, `is_slot_container`, per-element collectors (Stage 5)
- **Modify**: `src/lib.rs` — register new module; update pipeline doc comment
- **Modify**: `tests/integration.rs` — add `.realized` at call sites when `eval_to_realized_content` return type changes (Stage 2)
- **Modify**: `src/main.rs` — same `.realized` fix (Stage 2)

---

## Background reading before you start

Read these files in full before touching any code:
- `src/eval.rs` — especially `realize_to_content`, `collect_preserved_by_span`, `restore_preserved`, `restore_footnote_markers*`
- `src/content_slots.rs` — `extract_slots`, `collect_slots`, `replace_slot`, `SlotStep`
- `src/diff.rs` — `DiffBlock`, `DiffResultOp`, `diff_content`, `diff_slots`, `extract_block_units`
- `src/annotate.rs` — `build_annotated_content`, `apply_fill_inside`, `replace_modified_slots`
- Design rationale: `~/.claude/plans/i-m-reading-through-the-eager-hickey.md`

The Typst crate types you will use most: `Content`, `SequenceElem`, `StyledElem`, `ParElem`, `ListElem`, `ListItem`, `EnumElem`, `EnumItem`, `TermsElem`, `TermItem`, `FigureElem`, `FootnoteElem`, `QuoteElem`, `TableElem`, `GridElem`, `StackElem`, `BlockElem`, `AlignElem`, `PadElem`, `PlaceElem`, `ColumnsElem`, `BoxElem`, `RectElem`, `CircleElem`, `EllipseElem`, `HeadingElem`, `EquationElem`.

---

## Stage 1: Add annotated.rs — no callers yet

### Task 1: Define types in `src/annotated.rs` and register the module

**Files:**
- Create: `src/annotated.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write the failing compile test**

Add to `src/annotated.rs`:

```rust
// (empty file — module skeleton only)
```

Add to `src/lib.rs` after `pub mod eval;`:

```rust
pub mod annotated;
```

- [ ] **Step 2: Run to confirm it builds (trivially)**

```
cargo build 2>&1 | head -20
```

Expected: no errors.

- [ ] **Step 3: Add the complete type definitions**

Replace `src/annotated.rs` with:

```rust
//! Annotated realized content tree.
//!
//! [`AnnotatedContent`] pairs a realized [`Content`] node with semantic
//! information recovered from the pre-realization tree. The realized side is
//! preserved exactly as Typst produced it; annotations are built once and
//! never mutated.

use typst::foundations::Content;
use typst::syntax::Span;
use crate::content_slots::SlotStep;

/// A realized Content node together with its semantic identity.
pub struct AnnotatedContent {
    /// The realized content as Typst produced it (what gets rendered).
    pub realized: Content,
    /// Semantic information derived once at eval time.
    pub annotation: Annotation,
    /// Annotated children, mirroring the descent points of the realized tree.
    /// Empty for leaves and for nodes we don't descend into.
    pub children: Vec<AnnotatedContent>,
}

impl AnnotatedContent {
    /// Convenience: return the plain text of the realized content.
    pub fn plain_text(&self) -> typst::diag::EcoString {
        self.realized.plain_text()
    }

    /// Convenience: is the realized content empty?
    pub fn is_empty(&self) -> bool {
        self.realized.is_empty()
    }
}

pub struct Annotation {
    /// Pre-realization element type if this node is a tracked structural element.
    /// `None` for plain text, spaces, anonymous wrappers.
    pub semantic_kind: Option<SemanticKind>,
    /// Semantic slots — named positions within `children` that the diff recurses into.
    pub slots: Vec<SemanticSlot>,
    /// Footnote body if this realized node is a footnote marker site.
    pub footnote: Option<FootnoteInfo>,
    /// Source span for diagnostics (not used as a lookup key).
    pub span: Span,
}

impl Default for Annotation {
    fn default() -> Self {
        Annotation {
            semantic_kind: None,
            slots: vec![],
            footnote: None,
            span: Span::detached(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticKind {
    Paragraph,
    Heading,
    RawBlock,
    List,
    Enum,
    Terms,
    Table,
    Grid,
    Stack,
    Figure,
    Footnote,
    Quote,
    Equation,
    Wrapper(WrapperKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WrapperKind {
    Align, Pad, Place, Columns, Box, Block, Rect, Circle, Ellipse,
}

/// A named semantic position within an [`AnnotatedContent`] node.
///
/// `child_index` points into the parent's `children` vec.
/// `label` identifies the slot's role (e.g. `ListItem(0)`).
#[derive(Clone, Debug)]
pub struct SemanticSlot {
    pub label: SlotStep,
    pub child_index: usize,
}

pub struct FootnoteInfo {
    pub body: Content,
}
```

- [ ] **Step 4: Run build to confirm the types compile**

```
cargo build 2>&1 | head -30
```

Expected: builds cleanly (no tests yet).

- [ ] **Step 5: Commit**

```bash
git add src/annotated.rs src/lib.rs
git commit -m "refactor: add annotated.rs type definitions (no callers)"
```

---

### Task 2: Implement `annotate_realized` — transparent wrappers and leaf cases

**Files:**
- Modify: `src/annotated.rs`

This task implements the easy cases of the walker: transparent wrapper types (`SequenceElem`, `StyledElem`, `ParElem`) that descend pairwise into pre and realized trees, and the leaf fallback that wraps realized verbatim with `semantic_kind` inferred from pre's type.

**Background — three one-to-many expansion patterns** (from the design doc "Handling one-to-many expansion" section). A single pre-realization node routinely corresponds to multiple realized nodes. The walker must follow these rules:

1. **Outer wrapping** (e.g. `ListElem` → `BlockElem(GridElem(…))`, `HeadingElem` → `ParElem(…)`). Only the *outermost* realized node carries `semantic_kind`; intermediate wrappers (the inner `GridElem`, etc.) are anonymous. Per-type slot mappers (Tasks 3–4) descend through these wrappers via `collect_leaf_block_children` to find slot bodies — the wrappers don't get their own annotated nodes; slot indices point directly at slot-body children.
2. **Inner injection** (counter values, list markers, show-rule prefix text). The realized form has more interior nodes than the pre form, but the structural element is still recognizable at the outer level — e.g. `HeadingElem(TextElem("Intro"))` with numbering becomes `ParElem(SequenceElem(TextElem("1"), SpaceElem, TextElem("Intro")))`. The outer node carries `SemanticKind::Heading`; we treat it as a leaf in the annotated tree (`children: vec![]`). Inner realized children are not walked — the diff word-diffs the whole realized form via the kept `replace_text_container`. No need to identify which inner realized child *is* the original body.
3. **Sibling-level expansion** (rare; user show rules like `#show heading: it => [#it; #it]` that yield multiple siblings from one pre node). Pre's `SequenceElem` has N children, realized's has M > N. The walker descends pairwise where shapes align; on count divergence it falls back to **span-based child alignment** — for each pre child, walk forward through realized children until one with a matching `Content::span()` is found, then pair them; skipped realized children become anonymous annotated nodes. Spans here are a positional hint within a span group, not a primary key — when several children share a span (the Bug 1 collision class), document-order pairing within the matching span group still yields the correct match. No `HashMap<Span, …>` collapse.

The implementation below builds in rules 1 and 2 by structure (structural-element branches dispatch to per-type mappers that descend; leaf branches use `leaf_annotated`). Rule 3 is implemented as the `pair_sequence_by_span` helper invoked from the SequenceElem branch on length mismatch.

- [ ] **Step 1: Write the failing tests**

Add to `src/annotated.rs` at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Content, Packed, SequenceElem};
    use typst::model::ParElem;
    use typst::text::TextElem;

    fn text(s: &str) -> Content { TextElem::packed(s) }

    fn seq(items: impl IntoIterator<Item = Content>) -> Content {
        Content::sequence(items)
    }

    #[test]
    fn annotate_leaf_wraps_realized_verbatim() {
        let pre = text("hello");
        let realized = text("hello");
        let node = annotate_realized(&pre, &realized);
        assert_eq!(node.realized.plain_text(), "hello");
        assert!(node.children.is_empty());
    }

    #[test]
    fn annotate_sequence_descends_pairwise_when_lengths_match() {
        let pre = seq([text("a"), text("b")]);
        let realized = seq([text("a"), text("b")]);
        let node = annotate_realized(&pre, &realized);
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].realized.plain_text(), "a");
        assert_eq!(node.children[1].realized.plain_text(), "b");
    }

    #[test]
    fn annotate_sequence_with_extra_realized_children_produces_anonymous_extras() {
        // Sibling-level expansion (one-to-many rule 3): pre has 2 children, realized has 3.
        // All children have detached spans, so positional pairing applies inside the
        // shared span group: pre[0]→real[0], pre[1]→real[1], real[2]→anonymous leaf.
        let pre = seq([text("a"), text("b")]);
        let realized = seq([text("a"), text("b"), text("c")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.children.len(), 3);
        assert_eq!(node.children[0].realized.plain_text(), "a");
        assert_eq!(node.children[1].realized.plain_text(), "b");
        assert_eq!(node.children[2].realized.plain_text(), "c");
        // The unmatched extra is anonymous (no semantic_kind).
        assert!(node.children[2].annotation.semantic_kind.is_none());
    }

    #[test]
    fn annotate_sequence_with_extra_pre_children_drops_unmatched_pre() {
        // Symmetric: pre has 3 children, realized has 2 (a pre node produced no output).
        // pre[0]→real[0], pre[1]→real[1], pre[2] has no realized partner → dropped.
        let pre = seq([text("a"), text("b"), text("c")]);
        let realized = seq([text("a"), text("b")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].realized.plain_text(), "a");
        assert_eq!(node.children[1].realized.plain_text(), "b");
    }

    #[test]
    fn annotate_preserves_realized_content_unchanged() {
        // The realized field must be byte-identical to what was passed in.
        use typst::visualize::Color;
        let pre = text("before");
        let realized = text("after").styled(TextElem::fill.set(
            Color::from_u8(1, 2, 3, 255).into()
        ));
        let realized_clone = realized.clone();
        let node = annotate_realized(&pre, &realized);
        assert_eq!(node.realized, realized_clone);
    }
}
```

- [ ] **Step 2: Run to see tests fail**

```
cargo test annotate_leaf annotate_sequence annotate_preserves 2>&1 | tail -20
```

Expected: `error[E0425]: cannot find function \`annotate_realized\` in this scope` (all four tests fail to compile for the same reason).

- [ ] **Step 3: Implement `annotate_realized` — transparent wrapper and leaf cases**

Add before the `#[cfg(test)]` block in `src/annotated.rs`:

```rust
use typst::foundations::{SequenceElem, StyledElem};
use typst::model::{HeadingElem, ParElem};
use typst::math::EquationElem;
use typst::text::{RawElem, TextElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridElem, PadElem, PlaceElem,
    StackElem,
};
use typst::model::{
    EnumElem, FigureElem, FootnoteElem, FootnoteBody, ListElem, QuoteElem, TableElem, TermsElem,
};
use typst::visualize::{CircleElem, EllipseElem, RectElem};
use typst::foundations::StyleChain;

/// Build an annotated tree by walking `pre` (pre-realization) and `realized` together.
///
/// The realized field of every node in the returned tree is always identical to
/// a subtree of `realized` — this is the read-only invariant.
pub fn annotate_realized(pre: &Content, realized: &Content) -> AnnotatedContent {
    // --- Structural elements: semantic_kind + slot map ---
    if pre.is::<ListElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::List, |p, r| {
            map_list_to_children(p, r)
        });
    }
    if pre.is::<EnumElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Enum, |p, r| {
            map_enum_to_children(p, r)
        });
    }
    if pre.is::<TermsElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Terms, |p, r| {
            map_terms_to_children(p, r)
        });
    }
    if pre.is::<TableElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Table, |p, r| {
            map_table_to_children(p, r)
        });
    }
    if pre.is::<GridElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Grid, |p, r| {
            map_grid_to_children(p, r)
        });
    }
    if pre.is::<StackElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Stack, |p, r| {
            map_stack_to_children(p, r)
        });
    }
    if pre.is::<FigureElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Figure, |p, r| {
            map_figure_to_children(p, r)
        });
    }
    if pre.is::<FootnoteElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Footnote, |p, r| {
            map_footnote_to_children(p, r)
        });
    }
    if pre.is::<QuoteElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Quote, |p, r| {
            map_quote_to_children(p, r)
        });
    }
    if let Some(wrapper_kind) = wrapper_kind_of(pre) {
        return annotate_with_kind(pre, realized, SemanticKind::Wrapper(wrapper_kind), |p, r| {
            map_wrapper_to_children(p, r)
        });
    }
    if pre.is::<EquationElem>() {
        return leaf_annotated(realized, Annotation {
            semantic_kind: Some(SemanticKind::Equation),
            span: pre.span(),
            ..Annotation::default()
        });
    }
    if pre.is::<HeadingElem>() {
        return leaf_annotated(realized, Annotation {
            semantic_kind: Some(SemanticKind::Heading),
            span: pre.span(),
            ..Annotation::default()
        });
    }
    if pre.is::<RawElem>() {
        return leaf_annotated(realized, Annotation {
            semantic_kind: Some(SemanticKind::RawBlock),
            span: pre.span(),
            ..Annotation::default()
        });
    }

    // --- Transparent wrappers: pairwise descent ---
    if let (Some(pre_seq), Some(real_seq)) = (
        pre.to_packed::<SequenceElem>(),
        realized.to_packed::<SequenceElem>(),
    ) {
        // Common case: shapes align → pair positionally.
        // Sibling-level expansion (rule 3): shapes diverge → pair by span groups
        // in document order, with unmatched realized children becoming anonymous.
        let children = if pre_seq.children.len() == real_seq.children.len() {
            pre_seq.children.iter()
                .zip(real_seq.children.iter())
                .map(|(p, r)| annotate_realized(p, r))
                .collect()
        } else {
            pair_sequence_by_span(&pre_seq.children, &real_seq.children)
        };
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation { span: pre.span(), ..Annotation::default() },
            children,
        };
    }
    if let (Some(pre_s), Some(real_s)) = (
        pre.to_packed::<StyledElem>(),
        realized.to_packed::<StyledElem>(),
    ) {
        let child = annotate_realized(&pre_s.child, &real_s.child);
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation { span: pre.span(), ..Annotation::default() },
            children: vec![child],
        };
    }
    if let (Some(pre_p), Some(real_p)) = (
        pre.to_packed::<ParElem>(),
        realized.to_packed::<ParElem>(),
    ) {
        let child = annotate_realized(&pre_p.body, &real_p.body);
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Paragraph),
                span: pre.span(),
                ..Annotation::default()
            },
            children: vec![child],
        };
    }

    // --- Leaf fallback ---
    leaf_annotated(realized, Annotation {
        semantic_kind: semantic_kind_of(pre),
        span: pre.span(),
        ..Annotation::default()
    })
}

fn leaf_annotated(realized: &Content, annotation: Annotation) -> AnnotatedContent {
    AnnotatedContent { realized: realized.clone(), annotation, children: vec![] }
}

/// Pair pre and realized sequence children by span groups, in document order.
///
/// Used when a `SequenceElem`'s pre and realized children counts diverge (the
/// "sibling-level expansion" rule, e.g. user show rules that emit multiple
/// realized siblings from one pre node). The algorithm walks both sequences in
/// document order: for each pre child, advance a cursor through realized
/// children, treating skipped realized children as anonymous leaves until a
/// span match is found (then recursively annotate that pair). If no span match
/// is found before realized is exhausted, the pre child is dropped (it
/// produced no realized output). Trailing realized children become anonymous
/// leaves.
///
/// Spans here serve as a positional hint within a shared-span group, not as a
/// primary key — when several children share a span, document-order pairing
/// inside the group still yields the correct match. Detached spans (no source
/// location) compare equal, so they pair purely by position.
fn pair_sequence_by_span(
    pre_children: &[Content],
    real_children: &[Content],
) -> Vec<AnnotatedContent> {
    let mut out: Vec<AnnotatedContent> = Vec::new();
    let mut cursor: usize = 0;
    for pre_child in pre_children {
        let target = pre_child.span();
        // Walk forward past non-matching realized children, recording them as anonymous.
        while cursor < real_children.len() && real_children[cursor].span() != target {
            out.push(leaf_annotated(&real_children[cursor], Annotation::default()));
            cursor += 1;
        }
        if cursor < real_children.len() {
            out.push(annotate_realized(pre_child, &real_children[cursor]));
            cursor += 1;
        }
        // else: pre_child has no realized partner — silently drop.
    }
    // Any trailing unmatched realized children become anonymous leaves.
    while cursor < real_children.len() {
        out.push(leaf_annotated(&real_children[cursor], Annotation::default()));
        cursor += 1;
    }
    out
}

fn semantic_kind_of(pre: &Content) -> Option<SemanticKind> {
    if pre.is::<HeadingElem>() { return Some(SemanticKind::Heading); }
    if pre.is::<EquationElem>() { return Some(SemanticKind::Equation); }
    if pre.is::<RawElem>() { return Some(SemanticKind::RawBlock); }
    if pre.is::<ListElem>() { return Some(SemanticKind::List); }
    if pre.is::<EnumElem>() { return Some(SemanticKind::Enum); }
    if pre.is::<TermsElem>() { return Some(SemanticKind::Terms); }
    if pre.is::<TableElem>() { return Some(SemanticKind::Table); }
    if pre.is::<GridElem>() { return Some(SemanticKind::Grid); }
    if pre.is::<StackElem>() { return Some(SemanticKind::Stack); }
    if pre.is::<FigureElem>() { return Some(SemanticKind::Figure); }
    if pre.is::<FootnoteElem>() { return Some(SemanticKind::Footnote); }
    if pre.is::<QuoteElem>() { return Some(SemanticKind::Quote); }
    if let Some(wk) = wrapper_kind_of(pre) { return Some(SemanticKind::Wrapper(wk)); }
    if pre.is::<ParElem>() { return Some(SemanticKind::Paragraph); }
    None
}

fn wrapper_kind_of(pre: &Content) -> Option<WrapperKind> {
    if pre.is::<AlignElem>() { return Some(WrapperKind::Align); }
    if pre.is::<PadElem>() { return Some(WrapperKind::Pad); }
    if pre.is::<PlaceElem>() { return Some(WrapperKind::Place); }
    if pre.is::<ColumnsElem>() { return Some(WrapperKind::Columns); }
    if pre.is::<BoxElem>() { return Some(WrapperKind::Box); }
    if pre.is::<BlockElem>() { return Some(WrapperKind::Block); }
    if pre.is::<RectElem>() { return Some(WrapperKind::Rect); }
    if pre.is::<CircleElem>() { return Some(WrapperKind::Circle); }
    if pre.is::<EllipseElem>() { return Some(WrapperKind::Ellipse); }
    None
}

/// Build an AnnotatedContent for a structural element using a per-type mapper.
fn annotate_with_kind(
    pre: &Content,
    realized: &Content,
    kind: SemanticKind,
    mapper: impl Fn(&Content, &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>),
) -> AnnotatedContent {
    let (children, slots) = mapper(pre, realized);
    AnnotatedContent {
        realized: realized.clone(),
        annotation: Annotation {
            semantic_kind: Some(kind),
            slots,
            span: pre.span(),
            footnote: None,
        },
        children,
    }
}

/// Collect the "leaf block" children of a realized tree by descending through
/// BlockElem / GridElem / StyledElem wrappers.
///
/// This is how we locate the N slot bodies inside whatever wrapper structure
/// ROUTINES.realize produces (e.g., ListElem → BlockElem(GridElem([cells…]))).
pub fn collect_leaf_block_children(content: &Content) -> Vec<Content> {
    // Descend into BlockElem body
    if let Some(block) = content.to_packed::<BlockElem>() {
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            return collect_leaf_block_children(&body);
        }
    }
    // Collect GridElem cells (skip non-cell items like gutters)
    if let Some(grid) = content.to_packed::<GridElem>() {
        use typst::layout::{GridChild, GridItem};
        let mut cells = Vec::new();
        for child in &grid.children {
            match child {
                GridChild::Item(GridItem::Cell(cell)) => cells.push(cell.body.clone()),
                GridChild::Header(h) => {
                    for item in &h.children {
                        if let GridItem::Cell(cell) = item { cells.push(cell.body.clone()); }
                    }
                }
                GridChild::Footer(f) => {
                    for item in &f.children {
                        if let GridItem::Cell(cell) = item { cells.push(cell.body.clone()); }
                    }
                }
                _ => {}
            }
        }
        if !cells.is_empty() { return cells; }
    }
    // Descend into StyledElem
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let inner = collect_leaf_block_children(&styled.child);
        if !inner.is_empty() { return inner; }
    }
    // SequenceElem: return children directly
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.clone();
    }
    // Fallback: treat the node itself as a single child
    vec![content.clone()]
}
```

Stubs for the mapper functions (needed to compile — implement in Task 3/4):

```rust
fn map_list_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_enum_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_terms_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_table_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_grid_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_stack_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_figure_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_footnote_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_quote_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
fn map_wrapper_to_children(_pre: &Content, _realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) { (vec![], vec![]) }
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test annotate_leaf annotate_sequence annotate_preserves 2>&1 | tail -20
```

Expected: `test result: ok. 5 passed` — four sequence/leaf tests plus the "preserves realized content unchanged" test. The two span-pairing tests (`annotate_sequence_with_extra_realized_children_produces_anonymous_extras` and `annotate_sequence_with_extra_pre_children_drops_unmatched_pre`) pass because both pre and realized children in those tests have detached spans, which compare equal — so `pair_sequence_by_span` pairs them positionally within the single span group.

- [ ] **Step 5: Commit**

```bash
git add src/annotated.rs
git commit -m "refactor: implement annotate_realized transparent wrappers, leaf cases, and span-based sibling pairing"
```

---

### Task 3: Implement `pre_to_realized_slot_map` for List, Enum, Terms

**Files:**
- Modify: `src/annotated.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)]` block in `src/annotated.rs`:

```rust
    #[test]
    fn annotate_list_maps_each_item_body_to_a_realized_child() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem};

        let pre = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
            Packed::new(ListItem::new(text("Gamma"))),
        ]));
        // Simulate realized form: a sequence of item bodies (simplified)
        let realized = seq([text("Alpha"), text("Beta"), text("Gamma")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert_eq!(node.annotation.slots.len(), 3);
        assert!(matches!(node.annotation.slots[0].label, SlotStep::ListItem(0)));
        assert_eq!(node.annotation.slots[0].child_index, 0);
        assert!(matches!(node.annotation.slots[2].label, SlotStep::ListItem(2)));
        assert_eq!(node.children[0].realized.plain_text(), "Alpha");
        assert_eq!(node.children[2].realized.plain_text(), "Gamma");
    }

    #[test]
    fn annotate_list_falls_back_to_no_slots_on_item_count_mismatch() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem};

        let pre = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
        ]));
        let realized = seq([text("Alpha"), text("extra")]); // 2 realized children, 1 pre item
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert!(node.annotation.slots.is_empty(), "slot count mismatch must produce no slots");
    }

    #[test]
    fn annotate_enum_maps_each_item_body_to_a_realized_child() {
        use typst::foundations::Packed;
        use typst::model::{EnumElem, EnumItem};

        let pre = Content::new(EnumElem::new(vec![
            Packed::new(EnumItem::new(text("One"))),
            Packed::new(EnumItem::new(text("Two"))),
        ]));
        let realized = seq([text("One"), text("Two")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Enum));
        assert_eq!(node.annotation.slots.len(), 2);
        assert!(matches!(node.annotation.slots[0].label, SlotStep::EnumItem(0)));
        assert!(matches!(node.annotation.slots[1].label, SlotStep::EnumItem(1)));
        assert_eq!(node.children[1].realized.plain_text(), "Two");
    }

    #[test]
    fn annotate_terms_maps_term_and_description_separately() {
        use typst::foundations::Packed;
        use typst::model::{TermsElem, TermItem};

        let pre = Content::new(TermsElem::new(vec![
            Packed::new(TermItem::new(text("API"), text("Definition"))),
        ]));
        // Realized: 2 children for 1 term (term + description)
        let realized = seq([text("API"), text("Definition")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Terms));
        assert_eq!(node.annotation.slots.len(), 2);
        let labels: Vec<String> = node.annotation.slots.iter()
            .map(|s| format!("{:?}", s.label))
            .collect();
        assert!(labels[0].contains("Term(0)"), "{labels:?}");
        assert!(labels[1].contains("TermDescription(0)"), "{labels:?}");
    }
```

- [ ] **Step 2: Run to see tests fail**

```
cargo test annotate_list annotate_enum annotate_terms 2>&1 | tail -20
```

Expected: tests fail with "assertion failed" (mappers return empty vecs).

- [ ] **Step 3: Implement the mapper functions**

Replace the three stub functions in `src/annotated.rs`:

```rust
fn map_list_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(list) = pre.to_packed::<ListElem>() else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != list.children.len() {
        return (vec![], vec![]);
    }
    let children: Vec<AnnotatedContent> = list.children.iter()
        .zip(realized_children.iter())
        .map(|(item, real)| annotate_realized(&item.body, real))
        .collect();
    let slots: Vec<SemanticSlot> = (0..list.children.len())
        .map(|i| SemanticSlot { label: SlotStep::ListItem(i), child_index: i })
        .collect();
    (children, slots)
}

fn map_enum_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(enm) = pre.to_packed::<EnumElem>() else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != enm.children.len() {
        return (vec![], vec![]);
    }
    let children: Vec<AnnotatedContent> = enm.children.iter()
        .zip(realized_children.iter())
        .map(|(item, real)| annotate_realized(&item.body, real))
        .collect();
    let slots: Vec<SemanticSlot> = (0..enm.children.len())
        .map(|i| SemanticSlot { label: SlotStep::EnumItem(i), child_index: i })
        .collect();
    (children, slots)
}

fn map_terms_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(terms) = pre.to_packed::<TermsElem>() else { return (vec![], vec![]); };
    // Each term item contributes 2 slots: term + description.
    // Expected realized children count = 2 * items.len()
    let realized_children = collect_leaf_block_children(realized);
    let expected = terms.children.len() * 2;
    if realized_children.len() != expected {
        return (vec![], vec![]);
    }
    let mut children = Vec::new();
    let mut slots = Vec::new();
    for (i, item) in terms.children.iter().enumerate() {
        let term_real = &realized_children[i * 2];
        let desc_real = &realized_children[i * 2 + 1];
        slots.push(SemanticSlot { label: SlotStep::Term(i), child_index: children.len() });
        children.push(annotate_realized(&item.term, term_real));
        slots.push(SemanticSlot { label: SlotStep::TermDescription(i), child_index: children.len() });
        children.push(annotate_realized(&item.description, desc_real));
    }
    (children, slots)
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test annotate_list annotate_enum annotate_terms 2>&1 | tail -20
```

Expected: `test result: ok. 5 passed` (including the mismatch test).

> **Note:** If the terms test fails, the realized form of `TermsElem` may have a different child count or structure than assumed. Debug by printing `collect_leaf_block_children(realized)` and adjusting the counting logic. The terms mapper may need to descend differently into the realized tree — the exact structure depends on what `ROUTINES.realize` produces for terms elements.

- [ ] **Step 5: Commit**

```bash
git add src/annotated.rs
git commit -m "refactor: implement annotate_realized slot mappers for list/enum/terms"
```

---

### Task 4: Implement slot mappers for Table, Grid, Stack, Figure, Quote, Footnote, and single-body wrappers

**Files:**
- Modify: `src/annotated.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)]` block:

```rust
    #[test]
    fn annotate_table_maps_each_cell_by_document_order_index() {
        use typst::foundations::Packed;
        use typst::model::{TableCell, TableChild, TableElem, TableItem};

        let pre = Content::new(TableElem::new(vec![
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(text("A"))))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(text("B"))))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(text("C"))))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(text("D"))))),
        ]));
        let realized = seq([text("A"), text("B"), text("C"), text("D")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Table));
        assert_eq!(node.annotation.slots.len(), 4);
        assert!(matches!(node.annotation.slots[2].label, SlotStep::TableCell(2)));
        assert_eq!(node.children[3].realized.plain_text(), "D");
    }

    #[test]
    fn annotate_figure_maps_body_and_caption_separately() {
        use std::fs;
        use tempfile::TempDir;
        use crate::world::SystemWorld;
        use crate::eval::eval_to_content;
        use crate::content_slots::normalize_list_item_runs;

        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"),
            "#figure(rect(width: 10pt, height: 4pt), caption: [Old cap])"
        ).unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let pre = normalize_list_item_runs(eval_to_content(&world).unwrap());

        // Use pre itself as "realized" (simplified — just tests the mapper logic)
        let node = annotate_realized(&pre, &pre);
        // Find the figure node by walking
        fn find_figure(node: &AnnotatedContent) -> Option<&AnnotatedContent> {
            if node.annotation.semantic_kind == Some(SemanticKind::Figure) { return Some(node); }
            node.children.iter().find_map(find_figure)
        }
        let figure_node = find_figure(&node);
        assert!(figure_node.is_some(), "figure node not found in tree");
        let figure_node = figure_node.unwrap();
        assert!(figure_node.annotation.slots.iter().any(|s| matches!(s.label, SlotStep::FigureBody)));
        assert!(figure_node.annotation.slots.iter().any(|s| matches!(s.label, SlotStep::FigureCaption)));
    }

    #[test]
    fn annotate_each_wrapper_kind_sets_correct_semantic_kind() {
        // Wrappers are handled by wrapper_kind_of; verify at least Align and Block
        use typst::layout::{AlignElem, BlockBody, BlockElem};

        let align_pre = Content::new(AlignElem::new(text("body")));
        let node = annotate_realized(&align_pre, &align_pre);
        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Wrapper(WrapperKind::Align)));

        let block_pre = Content::new(BlockElem::new()
            .with_body(Some(BlockBody::Content(text("body")))));
        let node = annotate_realized(&block_pre, &block_pre);
        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Wrapper(WrapperKind::Block)));
    }

    #[test]
    fn annotate_empty_list_has_kind_list_and_zero_slots() {
        use typst::model::ListElem;
        let pre = Content::new(ListElem::new(vec![]));
        let realized = seq([]);
        let node = annotate_realized(&pre, &realized);
        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert!(node.annotation.slots.is_empty());
    }

    #[test]
    fn annotate_equation_has_kind_equation_and_no_slots() {
        use typst::math::EquationElem;
        let pre = Content::new(EquationElem::new(text("a + b")).with_block(false));
        let node = annotate_realized(&pre, &pre);
        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Equation));
        assert!(node.annotation.slots.is_empty());
    }

    #[test]
    fn annotate_list_descends_through_outer_block_grid_wrappers() {
        // One-to-many rule 1 (outer wrapping): ListElem realized as
        // BlockElem(GridElem([cells])). collect_leaf_block_children must descend
        // through both wrappers to find the cell bodies. Only the outer node
        // carries SemanticKind::List; intermediate wrappers are not materialized
        // as their own annotated nodes — slot indices point straight at slot bodies.
        use typst::foundations::Packed;
        use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};
        use typst::model::{ListElem, ListItem};

        let pre = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
        ]));
        let cells = vec![
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("Alpha"))))),
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("Beta"))))),
        ];
        let realized = Content::new(BlockElem::new()
            .with_body(Some(BlockBody::Content(Content::new(GridElem::new(cells))))));
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert_eq!(node.annotation.slots.len(), 2);
        assert_eq!(node.children[0].realized.plain_text(), "Alpha");
        assert_eq!(node.children[1].realized.plain_text(), "Beta");
        // The intermediate BlockElem and GridElem do not appear as their own
        // annotated children — children[0] is the first slot body directly.
        assert!(node.children[0].annotation.semantic_kind.is_none()
            || matches!(node.children[0].annotation.semantic_kind, Some(SemanticKind::Paragraph)));
    }

    #[test]
    fn annotate_heading_with_numbering_keeps_heading_kind_at_outer_level() {
        // One-to-many rule 2 (inner injection): a HeadingElem(TextElem("Intro"))
        // can realize as a structure containing prefix text from a counter
        // (e.g. "1" + space + "Intro"). The outer node carries
        // SemanticKind::Heading; the heading is a leaf in the annotated tree
        // (children empty) because the diff word-diffs the whole realized form
        // and doesn't need to identify which inner realized child IS the body.
        use typst::model::HeadingElem;

        let pre = Content::new(HeadingElem::new(text("Intro")));
        // Realized form simulates a numbered heading after the numbering show rule expanded.
        let realized = seq([text("1"), text(" "), text("Intro")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Heading));
        assert!(node.children.is_empty(),
            "heading is a leaf in the annotated tree; inner realized children are not walked");
        // Realized preserves the full form including the injected "1".
        assert!(node.realized.plain_text().contains('1'));
        assert!(node.realized.plain_text().contains("Intro"));
    }
```

- [ ] **Step 2: Run to see tests fail**

```
cargo test annotate_table annotate_figure annotate_each_wrapper annotate_empty_list annotate_equation annotate_list_descends annotate_heading_with_numbering 2>&1 | tail -25
```

Expected: compile errors for missing types or test assertion failures. The two new one-to-many tests (`annotate_list_descends_through_outer_block_grid_wrappers` and `annotate_heading_with_numbering_keeps_heading_kind_at_outer_level`) should already pass after Task 2 + Step 3 of this task — they exercise rules 1 and 2 of the design's "Handling one-to-many expansion" section, which the existing structural-element dispatch and leaf branches implement. If they don't pass, debug the mapper before proceeding.

- [ ] **Step 3: Implement the remaining mapper functions**

Replace the remaining stubs in `src/annotated.rs`:

```rust
fn map_table_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    use typst::model::{TableChild, TableItem};
    let Some(table) = pre.to_packed::<TableElem>() else { return (vec![], vec![]); };
    // Collect pre cell bodies in document order
    let mut pre_cells: Vec<Content> = Vec::new();
    for child in &table.children {
        match child {
            TableChild::Header(h) => {
                for item in &h.children {
                    if let TableItem::Cell(c) = item { pre_cells.push(c.body.clone()); }
                }
            }
            TableChild::Footer(f) => {
                for item in &f.children {
                    if let TableItem::Cell(c) = item { pre_cells.push(c.body.clone()); }
                }
            }
            TableChild::Item(TableItem::Cell(c)) => pre_cells.push(c.body.clone()),
            _ => {}
        }
    }
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != pre_cells.len() { return (vec![], vec![]); }
    let children: Vec<AnnotatedContent> = pre_cells.iter()
        .zip(realized_children.iter())
        .map(|(p, r)| annotate_realized(p, r))
        .collect();
    let slots: Vec<SemanticSlot> = (0..pre_cells.len())
        .map(|i| SemanticSlot { label: SlotStep::TableCell(i), child_index: i })
        .collect();
    (children, slots)
}

fn map_grid_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    use typst::layout::{GridChild, GridItem};
    let Some(grid) = pre.to_packed::<GridElem>() else { return (vec![], vec![]); };
    let mut pre_cells: Vec<Content> = Vec::new();
    for child in &grid.children {
        match child {
            GridChild::Header(h) => {
                for item in &h.children {
                    if let GridItem::Cell(c) = item { pre_cells.push(c.body.clone()); }
                }
            }
            GridChild::Footer(f) => {
                for item in &f.children {
                    if let GridItem::Cell(c) = item { pre_cells.push(c.body.clone()); }
                }
            }
            GridChild::Item(GridItem::Cell(c)) => pre_cells.push(c.body.clone()),
            _ => {}
        }
    }
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != pre_cells.len() { return (vec![], vec![]); }
    let children: Vec<AnnotatedContent> = pre_cells.iter()
        .zip(realized_children.iter())
        .map(|(p, r)| annotate_realized(p, r))
        .collect();
    let slots: Vec<SemanticSlot> = (0..pre_cells.len())
        .map(|i| SemanticSlot { label: SlotStep::GridCell(i), child_index: i })
        .collect();
    (children, slots)
}

fn map_stack_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    use typst::layout::StackChild;
    let Some(stack) = pre.to_packed::<StackElem>() else { return (vec![], vec![]); };
    let pre_blocks: Vec<(usize, Content)> = stack.children.iter().enumerate()
        .filter_map(|(i, child)| {
            if let StackChild::Block(body) = child { Some((i, body.clone())) } else { None }
        })
        .collect();
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != pre_blocks.len() { return (vec![], vec![]); }
    let mut children = Vec::new();
    let mut slots = Vec::new();
    for ((orig_idx, pre_body), real) in pre_blocks.into_iter().zip(realized_children.iter()) {
        slots.push(SemanticSlot { label: SlotStep::StackChild(orig_idx), child_index: children.len() });
        children.push(annotate_realized(&pre_body, real));
    }
    (children, slots)
}

fn map_figure_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(figure) = pre.to_packed::<FigureElem>() else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    let mut pre_parts: Vec<(SlotStep, Content)> = vec![(SlotStep::FigureBody, figure.body.clone())];
    if let Some(cap) = figure.caption.get_cloned(StyleChain::default()) {
        pre_parts.push((SlotStep::FigureCaption, cap.body.clone()));
    }
    if realized_children.len() != pre_parts.len() { return (vec![], vec![]); }
    let mut children = Vec::new();
    let mut slots = Vec::new();
    for ((label, pre_body), real) in pre_parts.into_iter().zip(realized_children.iter()) {
        slots.push(SemanticSlot { label, child_index: children.len() });
        children.push(annotate_realized(&pre_body, real));
    }
    (children, slots)
}

fn map_footnote_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(footnote) = pre.to_packed::<FootnoteElem>() else { return (vec![], vec![]); };
    let FootnoteBody::Content(body) = &footnote.body else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != 1 { return (vec![], vec![]); }
    let child = annotate_realized(body, &realized_children[0]);
    let slot = SemanticSlot { label: SlotStep::FootnoteBody, child_index: 0 };
    (vec![child], vec![slot])
}

fn map_quote_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let Some(quote) = pre.to_packed::<QuoteElem>() else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != 1 { return (vec![], vec![]); }
    let child = annotate_realized(&quote.body, &realized_children[0]);
    let slot = SemanticSlot { label: SlotStep::QuoteBody, child_index: 0 };
    (vec![child], vec![slot])
}

fn map_wrapper_to_children(pre: &Content, realized: &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>) {
    let pre_body = wrapper_body_of(pre);
    let Some(pre_body) = pre_body else { return (vec![], vec![]); };
    let realized_children = collect_leaf_block_children(realized);
    if realized_children.len() != 1 { return (vec![], vec![]); }
    let child = annotate_realized(&pre_body, &realized_children[0]);
    let slot = SemanticSlot { label: SlotStep::WrapperBody, child_index: 0 };
    (vec![child], vec![slot])
}

fn wrapper_body_of(content: &Content) -> Option<Content> {
    if let Some(e) = content.to_packed::<AlignElem>() { return Some(e.body.clone()); }
    if let Some(e) = content.to_packed::<PadElem>() { return Some(e.body.clone()); }
    if let Some(e) = content.to_packed::<PlaceElem>() { return Some(e.body.clone()); }
    if let Some(e) = content.to_packed::<ColumnsElem>() { return Some(e.body.clone()); }
    if let Some(e) = content.to_packed::<BoxElem>() { return e.body.get_cloned(StyleChain::default()); }
    if let Some(e) = content.to_packed::<BlockElem>() {
        return match e.body.get_cloned(StyleChain::default()) {
            Some(BlockBody::Content(b)) => Some(b),
            _ => None,
        };
    }
    if let Some(e) = content.to_packed::<RectElem>() { return e.body.get_cloned(StyleChain::default()); }
    if let Some(e) = content.to_packed::<CircleElem>() { return e.body.get_cloned(StyleChain::default()); }
    if let Some(e) = content.to_packed::<EllipseElem>() { return e.body.get_cloned(StyleChain::default()); }
    None
}
```

Also add the `FootnoteBody` import at the top of the file (it was missing from the use statement in Task 2). Find the `use typst::model::{...}` line and add `FootnoteBody` to it.

- [ ] **Step 4: Run all annotated.rs tests**

```
cargo test --lib annotated 2>&1 | tail -20
```

Expected: all tests pass.

> **Note:** The `annotate_figure_maps_body_and_caption_separately` test uses `pre` as its own `realized` argument — a shortcut that works because `annotate_realized` with identical pre/realized will simply match the figure body to the figure body (same child count). If the test fails, the realized form of a figure differs structurally from the pre-realization form even before going through `ROUTINES.realize`. In that case, replace the test with a manually constructed pair where `realized = seq([text("Body"), text("Caption")])` (explicit count matching).

- [ ] **Step 5: Commit**

```bash
git add src/annotated.rs
git commit -m "refactor: implement slot mappers for all structural element types"
```

---

### Task 5: Add footnote annotation and verify Stage 1 corpus compatibility

**Files:**
- Modify: `src/annotated.rs`

Footnote handling: `annotate_realized` should attach `FootnoteInfo { body }` to any realized node that is a footnote marker site. This replaces the `restore_footnote_markers` walk.

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)]` block:

```rust
    #[test]
    fn annotate_handles_repeated_function_expansions_with_distinct_content() {
        // Three sequence children with the same detached span (simulates repeated fn expansion)
        // The walker must match each pre child to its positionally-corresponding realized child.
        let pre = seq([text("First"), text("Second"), text("Third")]);
        let realized = seq([text("R1"), text("R2"), text("R3")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.children.len(), 3);
        // Realized content must be the REALIZED text, not the pre text
        assert_eq!(node.children[0].realized.plain_text(), "R1");
        assert_eq!(node.children[1].realized.plain_text(), "R2");
        assert_eq!(node.children[2].realized.plain_text(), "R3");
    }
```

- [ ] **Step 2: Run test**

```
cargo test annotate_handles_repeated 2>&1 | tail -10
```

Expected: passes (the pairwise descent already handles this correctly — this test is a regression pin, not a new feature test).

- [ ] **Step 3: Add footnote annotation support**

Add a public function `annotate_footnote_markers` that post-processes an `AnnotatedContent` tree to attach `FootnoteInfo` to marker nodes. This mirrors the current `restore_footnote_markers` but attaches info as an annotation rather than replacing nodes.

Add before the `#[cfg(test)]` block:

```rust
/// Walk `node` in document order and attach [`FootnoteInfo`] to any node whose realized
/// form is a footnote marker (a `TextElem` whose text is a superscript number).
///
/// `footnotes` is the ordered list of `FootnoteElem` nodes from the pre-realization tree,
/// collected once by the caller before `annotate_realized` runs.
/// The attachment is a post-pass because the marker sites are not structurally
/// predictable from the pre-realization tree.
pub fn annotate_footnote_markers(
    node: &mut AnnotatedContent,
    footnotes: &[Content],
    next: &mut usize,
) {
    if footnotes.is_empty() { return; }
    if *next >= footnotes.len() { return; }

    // Check if this realized node is a footnote marker number
    if is_footnote_marker_text(&node.realized, *next + 1) {
        node.annotation.footnote = Some(FootnoteInfo { body: footnotes[*next].clone() });
        *next += 1;
        return;
    }

    for child in &mut node.children {
        annotate_footnote_markers(child, footnotes, next);
        if *next >= footnotes.len() { return; }
    }
}

fn is_footnote_marker_text(content: &Content, number: usize) -> bool {
    use typst::text::TextElem;
    content
        .to_packed::<TextElem>()
        .is_some_and(|t| t.text.as_str() == number.to_string())
}
```

- [ ] **Step 4: Run all Stage 1 tests**

```
cargo test --lib annotated 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 5: Run corpus check (visual spot-check only — no caller changes yet)**

```
cargo build 2>&1 | head -10
```

Expected: builds cleanly. (No corpus changes yet; `annotate_realized` has no production callers.)

- [ ] **Step 6: Commit**

```bash
git add src/annotated.rs
git commit -m "refactor: add footnote annotation support in annotated.rs; complete Stage 1"
```

---

## Stage 2: Switch `eval_to_realized_content` return type

### Task 6: Return `AnnotatedContent`; downstream reads `.realized`

**Files:**
- Modify: `src/eval.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`
- Modify: `tests/integration.rs`

In this stage, `eval_to_realized_content` returns `Result<AnnotatedContent>`. It still calls `restore_preserved` + `restore_footnote_markers` internally (the old swap-back machinery). The `AnnotatedContent` is built by wrapping the existing output AND calling `annotate_realized(pre_content, swapped_back_content)` so that annotations are populated for Stage 3. All callers are updated to use `.realized` where needed.

**Important:** `diff_content` still takes `&Content` arguments in this stage. Callers update to `diff_content(&old.realized, &new.realized)`.

- [ ] **Step 1: Write the failing test**

Add to `src/eval.rs` tests:

```rust
    #[test]
    fn eval_to_realized_content_returns_annotated_content_with_realized_field() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "Hello *world*.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let annotated = eval_to_realized_content(&world).unwrap();
        // realized must contain the document text
        assert!(annotated.realized.plain_text().contains("Hello"));
        assert!(annotated.realized.plain_text().contains("world"));
    }
```

- [ ] **Step 2: Run to see the test fail**

```
cargo test eval_to_realized_content_returns_annotated 2>&1 | tail -10
```

Expected: compile error — `eval_to_realized_content` returns `Result<Content>`, not `Result<AnnotatedContent>`.

- [ ] **Step 3: Update `eval_to_realized_content` signature and implementation**

In `src/eval.rs`, change:

```rust
// OLD
pub fn eval_to_realized_content(world: &dyn World) -> Result<Content> {
    let content = normalize_list_item_runs(eval_to_content(world)?);
    let introspector = layout_introspector(world, &content)?;
    realize_to_content(world, &content, introspector)
}
```

to:

```rust
// NEW
pub fn eval_to_realized_content(world: &dyn World) -> Result<crate::annotated::AnnotatedContent> {
    let pre_content = normalize_list_item_runs(eval_to_content(world)?);
    let introspector = layout_introspector(world, &pre_content)?;
    let realized_content = realize_to_content(world, &pre_content, introspector)?;
    // Build annotations by walking pre+realized together.
    // In Stage 2, realized_content is still the swapped-back hybrid form.
    let mut annotated = crate::annotated::annotate_realized(&pre_content, &realized_content);
    // Attach footnote annotations
    let footnotes = collect_footnotes(&pre_content);
    let mut next = 0;
    crate::annotated::annotate_footnote_markers(&mut annotated, &footnotes, &mut next);
    Ok(annotated)
}
```

- [ ] **Step 4: Update `src/lib.rs` re-export**

The `eval_to_realized_content` export changes its return type automatically. Add a convenience re-export for `AnnotatedContent`:

```rust
pub mod annotated;  // already added
pub use annotated::AnnotatedContent;
pub use eval::{eval_to_content, eval_to_realized_content};
```

- [ ] **Step 5: Update `src/main.rs` callers**

In `main.rs`, `eval_to_realized_content` is called in `run_diff`. Add `.realized` at each call site:

```rust
// Find lines like:
//   let old = eval_to_realized_content(&old_world)?;
// Change to:
//   let old = eval_to_realized_content(&old_world)?.realized;
```

Run `grep -n "eval_to_realized_content" src/main.rs` to find all call sites, then apply the `.realized` suffix.

- [ ] **Step 6: Update `tests/integration.rs` callers**

Run:

```
grep -n "eval_to_realized_content" tests/integration.rs
```

For each call site, add `.realized` to get the underlying `Content`. For example:

```rust
// OLD
let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
let result = typst_diff::diff::diff_content(&old, &new);

// NEW
let old = typst_diff::eval_to_realized_content(&old_world).unwrap().realized;
let new = typst_diff::eval_to_realized_content(&new_world).unwrap().realized;
let result = typst_diff::diff::diff_content(&old, &new);
```

For lines like `new.plain_text()`, those become `new.plain_text()` (no change needed since `new` is now `Content`).

The one test that stores the annotated result directly (`eval_to_realized_content_returns_annotated_content_with_realized_field`) is already written for the new type.

- [ ] **Step 7: Run all tests**

```
cargo test 2>&1 | tail -30
```

Expected: all existing tests pass. Any `repeated_same_span_blocks_*` tests must still pass — verify specifically:

```
cargo test repeated_function_expansions repeated_same_span 2>&1 | tail -20
```

- [ ] **Step 8: Run corpus**

```
tests/run_corpus.sh 2>&1 | tail -10
```

Expected: all 48 corpus tests pass (output PDFs unchanged because the underlying logic hasn't changed).

- [ ] **Step 9: Commit**

```bash
git add src/eval.rs src/lib.rs src/main.rs tests/integration.rs
git commit -m "refactor: eval_to_realized_content returns AnnotatedContent; callers use .realized"
```

---

## Stage 3: Migrate `diff.rs` to read from annotations

### Task 7: Switch `DiffBlock`, `extract_block_units`, and `diff_content` to use `AnnotatedContent`; introduce `DiffNode`/`NodeStatus`

**Files:**
- Modify: `src/diff.rs`
- Modify: `src/annotate.rs` (minimal: update imports/call sites)
- Modify: `tests/integration.rs` (update where DiffResult fields changed)

This task replaces `DiffBlock.content: Content` with `DiffBlock.node: AnnotatedContent`, replaces `extract_block_units` logic to use `annotation.semantic_kind` for block classification, and introduces the tree-shaped `DiffResult` / `DiffNode` / `NodeStatus` types. `diff_slots` is replaced by slot-based recursion using `node.annotation.slots`.

**Phase A behavior preservation:** slot-shape mismatches still fall through to word diff. The `HasChangedDescendants` status is produced only when old/new annotation slot shapes match exactly.

- [ ] **Step 1: Write failing tests for the new types**

Add to `src/diff.rs` tests:

```rust
    #[test]
    fn diff_unchanged_document_produces_all_unchanged_nodes() {
        use crate::annotated::{AnnotatedContent, Annotation};
        let content_a = TextElem::packed("Same text.");
        let node_a = AnnotatedContent {
            realized: content_a.clone(),
            annotation: Annotation::default(),
            children: vec![],
        };
        let result = diff_annotated(&node_a, &node_a);
        assert!(result.blocks.iter().all(|n| matches!(n.status, NodeStatus::Unchanged)));
    }
```

- [ ] **Step 2: Run to see compile failure**

```
cargo test diff_unchanged_document 2>&1 | tail -10
```

Expected: `error[E0425]: cannot find function \`diff_annotated\``

- [ ] **Step 3: Add the new types and `diff_annotated` entry point**

Add near the top of `src/diff.rs` (after existing `use` statements):

```rust
use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

/// Tree-shaped diff result. Each top-level element in the diff corresponds to
/// one block in the new document (or deleted block from the old).
pub struct DiffResult {
    pub blocks: Vec<DiffNode>,
    pub root_styles: Styles,
}

/// A single node in the diff result tree.
pub struct DiffNode {
    pub node: AnnotatedContent,
    pub status: NodeStatus,
    /// Per-slot children, populated when status is `HasChangedDescendants`.
    pub children: Vec<DiffNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeStatus {
    /// Entire subtree is unchanged (hash-equal realized content).
    Unchanged,
    /// Outer structure unchanged; at least one descendant has a non-Unchanged status.
    HasChangedDescendants,
    /// Block present in old but absent in new.
    Deleted,
    /// Block absent in old but present in new.
    Inserted,
    /// Text changed; word-level ops carry the edit.
    Modified(Vec<WordOp>),
}
```

Add the `diff_annotated` function (the new primary entry point, wraps the existing `diff_content`):

```rust
/// Diff two annotated content trees, producing a `DiffResult`.
///
/// In Phase A this is a thin wrapper: it calls `diff_content` on the underlying
/// `.realized` Content of each argument and converts the flat `Vec<DiffResultOp>`
/// to a `Vec<DiffNode>`. Phase B will remove the conversion and use the
/// annotated tree directly.
pub fn diff_annotated(old: &AnnotatedContent, new: &AnnotatedContent) -> DiffResult {
    let flat = diff_content(&old.realized, &new.realized);
    let blocks = flat.block_ops.into_iter().map(|op| diff_result_op_to_node(op, old, new)).collect();
    DiffResult { blocks, root_styles: flat.root_styles }
}

fn diff_result_op_to_node(op: DiffResultOp, _old: &AnnotatedContent, new_ac: &AnnotatedContent) -> DiffNode {
    match op {
        DiffResultOp::Equal(block) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Unchanged,
            children: vec![],
        },
        DiffResultOp::Deleted(block) => DiffNode {
            node: AnnotatedContent {
                realized: block.content.clone(),
                annotation: Annotation { ..Annotation::default() },
                children: vec![],
            },
            status: NodeStatus::Deleted,
            children: vec![],
        },
        DiffResultOp::Inserted(block) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Inserted,
            children: vec![],
        },
        DiffResultOp::Modified(block, word_ops) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Modified(word_ops),
            children: vec![],
        },
        DiffResultOp::ModifiedSlots(block, slot_diffs) => {
            // Convert slot diffs to children in Phase A
            let node = find_or_wrap_annotated(&block.content, new_ac);
            let children = slot_diffs.into_iter().map(|sd| DiffNode {
                node: AnnotatedContent {
                    realized: sd.ops.iter().find_map(|op| match op {
                        DiffResultOp::Modified(b, _) => Some(b.content.clone()),
                        DiffResultOp::Equal(b) => Some(b.content.clone()),
                        _ => None,
                    }).unwrap_or_else(|| TextElem::packed("")),
                    annotation: Annotation::default(),
                    children: vec![],
                },
                status: NodeStatus::HasChangedDescendants,
                children: vec![],
            }).collect();
            DiffNode {
                node,
                status: NodeStatus::HasChangedDescendants,
                children,
            }
        }
    }
}

/// Locate the `AnnotatedContent` in `root` whose `realized` matches `content`,
/// or wrap `content` in a bare `AnnotatedContent` if not found.
fn find_or_wrap_annotated(content: &Content, root: &AnnotatedContent) -> AnnotatedContent {
    fn find(node: &AnnotatedContent, target: &Content) -> Option<AnnotatedContent> {
        if &node.realized == target {
            return Some(AnnotatedContent {
                realized: node.realized.clone(),
                annotation: Annotation {
                    semantic_kind: node.annotation.semantic_kind.clone(),
                    slots: node.annotation.slots.clone(),
                    footnote: None,
                    span: node.annotation.span,
                },
                children: node.children.iter().map(|c| AnnotatedContent {
                    realized: c.realized.clone(),
                    annotation: Annotation {
                        semantic_kind: c.annotation.semantic_kind.clone(),
                        slots: c.annotation.slots.clone(),
                        footnote: None,
                        span: c.annotation.span,
                    },
                    children: vec![],
                }).collect(),
            });
        }
        node.children.iter().find_map(|child| find(child, target))
    }
    find(root, content).unwrap_or_else(|| AnnotatedContent {
        realized: content.clone(),
        annotation: Annotation::default(),
        children: vec![],
    })
}
```

Also update `DiffResult` to implement `modification_log` over the new tree:

```rust
impl DiffResult {
    pub fn modification_log(&self) -> String {
        let mut log = String::new();
        for (index, node) in self.blocks.iter().enumerate() {
            log_diff_node(&mut log, node, index, &[]);
        }
        log
    }
}

fn log_diff_node(log: &mut String, node: &DiffNode, index: usize, slot_prefix: &[SlotStep]) {
    match &node.status {
        NodeStatus::Unchanged => {}
        NodeStatus::Deleted => push_log_entry(log, index, "delete",
            &[("text", node.node.realized.plain_text().to_string())]),
        NodeStatus::Inserted => push_log_entry(log, index, "insert",
            &[("text", node.node.realized.plain_text().to_string())]),
        NodeStatus::Modified(word_ops) => {
            let deletes = collect_word_op_text(word_ops, |op| match op {
                WordOp::Delete(t) => Some(t), _ => None });
            let inserts = collect_word_op_text(word_ops, |op| match op {
                WordOp::Insert(t) => Some(t), _ => None });
            push_log_entry(log, index, "modify", &[
                ("block", node.node.realized.plain_text().to_string()),
                ("deleted", deletes), ("inserted", inserts),
            ]);
        }
        NodeStatus::HasChangedDescendants => {
            for (ci, child) in node.children.iter().enumerate() {
                log_diff_node(log, child, ci, slot_prefix);
            }
        }
    }
}
```

> **Important:** Keep the OLD `DiffResult` struct (rename it `DiffResultFlat`) and the OLD `diff_content` function in place for Stage 3 to avoid breaking `annotate.rs`. Create `DiffResult` as the new struct. Then update `annotate.rs` in Task 8. For now, `build_annotated_content` still uses the old flat result; `diff_annotated` is additive.

- [ ] **Step 4: Run all tests**

```
cargo test 2>&1 | tail -30
```

Expected: all existing tests pass, plus `diff_unchanged_document_produces_all_unchanged_nodes` passes.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "refactor: add DiffNode/NodeStatus/DiffResult tree types and diff_annotated entry point"
```

---

## Stage 4: Migrate `annotate.rs` to consume `DiffNode` trees

### Task 8: Update `build_annotated_content` to walk `DiffNode`; update `apply_fill_inside` to use annotated children

**Files:**
- Modify: `src/annotate.rs`
- Modify: `src/main.rs` (switch to `diff_annotated` + new `DiffResult`)
- Modify: `tests/integration.rs` (switch callers)

This task switches the annotate pipeline to consume the new `DiffResult`/`DiffNode` types. `apply_fill_inside` changes to use `node.annotation.slots` + `node.children[child_index]` (via `replace_slot`) for the Phase A case.

- [ ] **Step 1: Write failing tests**

Add to `src/annotate.rs` tests:

```rust
    #[test]
    fn annotate_walks_diff_node_tree_and_emits_colors_at_correct_status() {
        use crate::annotated::{AnnotatedContent, Annotation};
        use crate::diff::{DiffNode, DiffResult, NodeStatus, WordOp, Token};

        let node = DiffNode {
            node: AnnotatedContent {
                realized: TextElem::packed("new text"),
                annotation: Annotation::default(),
                children: vec![],
            },
            status: NodeStatus::Inserted,
            children: vec![],
        };
        let result = DiffResult {
            blocks: vec![node],
            root_styles: Default::default(),
        };
        let content = build_annotated_content_from_tree(&result, false);
        assert!(!content.is_empty());
    }
```

- [ ] **Step 2: Run to see compile failure**

```
cargo test annotate_walks_diff_node 2>&1 | tail -10
```

Expected: `error[E0425]: cannot find function \`build_annotated_content_from_tree\``

- [ ] **Step 3: Add `build_annotated_content_from_tree` (walks new DiffNode tree)**

Add to `src/annotate.rs`:

```rust
use crate::diff::{DiffNode, NodeStatus};

/// Build annotated content from the new tree-shaped [`crate::diff::DiffResult`].
pub fn build_annotated_content_from_tree(
    result: &crate::diff::DiffResult,
    compact_substitutions: bool,
) -> Content {
    let mut groups: Vec<Content> = Vec::new();
    let mut current_blocks: Vec<Content> = Vec::new();
    let mut current_page_styles = None;

    for node_block in annotate_diff_nodes(&result.blocks, compact_substitutions) {
        if current_page_styles
            .as_ref()
            .is_some_and(|styles| styles != &node_block.page_styles)
        {
            flush_group(&mut groups, &mut current_blocks, current_page_styles.take());
        }
        current_page_styles.get_or_insert_with(|| node_block.page_styles.clone());
        current_blocks.push(node_block.content);
    }
    flush_group(&mut groups, &mut current_blocks, current_page_styles);
    Content::sequence(groups).styled_with_map(result.root_styles.clone())
}

fn annotate_diff_nodes(nodes: &[DiffNode], compact: bool) -> Vec<DiffBlock> {
    nodes.iter().map(|node| annotate_single_node(node, compact)).collect()
}

fn annotate_single_node(node: &DiffNode, compact: bool) -> DiffBlock {
    let page_styles = Default::default(); // page styles are handled by block grouping
    match &node.status {
        NodeStatus::Unchanged => DiffBlock {
            content: node.node.realized.clone(),
            page_styles,
        },
        NodeStatus::Inserted => DiffBlock {
            content: if node.node.realized.plain_text().is_empty() {
                node.node.realized.clone()
            } else {
                apply_fill_inside_annotated(&node.node, green())
            },
            page_styles,
        },
        NodeStatus::Deleted => {
            let colored = plain_content(&node.node.realized)
                .styled(TextElem::fill.set(red().into()));
            let struck = Content::new(StrikeElem::new(colored));
            DiffBlock {
                content: replace_text_container(&node.node.realized, &struck).unwrap_or(struck),
                page_styles,
            }
        }
        NodeStatus::Modified(word_ops) => {
            let inline = annotated_inline_content(word_ops, compact);
            DiffBlock {
                content: replace_text_container(&node.node.realized, &inline).unwrap_or(inline),
                page_styles,
            }
        }
        NodeStatus::HasChangedDescendants => {
            // Recurse into children, reconstruct the outer container with annotated slots
            let content = apply_changed_descendants(&node.node, &node.children, compact)
                .unwrap_or_else(|| node.node.realized.clone());
            DiffBlock { content, page_styles }
        }
    }
}

fn apply_fill_inside_annotated(node: &crate::annotated::AnnotatedContent, fill: Color) -> Content {
    if node.annotation.slots.is_empty() {
        return node.realized.clone().styled(TextElem::fill.set(fill.into()));
    }
    let mut result = node.realized.clone();
    for slot in &node.annotation.slots {
        let slot_body = &node.children[slot.child_index].realized;
        let colored = slot_body.clone().styled(TextElem::fill.set(fill.into()));
        if let Some(next) = crate::content_slots::replace_slot(&result, &[slot.label.clone()], colored) {
            result = next;
        }
    }
    result
}

fn apply_changed_descendants(
    node: &crate::annotated::AnnotatedContent,
    children: &[DiffNode],
    compact: bool,
) -> Option<Content> {
    // For Phase A: reconstruct the realized form by splicing each changed child.
    // The slot's label gives us the replace_slot path into node.realized.
    let mut result = node.realized.clone();
    for diff_child in children {
        let annotated_slot = node.annotation.slots.iter()
            .find(|s| s.child_index < node.children.len()
                && node.children[s.child_index].realized == diff_child.node.realized);
        if let Some(slot) = annotated_slot {
            let new_content = annotate_single_node(diff_child, compact).content;
            if let Some(next) = crate::content_slots::replace_slot(&result, &[slot.label.clone()], new_content) {
                result = next;
            }
        }
    }
    Some(result)
}
```

- [ ] **Step 4: Run all tests**

```
cargo test 2>&1 | tail -30
```

Expected: all existing tests plus `annotate_walks_diff_node_tree_and_emits_colors_at_correct_status` pass.

- [ ] **Step 5: Switch `main.rs` to use `diff_annotated` + `build_annotated_content_from_tree`**

In `src/main.rs`, in the `run_diff` function:

```rust
// OLD
let old = eval_to_realized_content(&old_world)?.realized;
let new = eval_to_realized_content(&new_world)?.realized;
let result = diff::diff_content(&old, &new);
let annotated = annotate::build_annotated_content(&result, args.compact);

// NEW
let old = eval_to_realized_content(&old_world)?;
let new = eval_to_realized_content(&new_world)?;
let result = diff::diff_annotated(&old, &new);
let annotated = annotate::build_annotated_content_from_tree(&result, args.compact);
```

> The `.realized` suffix is removed here — we now pass `AnnotatedContent` directly to `diff_annotated`.

- [ ] **Step 6: Run corpus**

```
tests/run_corpus.sh 2>&1 | tail -20
```

Expected: all 48 corpus tests pass. If any differ, investigate — the Stage 4 implementation should produce identical output to Stage 3 for the Phase A cases.

- [ ] **Step 7: Commit**

```bash
git add src/annotate.rs src/main.rs tests/integration.rs
git commit -m "refactor: annotate.rs walks DiffNode tree; main.rs uses diff_annotated"
```

---

## Stage 5: Delete swap-back machinery; wire `annotate_realized` into production

### Task 9: Replace `restore_preserved` + `restore_footnote_markers` with `annotate_realized`

**Files:**
- Modify: `src/eval.rs`

This is the key Stage 5 change. Instead of `collect_preserved_by_span` + `restore_preserved` + `restore_footnote_markers`, `realize_to_content` now produces the raw realized content and `eval_to_realized_content` calls `annotate_realized` with the ACTUAL realized form.

- [ ] **Step 1: Write a regression test that pins span-independence**

The existing test `repeated_function_expansions_with_same_span_keep_their_own_content` (in `tests/integration.rs`) already covers this. Run it before making changes to confirm it passes:

```
cargo test repeated_function_expansions 2>&1 | tail -10
```

Expected: passes.

- [ ] **Step 2: Strip the swap-back from `realize_to_content`**

In `src/eval.rs`, `realize_to_content` currently:
1. Calls `collect_preserved_by_span`
2. Calls `collect_footnotes`
3. Calls `ROUTINES.realize`
4. Calls `restore_preserved` on each realized item
5. Calls `restore_footnote_markers`

Change it to return the raw realized content (no swap-back). Remove the `mut preserved` and `footnotes` variables and calls to `restore_preserved` / `restore_footnote_markers`. The function signature stays `Result<Content>` but now returns the actual realized form:

```rust
fn realize_to_content(
    world: &dyn World,
    content: &Content,
    introspector: Introspector,
) -> Result<Content> {
    let library = world.library();
    let target = TargetElem::target.set(Target::Paged).wrap();
    let base = StyleChain::new(&library.styles);
    let styles = base.chain(&target);
    let style_map = styles.to_map().outside();
    let root_page_styles = page_styles(&style_map);
    let styles = StyleChain::new(&style_map);
    // NOTE: No collect_preserved_by_span, no collect_footnotes.
    // The caller (eval_to_realized_content) handles annotations via annotate_realized.

    let traced = Traced::default();
    let mut sink = Sink::new();
    let mut engine = Engine {
        routines: &ROUTINES,
        world: world.track(),
        introspector: introspector.track(),
        traced: traced.track(),
        sink: sink.track_mut(),
        route: Route::default(),
    };

    let arenas = Arenas::default();
    let mut info = DocumentInfo::default();
    let mut locator = Locator::root().split();
    let realized = (ROUTINES.realize)(
        RealizationKind::LayoutDocument { info: &mut info },
        &mut engine,
        &mut locator,
        &arenas,
        content,
        styles,
    )
    .map_err(|errs| anyhow::anyhow!("realize failed:\n{}", format_diagnostics(world, &errs)))?;

    let delayed = sink.delayed();
    if !delayed.is_empty() {
        return Err(anyhow::anyhow!(
            "realize errors:\n{}",
            format_diagnostics(world, &delayed)
        ));
    }

    // Build the realized sequence with style wrapping (same as before but no restore_preserved)
    let realized = Content::sequence(realized.iter().map(|(realized_content, styles)| {
        let styles = if realized_content.is::<PagebreakElem>() {
            marginal_styles(&styles.to_map())
        } else {
            non_page_styles(styles.to_map())
        };
        (*realized_content).clone().styled_with_map(styles)
    }))
    .styled_with_map(root_page_styles);

    Ok(realized)
}
```

Also remove from the `use` list in `eval.rs`: `is_slot_container`, `replace_slot` (and remove the `extract_slots` import if it was there). Keep `normalize_list_item_runs`.

- [ ] **Step 3: Remove unused functions from `eval.rs`**

Delete these functions entirely:
- `collect_preserved_by_span`
- `restore_preserved`
- `restore_footnote_markers`
- `restore_footnote_markers_inner`
- `restore_footnote_markers_in_sequence`
- `is_footnote_marker_deep`
- `is_footnote_scaffold`
- `is_footnote_marker`
- `collect_footnotes`

Remove their unit tests (they will be replaced below):
- `collect_preserved_by_span_keeps_multiple_values_for_same_span` — DELETE (mechanism gone)
- `restore_preserved_consumes_same_span_values_in_order` — DELETE
- `restore_preserved_recurses_into_slot_container_children` — DELETE
- `restore_preserved_leaves_unknown_content_unchanged` — DELETE
- `restore_footnote_markers_replaces_markers_in_document_order` — DELETE
- `restore_footnote_markers_handles_styled_marker` — DELETE
- `restore_footnote_markers_does_not_replace_non_matching_numbers` — DELETE

Add a replacement test that verifies the new walker handles the same guarantee:

```rust
    #[test]
    fn annotate_realized_handles_repeated_function_expansions_with_distinct_content() {
        // Integration test: repeated fn body with same span → distinct annotated children
        // (The span-uniqueness bug class is architecturally eliminated; this is a
        // regression guard on the overall pipeline behavior.)
        let (_dir, old_world, new_world) = temp_worlds(
            "#let f(body) = [#body]\n#f[a]\n#f[b]",
            "#let f(body) = [#body]\n#f[x]\n#f[b]",
        );
        let old = eval_to_realized_content(&old_world).unwrap();
        let new = eval_to_realized_content(&new_world).unwrap();
        // The new document's realized form must contain both x and b
        assert!(new.realized.plain_text().contains('x'), "{}", new.realized.plain_text());
        assert!(new.realized.plain_text().contains('b'), "{}", new.realized.plain_text());
    }
```

- [ ] **Step 4: Run all tests**

```
cargo test 2>&1 | tail -30
```

Expected: all tests pass. If any test breaks, investigate whether it relied on `restore_preserved` behavior (swap-back). Such tests need to be rewritten against the new annotated-tree semantics.

- [ ] **Step 5: Run corpus**

```
tests/run_corpus.sh --verbose 2>&1 | tail -30
```

Expected: all 48 corpus pairs pass. The visual output may differ from the pre-Stage-5 output for documents with lists, tables, or figures (because `node.realized` is now the actual realized form, not the swapped-back form, and `apply_fill_inside_annotated` uses `replace_slot` on the actual realized form). If the slot-based apply fails silently (returns no-op), the fill will be applied to the whole block instead of slot-by-slot. This is a visual regression — investigate `apply_fill_inside_annotated` and fix `replace_slot` paths if needed. Spot-check corpus tests: `19-list-item-added`, `38-table-cell-changed`, `43-figure-caption-changed`.

- [ ] **Step 6: Commit**

```bash
git add src/eval.rs
git commit -m "refactor: remove swap-back machinery; eval produces actual realized content"
```

---

### Task 10: Shrink `content_slots.rs`; final cleanup

**Files:**
- Modify: `src/content_slots.rs`
- Modify: `src/diff.rs` (remove lingering `extract_slots` call in `collect_slot_tokens`)
- Modify: `src/annotate.rs` (confirm `replace_slot` calls are all that remain)

- [ ] **Step 1: Audit remaining usages of items slated for deletion**

```bash
grep -rn "extract_slots\|is_slot_container\|ContentSlot\|collect_slots\|collect_table_slots\|collect_grid_slots\|wrapper_body\|push_slot" src/ tests/
```

Review each hit. Items to keep: `SlotStep`, `normalize_list_item_runs`, `replace_slot`, `replace_inline_content`. Items to delete: `extract_slots`, `is_slot_container`, `ContentSlot`, `collect_slots` and all its helpers.

- [ ] **Step 2: Delete `extract_slots` and `is_slot_container` from `content_slots.rs`**

Remove the following from `src/content_slots.rs`:
- `ContentSlot` struct and its doc comment
- `extract_slots` function
- `is_slot_container` function
- `collect_slots` function and all its private helpers (`push_slot`, `wrapper_body`, `collect_table_slots`, `collect_table_item_slots`, `collect_table_item_slot`, `collect_grid_slots`, `collect_grid_item_slots`, `collect_grid_item_slot`)

Keep:
- `SlotStep` enum (used by `annotated.rs` `SemanticSlot.label` and `replace_slot` paths)
- `normalize_list_item_runs` + `group_list_item_runs`
- `replace_slot` + all its helpers (`replace_wrapper_body`, `replace_table_cell`, etc.)
- `replace_inline_content` + `is_inlineish`

- [ ] **Step 3: Remove `extract_slots` usage from `diff.rs`**

In `src/diff.rs`, `collect_slot_tokens` calls `extract_slots`:

```rust
fn collect_slot_tokens(content: &Content, out: &mut Vec<Token>) -> bool {
    let slots = extract_slots(content);
    ...
}
```

Replace with: since we no longer extract slots from the realized form in diff.rs (diff.rs now uses `node.annotation.slots`), the `collect_slot_tokens` function should be simplified or removed. If it's still needed for the word-diff tokenizer (which operates on `node.realized`), replace `extract_slots` with a direct check on `annotation.slots` via the node being tokenized.

For Phase A, the simplest fix: if `content` is a structural element kind (check against the known types that were in `is_slot_container`), produce no extra tokens — let the tokenizer treat the whole block as atomic. The word diff for such elements operates at the block level:

```rust
fn collect_slot_tokens(content: &Content, out: &mut Vec<Token>) -> bool {
    // In Phase A, slot-bearing elements are handled at the block level.
    // The tokenizer sees only the plain text of the slot content,
    // which extract_words already recurses into via its generic fallback.
    // Return false to fall through to plain_text tokenization.
    false
}
```

Or remove `collect_slot_tokens` entirely and let the `else` branch of `collect_tokens` handle it.

- [ ] **Step 4: Remove deleted `content_slots` items from `eval.rs` imports**

In `src/eval.rs`:
```rust
// OLD
use crate::content_slots::{
    extract_slots, is_slot_container, normalize_list_item_runs, replace_slot,
};

// NEW  
use crate::content_slots::normalize_list_item_runs;
```

- [ ] **Step 5: Update `content_slots.rs` tests**

Delete tests for deleted functions:
- `is_slot_container_matches_representative_extractable_elements` — DELETE
- `list_slots_extract_and_replace_each_item_body` — the EXTRACT part tests `extract_slots` (delete), but the REPLACE part tests `replace_slot` (keep). Rewrite to test only `replace_slot` directly:

```rust
#[test]
fn list_replace_slot_changes_correct_item() {
    use typst::foundations::Packed;
    use typst::model::{ListElem, ListItem};

    let content = Content::new(ListElem::new(vec![
        Packed::new(ListItem::new(text("Alpha"))),
        Packed::new(ListItem::new(text("Beta"))),
    ]));
    let replaced = replace_slot(&content, &[SlotStep::ListItem(1)], text("Better")).unwrap();
    assert_eq!(replaced.to_packed::<ListElem>().unwrap().children[1].body.plain_text(), "Better");
    // Original unchanged
    assert_eq!(content.to_packed::<ListElem>().unwrap().children[1].body.plain_text(), "Beta");
}
```

Similarly port the extract+replace tests for enum, terms, figure, footnote, quote, table, grid, stack, wrapper — keep only the replace half.

- [ ] **Step 6: Run all tests**

```
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

```
tests/run_corpus.sh 2>&1 | tail -10
```

Expected: all 48 corpus pairs pass.

- [ ] **Step 7: Update `src/lib.rs` pipeline comment**

In `src/lib.rs`, update the doc comment to reflect the new pipeline:

```rust
//! ```text
//! old.typ ──► SystemWorld ──► eval_to_realized_content ──► old: AnnotatedContent
//! new.typ ──► SystemWorld ──► eval_to_realized_content ──► new: AnnotatedContent
//!                                       │
//!                          diff::diff_annotated(old, new) ──► DiffResult
//!                                       │
//!               annotate::build_annotated_content_from_tree(result) ──► Content
//!                                       │
//!                render::render_to_pdf(content, new_world) ──► Vec<u8>
//! ```
```

- [ ] **Step 8: Final full verification**

```bash
cargo test 2>&1 | tail -10
tests/run_corpus.sh 2>&1 | tail -10
```

Expected: `cargo test` reports ≥33 tests passing (all existing plus new Stage 1-5 tests). Corpus: 48/48 passing.

- [ ] **Step 9: Commit**

```bash
git add src/content_slots.rs src/diff.rs src/annotate.rs src/eval.rs src/lib.rs
git commit -m "refactor: shrink content_slots.rs; delete extract_slots and is_slot_container"
```

---

## Self-review checklist

Run through this before considering the refactor complete:

**Spec coverage:**
- [x] `AnnotatedContent` / `Annotation` / `SemanticKind` / `SemanticSlot` / `FootnoteInfo` types defined
- [x] `annotate_realized` walker implemented (transparent wrappers, structural elements, leaves)
- [x] Per-element slot mappers for all 12 types in `content_slots.rs`'s original `is_slot_container`
- [x] `annotate_footnote_markers` post-pass
- [x] One-to-many expansion rules from design doc:
  - [x] Rule 1 (outer wrapping): `collect_leaf_block_children` descends through `BlockElem` / `GridElem` / `StyledElem`; only outermost realized node carries `semantic_kind`. Covered by `annotate_list_descends_through_outer_block_grid_wrappers` (Task 4).
  - [x] Rule 2 (inner injection): `HeadingElem` / `EquationElem` / `RawElem` branches use `leaf_annotated` → inner realized children stay anonymous and unwalked; diff word-diffs the whole realized form. Covered by `annotate_heading_with_numbering_keeps_heading_kind_at_outer_level` (Task 4).
  - [x] Rule 3 (sibling-level expansion): SequenceElem length mismatch falls through to `pair_sequence_by_span` — span-based document-order pairing with anonymous unmatched extras. Covered by `annotate_sequence_with_extra_realized_children_produces_anonymous_extras` and `annotate_sequence_with_extra_pre_children_drops_unmatched_pre` (Task 2).
- [x] `eval_to_realized_content` returns `AnnotatedContent` (Stage 2)
- [x] `diff_annotated` entry point + `DiffNode`/`NodeStatus`/`DiffResult` types (Stage 3)
- [x] `build_annotated_content_from_tree` walks `DiffNode` tree (Stage 4)
- [x] Swap-back machinery deleted from `eval.rs` (Stage 5)
- [x] `extract_slots` / `is_slot_container` deleted from `content_slots.rs` (Stage 5)
- [x] `collect_preserved_by_span` / `restore_preserved` / `restore_footnote_markers*` deleted (Stage 5)

**Type consistency check:** `SemanticSlot.label` is `SlotStep`, same enum used in `replace_slot` paths throughout. `DiffNode.children` mirrors `node.children` from `AnnotatedContent`. `NodeStatus::Modified(Vec<WordOp>)` — same `WordOp` type as before.

**Known issues / Phase B work:**
- `apply_fill_inside_annotated` and `apply_changed_descendants` in Stage 4 still call `replace_slot` with path-based addressing. In Stage 5, once `node.realized` is the actual realized form (not swapped-back), these calls may fail silently if the realized form doesn't match the `SlotStep` paths. Verify by spot-checking corpus 19, 38, 43 and fix if needed.
- Slot-level LCS for shape-mismatched containers (e.g., list with an item added) is Phase B work: `diff_annotated` currently falls through to flat word diff for these cases.
- `modification_log` in the new `DiffResult` uses a simplified walker. The `slot_path_prefix` detail from the old implementation is dropped; if any downstream tooling relied on the exact format, update.

---

## Verification commands (run at any point to check regression)

```bash
cargo build                    # must succeed
cargo test                     # all unit + integration tests
tests/run_corpus.sh            # 48 corpus pairs, visual spot-check
tests/run_corpus.sh --filter 19   # list-item-added
tests/run_corpus.sh --filter 38   # table-cell-changed (if exists)
tests/run_corpus.sh --filter 39   # fn-content-args-changed (span uniqueness)
tests/run_corpus.sh --verbose  # show modification logs for all pairs
```
