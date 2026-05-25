//! Annotated realized content tree.
//!
//! [`AnnotatedContent`] pairs a realized [`Content`] node with semantic
//! information recovered from the pre-realization tree. The realized side is
//! preserved exactly as Typst produced it; annotations are built once and
//! never mutated.

use crate::container_ops::{self, ContainerKind};
use std::collections::VecDeque;
use typst::foundations::{Content, SequenceElem, StyleChain, StyledElem};
use typst::layout::{BlockBody, BlockElem};
use typst::math::EquationElem;
use typst::model::{HeadingElem, ParElem};
use typst::syntax::Span;
use typst::text::RawElem;

/// A realized Content node together with its semantic identity.
#[derive(Clone)]
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

    /// Resolve a descendant path through [`AnnotatedContent::children`].
    pub fn get_path(&self, path: &[usize]) -> Option<&AnnotatedContent> {
        let mut node = self;
        for index in path {
            node = node.children.get(*index)?;
        }
        Some(node)
    }

    /// Resolve a mutable descendant path through [`AnnotatedContent::children`].
    pub fn get_path_mut(&mut self, path: &[usize]) -> Option<&mut AnnotatedContent> {
        let mut node = self;
        for index in path {
            node = node.children.get_mut(*index)?;
        }
        Some(node)
    }
}

#[derive(Clone)]
pub struct Annotation {
    /// Pre-realization element type if this node is a tracked structural element.
    /// `None` for plain text, spaces, anonymous wrappers.
    pub semantic_kind: Option<SemanticKind>,
    /// Semantic slots — named positions within `children` that the diff recurses into.
    pub slots: Vec<SemanticSlot>,
    /// Footnote body if this realized node is a footnote marker site.
    pub footnote: Option<FootnoteInfo>,
    /// Structured content to use as the local edit surface when realization is opaque.
    pub patch_surface: Option<Content>,
    /// Source equations whose realized math carriers live under this realized node.
    pub equation_origins: Vec<Content>,
    /// Source span for diagnostics (not used as a lookup key).
    pub span: Span,
}

impl Default for Annotation {
    fn default() -> Self {
        Annotation {
            semantic_kind: None,
            slots: vec![],
            footnote: None,
            patch_surface: None,
            equation_origins: vec![],
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
    Align,
    Pad,
    Place,
    Columns,
    Box,
    Block,
    Rect,
    Circle,
    Ellipse,
}

/// One semantic slot label inside a structured container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotStep {
    ListItem(usize),
    EnumItem(usize),
    Term(usize),
    TermDescription(usize),
    FigureBody,
    FigureCaption,
    FootnoteBody,
    QuoteBody,
    WrapperBody,
    TableCell(usize),
    GridCell(usize),
    StackChild(usize),
}

/// A named semantic position within an [`AnnotatedContent`] node.
///
/// `path` points through the annotated realized tree's `children` vec.
/// `label` identifies the slot's role (e.g. `ListItem(0)`).
#[derive(Clone, Debug)]
pub struct SemanticSlot {
    pub label: SlotStep,
    pub path: Vec<usize>,
}

#[derive(Clone)]
pub struct FootnoteInfo {
    pub body: Content,
}

/// Build an annotated tree by walking `pre` (pre-realization) and `realized` together.
///
/// The realized field of every node in the returned tree is always identical to
/// a subtree of `realized` — this is the read-only invariant.
pub fn annotate_realized(pre: &Content, realized: &Content) -> AnnotatedContent {
    // Root-level mismatch: pre is a bare SequenceElem but realized has been
    // wrapped with root page styles (StyledElem → SequenceElem). Peel the
    // wrapper and recurse with the same pre so the inner SequenceElem matches.
    if pre.to_packed::<SequenceElem>().is_some() {
        if let Some(styled) = realized.to_packed::<StyledElem>() {
            let inner = annotate_realized(pre, &styled.child);
            return AnnotatedContent {
                realized: realized.clone(),
                annotation: Annotation {
                    span: pre.span(),
                    ..Annotation::default()
                },
                children: inner.children,
            };
        }
    }

    // --- Structural elements: semantic_kind + slot map ---
    if let Some(kind) = ContainerKind::of(pre) {
        return annotate_container(pre, realized, kind);
    }
    if pre.is::<EquationElem>() {
        return leaf_annotated(
            realized,
            Annotation {
                semantic_kind: Some(SemanticKind::Equation),
                span: pre.span(),
                ..Annotation::default()
            },
        );
    }
    if pre.is::<HeadingElem>() {
        return leaf_annotated(
            realized,
            Annotation {
                semantic_kind: Some(SemanticKind::Heading),
                span: pre.span(),
                ..Annotation::default()
            },
        );
    }
    if pre.is::<RawElem>() {
        return leaf_annotated(
            realized,
            Annotation {
                semantic_kind: Some(SemanticKind::RawBlock),
                span: pre.span(),
                ..Annotation::default()
            },
        );
    }

    // --- Transparent wrappers: pairwise descent ---
    if let (Some(pre_seq), Some(real_seq)) = (
        pre.to_packed::<SequenceElem>(),
        realized.to_packed::<SequenceElem>(),
    ) {
        let children = if pre_seq.children.len() == real_seq.children.len() {
            pre_seq
                .children
                .iter()
                .zip(real_seq.children.iter())
                .map(|(p, r)| annotate_realized(p, r))
                .collect()
        } else {
            pair_sequence_by_span(&pre_seq.children, &real_seq.children)
        };
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                span: pre.span(),
                ..Annotation::default()
            },
            children,
        };
    }
    if let Some(pre_seq) = pre.to_packed::<SequenceElem>() {
        let children = pre_seq
            .children
            .iter()
            .map(|child| annotate_realized(child, child))
            .collect();
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                span: pre.span(),
                ..Annotation::default()
            },
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
            annotation: Annotation {
                span: pre.span(),
                ..Annotation::default()
            },
            children: vec![child],
        };
    }
    if let (Some(pre_p), Some(real_p)) =
        (pre.to_packed::<ParElem>(), realized.to_packed::<ParElem>())
    {
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
    leaf_annotated(
        realized,
        Annotation {
            semantic_kind: semantic_kind_of(pre),
            span: pre.span(),
            ..Annotation::default()
        },
    )
}

fn leaf_annotated(realized: &Content, annotation: Annotation) -> AnnotatedContent {
    AnnotatedContent {
        realized: realized.clone(),
        annotation,
        children: vec![],
    }
}

/// Return the effective span of a realized content node.
///
/// Realization wraps nodes with `StyledElem` for non-page styles. The outer
/// `StyledElem` inherits the file root span (`Span(1)`) rather than the
/// original node's span. Look through StyledElem wrappers to find the first
/// non-file-root span so that `pair_sequence_by_span` can match pre children
/// against their realized counterparts.
fn effective_span(c: &Content) -> typst::syntax::Span {
    if let Some(styled) = c.to_packed::<StyledElem>() {
        let inner = effective_span(&styled.child);
        if !inner.is_detached() && inner != c.span() {
            return inner;
        }
    }
    c.span()
}

/// Pair pre and realized sequence children by span, in document order.
///
/// Used when a `SequenceElem`'s pre and realized children counts diverge.
/// Walks both sequences in document order; for each pre child, advances a
/// cursor through realized children seeking a span match. Skipped realized
/// children become anonymous leaves. Pre children with no realized partner
/// are dropped. Trailing realized children become anonymous leaves.
///
/// Realized children that are `StyledElem` wrappers (added by realization)
/// are matched by looking through the wrapper to the inner content's span via
/// `effective_span`, because the outer wrapper inherits the file root span.
fn pair_sequence_by_span(
    pre_children: &[Content],
    real_children: &[Content],
) -> Vec<AnnotatedContent> {
    let mut out: Vec<AnnotatedContent> = Vec::new();
    let mut cursor: usize = 0;
    for pre_child in pre_children {
        let target = pre_child.span();
        let mut match_idx = None;
        for (idx, real_child) in real_children.iter().enumerate().skip(cursor) {
            if effective_span(real_child) == target {
                match_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = match_idx {
            while cursor < idx {
                out.push(leaf_annotated(
                    &real_children[cursor],
                    Annotation::default(),
                ));
                cursor += 1;
            }
            out.push(annotate_realized(pre_child, &real_children[idx]));
            cursor = idx + 1;
            continue;
        }

        // If no span match exists, fall back to positional pairing instead of
        // dropping the pre node. This preserves structural semantics when
        // realization rewrites spans on wrapper-heavy sequences.
        if cursor < real_children.len() {
            out.push(annotate_realized(pre_child, &real_children[cursor]));
            cursor += 1;
        }
    }
    // Any trailing unmatched realized children become anonymous leaves.
    while cursor < real_children.len() {
        out.push(leaf_annotated(
            &real_children[cursor],
            Annotation::default(),
        ));
        cursor += 1;
    }
    out
}

fn semantic_kind_of(pre: &Content) -> Option<SemanticKind> {
    if pre.is::<HeadingElem>() {
        return Some(SemanticKind::Heading);
    }
    if pre.is::<EquationElem>() {
        return Some(SemanticKind::Equation);
    }
    if pre.is::<RawElem>() {
        return Some(SemanticKind::RawBlock);
    }
    if let Some(kind) = ContainerKind::of(pre) {
        return Some(kind.semantic_kind());
    }
    if pre.is::<ParElem>() {
        return Some(SemanticKind::Paragraph);
    }
    None
}

/// Build an AnnotatedContent for a structural element through shared container ops.
fn annotate_container(pre: &Content, realized: &Content, kind: ContainerKind) -> AnnotatedContent {
    let semantic_kind = kind.semantic_kind();
    let mapping = container_ops::map_container(pre, realized, kind);
    let patch_surface = mapping.patch_surface;
    let patch_surface = (patch_surface != *realized).then_some(patch_surface);
    AnnotatedContent {
        realized: realized.clone(),
        annotation: Annotation {
            semantic_kind: Some(semantic_kind),
            slots: mapping.slots,
            patch_surface,
            equation_origins: vec![],
            span: pre.span(),
            footnote: None,
        },
        children: mapping.children,
    }
}

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
    if footnotes.is_empty() {
        return;
    }
    if *next >= footnotes.len() {
        return;
    }

    // Check if this realized node is a footnote marker number
    if is_footnote_marker_text(&node.realized, *next + 1) {
        node.annotation.footnote = Some(FootnoteInfo {
            body: footnotes[*next].clone(),
        });
        *next += 1;
        return;
    }

    for child in &mut node.children {
        annotate_footnote_markers(child, footnotes, next);
        if *next >= footnotes.len() {
            return;
        }
    }
}

/// Attach source equation nodes to the realized leaves that contain their realized math.
///
/// Typst realization turns math into render-oriented `inline` / `display` content, often
/// wrapped in paragraphs, blocks, and styles. The diff needs logical equation identity,
/// so this pass maps source `EquationElem`s to realized math carriers in document order
/// without changing the realized tree shape.
pub fn annotate_equation_origins(pre: &Content, node: &mut AnnotatedContent) {
    let mut equations = VecDeque::from(collect_source_equations(pre));
    assign_equation_origins(node, &mut equations);
}

fn collect_source_equations(content: &Content) -> Vec<Content> {
    let mut equations = Vec::new();
    let _ = content.traverse::<_, ()>(&mut |content| {
        if content.is::<EquationElem>() {
            equations.push(content);
        }
        std::ops::ControlFlow::Continue(())
    });
    equations
}

fn assign_equation_origins(node: &mut AnnotatedContent, equations: &mut VecDeque<Content>) {
    if node.children.is_empty() {
        let count = realized_equation_carrier_count(&node.realized).min(equations.len());
        node.annotation.equation_origins =
            (0..count).filter_map(|_| equations.pop_front()).collect();
        return;
    }

    for child in &mut node.children {
        assign_equation_origins(child, equations);
    }
}

fn realized_equation_carrier_count(content: &Content) -> usize {
    if is_realized_equation_carrier(content) {
        return 1;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .map(realized_equation_carrier_count)
            .sum();
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return realized_equation_carrier_count(&styled.child);
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return realized_equation_carrier_count(&par.body);
    }
    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return realized_equation_carrier_count(&heading.body);
    }
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return realized_equation_carrier_count(&body);
    }
    0
}

fn is_realized_equation_carrier(content: &Content) -> bool {
    content.is::<EquationElem>()
        || matches!(content.func().name(), "inline" | "display")
        || (content.is::<BlockElem>() && content.plain_text().is_empty())
}

fn is_footnote_marker_text(content: &Content, number: usize) -> bool {
    use typst::text::TextElem;
    if let Some(t) = content.to_packed::<TextElem>() {
        return t.text.as_str() == number.to_string();
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return is_footnote_marker_text(&styled.child, number);
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .any(|c| is_footnote_marker_text(c, number));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::TextElem;

    fn text(s: &str) -> Content {
        TextElem::packed(s)
    }

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
        let realized =
            text("after").styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let realized_clone = realized.clone();
        let node = annotate_realized(&pre, &realized);
        assert_eq!(node.realized, realized_clone);
    }

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
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::ListItem(0)
        ));
        assert_eq!(node.annotation.slots[0].path, vec![0]);
        assert!(matches!(
            node.annotation.slots[2].label,
            SlotStep::ListItem(2)
        ));
        assert_eq!(node.children[0].realized.plain_text(), "Alpha");
        assert_eq!(node.children[2].realized.plain_text(), "Gamma");
    }

    #[test]
    fn annotate_list_descends_into_realized_list_items() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem};

        let pre = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
        ]));
        let realized = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
        ]));
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].realized.plain_text(), "Alpha");
        assert_eq!(node.children[1].realized.plain_text(), "Beta");
    }

    #[test]
    fn annotate_list_keeps_slots_when_realized_child_count_mismatches() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem};

        let pre = Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
            "Alpha",
        )))]));
        let realized = seq([text("Alpha"), text("extra")]); // 2 realized children, 1 pre item
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert_eq!(node.annotation.slots.len(), 1);
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.annotation.slots[0].path, vec![0]);
        assert_eq!(node.children[0].realized.plain_text(), "Alpha");
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
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::EnumItem(0)
        ));
        assert!(matches!(
            node.annotation.slots[1].label,
            SlotStep::EnumItem(1)
        ));
        assert_eq!(node.children[1].realized.plain_text(), "Two");
    }

    #[test]
    fn annotate_terms_maps_term_and_description_separately() {
        use typst::foundations::Packed;
        use typst::model::{TermItem, TermsElem};

        let pre = Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
            text("API"),
            text("Definition"),
        ))]));
        // Realized: 2 children for 1 term (term + description)
        let realized = seq([text("API"), text("Definition")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Terms));
        assert_eq!(node.annotation.slots.len(), 2);
        let labels: Vec<String> = node
            .annotation
            .slots
            .iter()
            .map(|s| format!("{:?}", s.label))
            .collect();
        assert!(
            matches!(node.annotation.slots[0].label, SlotStep::Term(0)),
            "{labels:?}"
        );
        assert!(
            matches!(node.annotation.slots[1].label, SlotStep::TermDescription(0)),
            "{labels:?}"
        );
        assert_eq!(node.children[0].realized.plain_text(), "API");
        assert_eq!(node.children[1].realized.plain_text(), "Definition");
    }

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
        assert!(matches!(
            node.annotation.slots[2].label,
            SlotStep::TableCell(2)
        ));
        assert_eq!(node.children[3].realized.plain_text(), "D");
    }

    #[test]
    fn annotate_figure_maps_body_and_caption_separately() {
        use crate::eval::eval_to_content;
        use crate::normalize::normalize_list_item_runs;
        use crate::world::SystemWorld;
        use std::fs;
        use tempfile::TempDir;
        use typst::model::FigureElem;

        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("main.typ"),
            "#figure(rect(width: 10pt, height: 4pt), caption: [Old cap])",
        )
        .unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let pre = normalize_list_item_runs(eval_to_content(&world).unwrap());

        // Find the FigureElem in the pre tree
        fn find_figure_pre(content: &Content) -> Option<Content> {
            if content.is::<FigureElem>() {
                return Some(content.clone());
            }
            if let Some(seq) = content.to_packed::<SequenceElem>() {
                for child in &seq.children {
                    if let Some(f) = find_figure_pre(child) {
                        return Some(f);
                    }
                }
            }
            if let Some(styled) = content.to_packed::<StyledElem>() {
                return find_figure_pre(&styled.child);
            }
            None
        }
        let figure_pre = find_figure_pre(&pre).expect("figure not found in pre tree");

        // Use a 2-element realized sequence (body + caption)
        let realized = seq([text("Body text"), text("Caption text")]);
        let node = annotate_realized(&figure_pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Figure));
        assert!(
            node.annotation
                .slots
                .iter()
                .any(|s| matches!(s.label, SlotStep::FigureBody))
        );
        assert!(
            node.annotation
                .slots
                .iter()
                .any(|s| matches!(s.label, SlotStep::FigureCaption))
        );
    }

    #[test]
    fn annotate_each_wrapper_kind_sets_correct_semantic_kind() {
        // Wrappers are handled by wrapper_kind_of; verify at least Align and Block
        use typst::layout::{AlignElem, BlockBody, BlockElem};

        let align_pre = Content::new(AlignElem::new(text("body")));
        let node = annotate_realized(&align_pre, &align_pre);
        assert_eq!(
            node.annotation.semantic_kind,
            Some(SemanticKind::Wrapper(WrapperKind::Align))
        );

        let block_pre =
            Content::new(BlockElem::new().with_body(Some(BlockBody::Content(text("body")))));
        let node = annotate_realized(&block_pre, &block_pre);
        assert_eq!(
            node.annotation.semantic_kind,
            Some(SemanticKind::Wrapper(WrapperKind::Block))
        );
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
        // through both wrappers to find the cell bodies.
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
        let realized = Content::new(
            BlockElem::new()
                .with_body(Some(BlockBody::Content(Content::new(GridElem::new(cells))))),
        );
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::List));
        assert_eq!(node.annotation.slots.len(), 2);
        assert_eq!(node.annotation.slots[0].path, vec![0, 0]);
        assert_eq!(node.annotation.slots[1].path, vec![0, 1]);
        assert_eq!(
            node.get_path(&node.annotation.slots[0].path)
                .unwrap()
                .realized
                .plain_text(),
            "Alpha"
        );
        assert_eq!(
            node.get_path(&node.annotation.slots[1].path)
                .unwrap()
                .realized
                .plain_text(),
            "Beta"
        );
    }

    #[test]
    fn annotate_heading_with_numbering_keeps_heading_kind_at_outer_level() {
        use typst::model::HeadingElem;

        let pre = Content::new(HeadingElem::new(text("Intro")));
        let realized = seq([text("1"), text(" "), text("Intro")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Heading));
        assert!(
            node.children.is_empty(),
            "heading is a leaf in the annotated tree"
        );
        assert!(node.realized.plain_text().contains('1'));
        assert!(node.realized.plain_text().contains("Intro"));
    }

    #[test]
    fn annotate_grid_maps_each_cell_by_document_order_index() {
        use typst::foundations::Packed;
        use typst::layout::{GridCell, GridChild, GridElem, GridItem};

        let pre = Content::new(GridElem::new(vec![
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("X"))))),
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("Y"))))),
        ]));
        let realized = seq([text("X"), text("Y")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Grid));
        assert_eq!(node.annotation.slots.len(), 2);
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::GridCell(0)
        ));
        assert!(matches!(
            node.annotation.slots[1].label,
            SlotStep::GridCell(1)
        ));
        assert_eq!(node.children[0].realized.plain_text(), "X");
        assert_eq!(node.children[1].realized.plain_text(), "Y");
    }

    #[test]
    fn annotate_stack_maps_block_children() {
        use typst::layout::{StackChild, StackElem};

        let pre = Content::new(StackElem::new(vec![
            StackChild::Block(text("Block0")),
            StackChild::Block(text("Block1")),
        ]));
        let realized = seq([text("Block0"), text("Block1")]);
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Stack));
        assert_eq!(node.annotation.slots.len(), 2);
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::StackChild(0)
        ));
        assert!(matches!(
            node.annotation.slots[1].label,
            SlotStep::StackChild(1)
        ));
        assert_eq!(node.children[0].realized.plain_text(), "Block0");
        assert_eq!(node.children[1].realized.plain_text(), "Block1");
    }

    #[test]
    fn annotate_footnote_maps_body_as_single_slot() {
        use typst::model::{FootnoteBody, FootnoteElem};

        let body_content = text("Footnote text");
        let pre = Content::new(FootnoteElem::new(FootnoteBody::Content(body_content)));
        let realized = text("Footnote text");
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Footnote));
        assert_eq!(node.annotation.slots.len(), 1);
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::FootnoteBody
        ));
        assert_eq!(node.children[0].realized.plain_text(), "Footnote text");
    }

    #[test]
    fn annotate_quote_maps_body_as_single_slot() {
        use typst::model::QuoteElem;

        let pre = Content::new(QuoteElem::new(text("Quote body")));
        let realized = text("Quote body");
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Quote));
        assert_eq!(node.annotation.slots.len(), 1);
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::QuoteBody
        ));
        assert_eq!(node.annotation.slots[0].path, vec![0]);
        assert_eq!(node.children[0].realized.plain_text(), "Quote body");
    }

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

    #[test]
    fn annotate_footnote_marker_detects_styled_marker() {
        // Styled footnote markers (e.g. superscript) wrap TextElem in StyledElem.
        // annotate_footnote_markers must look through the wrapper.
        use typst::model::FootnoteBody;
        use typst::model::FootnoteElem;

        let footnote_body = text("Note body");
        let footnotes = vec![Content::new(FootnoteElem::new(FootnoteBody::Content(
            footnote_body,
        )))];

        // Simulate a realized tree where the marker is a styled "1" (e.g. superscript)
        let marker = text("1")
            .styled(TextElem::fill.set(typst::visualize::Color::from_u8(0, 0, 0, 255).into()));
        let mut node = AnnotatedContent {
            realized: marker,
            annotation: Annotation::default(),
            children: vec![],
        };
        let mut next = 0;
        annotate_footnote_markers(&mut node, &footnotes, &mut next);

        assert_eq!(next, 1, "footnote should have been matched");
        assert!(
            node.annotation.footnote.is_some(),
            "footnote info should be attached"
        );
    }
}
