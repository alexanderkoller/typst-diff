//! Annotated realized content tree.
//!
//! [`AnnotatedContent`] pairs a realized [`Content`] node with semantic
//! information recovered from the pre-realization tree. The realized side is
//! preserved exactly as Typst produced it; annotations are built once and
//! never mutated.

use typst::foundations::{Content, SequenceElem, StyledElem};
use typst::foundations::StyleChain;
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridChild, GridElem, GridItem, PadElem,
    PlaceElem, StackElem,
};
use typst::math::EquationElem;
use typst::model::{
    EnumElem, FigureElem, FootnoteElem, HeadingElem, ListElem, ParElem, QuoteElem, TableElem,
    TermsElem,
};
use typst::syntax::Span;
use typst::text::RawElem;
use typst::visualize::{CircleElem, EllipseElem, RectElem};
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

    /// Is the realized content empty?
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

/// Build an annotated tree by walking `pre` (pre-realization) and `realized` together.
///
/// The realized field of every node in the returned tree is always identical to
/// a subtree of `realized` — this is the read-only invariant.
pub fn annotate_realized(pre: &Content, realized: &Content) -> AnnotatedContent {
    // --- Structural elements: semantic_kind + slot map ---
    if pre.is::<ListElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::List, map_list_to_children);
    }
    if pre.is::<EnumElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Enum, map_enum_to_children);
    }
    if pre.is::<TermsElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Terms, map_terms_to_children);
    }
    if pre.is::<TableElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Table, map_table_to_children);
    }
    if pre.is::<GridElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Grid, map_grid_to_children);
    }
    if pre.is::<StackElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Stack, map_stack_to_children);
    }
    if pre.is::<FigureElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Figure, map_figure_to_children);
    }
    if pre.is::<FootnoteElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Footnote, map_footnote_to_children);
    }
    if pre.is::<QuoteElem>() {
        return annotate_with_kind(pre, realized, SemanticKind::Quote, map_quote_to_children);
    }
    if let Some(wrapper_kind) = wrapper_kind_of(pre) {
        return annotate_with_kind(pre, realized, SemanticKind::Wrapper(wrapper_kind), map_wrapper_to_children);
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

/// Pair pre and realized sequence children by span, in document order.
///
/// Used when a `SequenceElem`'s pre and realized children counts diverge.
/// Walks both sequences in document order; for each pre child, advances a
/// cursor through realized children seeking a span match. Skipped realized
/// children become anonymous leaves. Pre children with no realized partner
/// are dropped. Trailing realized children become anonymous leaves.
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
    mapper: fn(&Content, &Content) -> (Vec<AnnotatedContent>, Vec<SemanticSlot>),
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
pub fn collect_leaf_block_children(content: &Content) -> Vec<Content> {
    // Descend into BlockElem body
    if let Some(block) = content.to_packed::<BlockElem>() {
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            return collect_leaf_block_children(&body);
        }
    }
    // Collect GridElem cells (skip non-cell items like gutters)
    if let Some(grid) = content.to_packed::<GridElem>() {
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

// Stub mappers — to be implemented in Tasks 3/4.
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

#[cfg(test)]
mod tests {
    use super::*;
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
