//! Annotated realized content tree.
//!
//! [`AnnotatedContent`] pairs a realized [`Content`] node with semantic
//! information recovered from the pre-realization tree. The realized side is
//! preserved exactly as Typst produced it; annotations are built once and
//! never mutated.

use crate::container_ops::{self, ContainerKind};
use crate::content_tree;
use std::collections::VecDeque;
use typst::foundations::{Content, ContextElem, SequenceElem, StyleChain, StyledElem};
use typst::introspection::{Tag, TagElem};
use typst::layout::{BlockBody, BlockElem};
use typst::math::EquationElem;
use typst::model::{FootnoteBody, FootnoteElem, HeadingElem, ParElem, ParbreakElem};
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

pub(crate) fn effective_content_with(
    node: &AnnotatedContent,
    surface_is_sufficient: impl Fn(&Content) -> bool + Copy,
) -> Content {
    let surface = node
        .annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized);
    if surface_is_sufficient(surface) || node.children.is_empty() {
        return surface.clone();
    }
    Content::sequence(
        node.children
            .iter()
            .map(|child| effective_content_with(child, surface_is_sufficient)),
    )
}

pub(crate) fn effective_text_content(node: &AnnotatedContent) -> Content {
    effective_content_with(node, |surface| !surface.plain_text().is_empty())
}

pub(crate) fn effective_render_content(node: &AnnotatedContent) -> Content {
    effective_content_with(node, |surface| !surface.is_empty())
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
    /// Structured content to use as the local edit surface when realization is
    /// opaque or when realized layout scaffolding differs from authored slots.
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
    Quote,
    Equation,
    Wrapper(WrapperKind),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
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
/// `path` is relative to the node's patch surface, not incidental realized
/// layout scaffolding. `label` identifies the slot's role (e.g. `ListItem(0)`).
#[derive(Clone, Debug)]
pub struct SemanticSlot {
    pub label: SlotStep,
    pub path: Vec<usize>,
    pub patch_path: Option<Vec<usize>>,
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
    if pre.to_packed::<SequenceElem>().is_some()
        && let Some(styled) = realized.to_packed::<StyledElem>()
    {
        let child =
            container_ops::materialize_style_dependent_fields(&styled.child, &styled.styles);
        let inner = annotate_realized(pre, &child);
        let patch_surface = inner
            .annotation
            .patch_surface
            .clone()
            .map(|surface| surface.styled_with_map(styled.styles.clone()));
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                patch_surface: patch_surface.filter(|surface| surface != realized),
                span: pre.span(),
                ..Annotation::default()
            },
            children: inner.children,
        };
    }

    // Symmetric root/wrapper mismatch: normalization can leave the pre tree
    // styled while realization has already pushed or discarded that wrapper.
    // Treat the pre-only style wrapper as transparent so semantic children are
    // still recovered from the authored body.
    if let Some(styled) = pre.to_packed::<StyledElem>()
        && realized.to_packed::<StyledElem>().is_none()
    {
        let child =
            container_ops::materialize_style_dependent_fields(&styled.child, &styled.styles);
        let inner = annotate_realized(&child, realized);
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                patch_surface: inner.annotation.patch_surface,
                span: pre.span(),
                ..Annotation::default()
            },
            children: inner.children,
        };
    }

    // --- Structural elements: semantic_kind + slot map ---
    if pre.is::<FootnoteElem>() {
        return annotate_footnote_element(pre, realized);
    }
    if pre.is::<ContextElem>() {
        if realized.is::<ContextElem>() {
            return leaf_annotated(
                realized,
                Annotation {
                    span: pre.span(),
                    ..Annotation::default()
                },
            );
        }
        let semantic =
            crate::context_recording::take(pre.span()).unwrap_or_else(|| realized.clone());
        let mut node = annotate_realized(&semantic, realized);
        if !node.annotation.slots.is_empty()
            && node.annotation.patch_surface.is_none()
            && realized.plain_text().trim().is_empty()
            && !semantic.plain_text().trim().is_empty()
        {
            node.annotation.patch_surface = Some(semantic);
        }
        node.annotation.span = pre.span();
        return node;
    }
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
        return annotate_heading(pre, realized);
    }
    if pre.is::<RawElem>() {
        return leaf_annotated(
            realized,
            Annotation {
                semantic_kind: Some(SemanticKind::RawBlock),
                patch_surface: (pre != realized).then_some(pre.clone()),
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
        let children: Vec<AnnotatedContent> =
            if sequence_contains_footnote(pre_seq) && sequence_has_paragraph_blocks(real_seq) {
                pair_sequence_with_footnote_paragraphs(&pre_seq.children, &real_seq.children)
            } else if pre_seq.children.len() == real_seq.children.len()
                && !sequence_needs_context_pairing(&pre_seq.children, &real_seq.children)
            {
                pre_seq
                    .children
                    .iter()
                    .zip(real_seq.children.iter())
                    .map(|(p, r)| annotate_realized(p, r))
                    .collect()
            } else {
                pair_sequence_by_span(&pre_seq.children, &real_seq.children)
            };
        let patch_surface = sequence_patch_surface(&children);
        let has_layout_surface = children.len() != real_seq.children.len()
            && children
                .iter()
                .any(|child| child.realized.is::<ParbreakElem>());
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                patch_surface: has_layout_surface.then_some(patch_surface),
                span: pre.span(),
                ..Annotation::default()
            },
            children,
        };
    }
    if pre.to_packed::<SequenceElem>().is_some()
        && let Some(real_p) = realized.to_packed::<ParElem>()
    {
        let child = annotate_realized(pre, &real_p.body);
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
    if let Some(pre_seq) = pre.to_packed::<SequenceElem>() {
        let children: Vec<AnnotatedContent> = pre_seq
            .children
            .iter()
            .map(|child| annotate_realized(child, child))
            .collect();
        let patch_surface = sequence_patch_surface(&children);
        let has_layout_surface = children
            .iter()
            .any(|child| child.realized.is::<ParbreakElem>());
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: Annotation {
                patch_surface: has_layout_surface.then_some(patch_surface),
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
        let pre_child =
            container_ops::materialize_style_dependent_fields(&pre_s.child, &pre_s.styles);
        let real_child =
            container_ops::materialize_style_dependent_fields(&real_s.child, &real_s.styles);
        let child = annotate_realized(&pre_child, &real_child);
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
    if let Some((semantic, span)) = recorded_context_descendant(realized) {
        let mut node = annotate_realized(&semantic, realized);
        if !node.annotation.slots.is_empty()
            && node.annotation.patch_surface.is_none()
            && realized.plain_text().trim().is_empty()
            && !semantic.plain_text().trim().is_empty()
        {
            node.annotation.patch_surface = Some(semantic);
        }
        node.annotation.span = span;
        return node;
    }

    if is_opaque_visual_element_name(pre.func().name())
        && realized.plain_text().trim().is_empty()
        && pre != realized
    {
        return leaf_annotated(
            realized,
            Annotation {
                patch_surface: Some(pre.clone()),
                span: pre.span(),
                ..Annotation::default()
            },
        );
    }

    leaf_annotated(
        realized,
        Annotation {
            semantic_kind: semantic_kind_of(pre),
            span: pre.span(),
            ..Annotation::default()
        },
    )
}

fn sequence_contains_footnote(seq: &SequenceElem) -> bool {
    seq.children.iter().any(content_contains_footnote)
}

fn content_contains_footnote(content: &Content) -> bool {
    if content.is::<FootnoteElem>() {
        return true;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.iter().any(content_contains_footnote);
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return content_contains_footnote(&styled.child);
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return content_contains_footnote(&par.body);
    }
    false
}

fn content_contains_context(content: &Content) -> bool {
    if content.is::<ContextElem>() {
        return true;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.iter().any(content_contains_context);
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return content_contains_context(&styled.child);
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return content_contains_context(&par.body);
    }
    false
}

fn sequence_has_paragraph_blocks(seq: &SequenceElem) -> bool {
    seq.children
        .iter()
        .any(|child| paragraph_body_path(child).is_some())
}

fn pair_sequence_with_footnote_paragraphs(
    pre_children: &[Content],
    real_children: &[Content],
) -> Vec<AnnotatedContent> {
    let runs = source_paragraph_runs(pre_children);
    let mut run_index = 0;
    let mut out = Vec::with_capacity(real_children.len());

    for real_child in real_children {
        if paragraph_body_path(real_child).is_some() && run_index < runs.len() {
            let run = &runs[run_index];
            run_index += 1;
            if run.iter().any(content_contains_footnote) {
                out.push(annotate_footnote_paragraph_run(run, real_child));
            } else {
                let pre_run = Content::sequence(run.iter().cloned());
                out.push(annotate_realized(&pre_run, real_child));
            }
        } else {
            out.push(leaf_annotated(real_child, Annotation::default()));
        }
    }

    out
}

fn source_paragraph_runs(pre_children: &[Content]) -> Vec<Vec<Content>> {
    let mut runs = Vec::new();
    let mut current = Vec::new();

    for child in pre_children {
        if child.is::<ParbreakElem>() {
            if current.iter().any(|content: &Content| !content.is_empty()) {
                runs.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        } else {
            current.push(child.clone());
        }
    }

    if current.iter().any(|content| !content.is_empty()) {
        runs.push(current);
    }

    runs
}

fn annotate_footnote_paragraph_run(run: &[Content], realized: &Content) -> AnnotatedContent {
    let pre_body = Content::sequence(run.iter().cloned());
    let Some(body_path) = paragraph_body_path(realized) else {
        return annotate_realized(&pre_body, realized);
    };
    let Some(patch_surface) =
        content_tree::replace_realized_content_at_path(realized, &body_path, pre_body)
    else {
        return annotate_realized(&Content::sequence(run.iter().cloned()), realized);
    };
    let patch_tree = annotate_realized(&patch_surface, &patch_surface);
    let mut slots = Vec::new();
    collect_promoted_footnote_slots(&patch_tree, &mut Vec::new(), &mut slots);
    if slots.is_empty() {
        return patch_tree;
    }

    AnnotatedContent {
        realized: realized.clone(),
        annotation: Annotation {
            semantic_kind: Some(SemanticKind::Paragraph),
            slots,
            patch_surface: Some(patch_surface),
            span: run
                .iter()
                .find(|content| !content.span().is_detached())
                .map(Content::span)
                .unwrap_or_else(Span::detached),
            ..Annotation::default()
        },
        children: patch_tree.children,
    }
}

fn annotate_footnote_element(pre: &Content, realized: &Content) -> AnnotatedContent {
    let Some(pre_body) = footnote_body(pre) else {
        return leaf_annotated(
            realized,
            Annotation {
                span: pre.span(),
                ..Annotation::default()
            },
        );
    };
    let realized_body = footnote_body(realized).unwrap_or_else(|| pre_body.clone());
    let mut body = annotate_realized(&pre_body, &realized_body);
    body.annotation.span = pre.span();
    AnnotatedContent {
        realized: realized.clone(),
        annotation: Annotation {
            slots: vec![SemanticSlot {
                label: SlotStep::FootnoteBody,
                path: vec![0],
                patch_path: None,
            }],
            span: pre.span(),
            ..Annotation::default()
        },
        children: vec![body],
    }
}

fn footnote_body(content: &Content) -> Option<Content> {
    let footnote = content.to_packed::<FootnoteElem>()?;
    let FootnoteBody::Content(body) = &footnote.body else {
        return None;
    };
    Some(body.clone())
}

fn paragraph_body_path(content: &Content) -> Option<Vec<usize>> {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let mut path = paragraph_body_path(&styled.child)?;
        path.insert(0, 0);
        return Some(path);
    }
    content.is::<ParElem>().then_some(vec![0])
}

fn collect_promoted_footnote_slots(
    node: &AnnotatedContent,
    path: &mut Vec<usize>,
    out: &mut Vec<SemanticSlot>,
) {
    for slot in &node.annotation.slots {
        if matches!(slot.label, SlotStep::FootnoteBody) {
            let mut slot_path = path.clone();
            slot_path.extend(slot.path.iter().copied());
            out.push(SemanticSlot {
                label: SlotStep::FootnoteBody,
                path: slot_path,
                patch_path: None,
            });
        }
    }

    for (index, child) in node.children.iter().enumerate() {
        path.push(index);
        collect_promoted_footnote_slots(child, path, out);
        path.pop();
    }
}

fn leaf_annotated(realized: &Content, annotation: Annotation) -> AnnotatedContent {
    AnnotatedContent {
        realized: realized.clone(),
        annotation,
        children: vec![],
    }
}

fn annotated_surface(node: &AnnotatedContent) -> Content {
    node.annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized)
        .clone()
}

fn sequence_patch_surface(children: &[AnnotatedContent]) -> Content {
    Content::sequence(children.iter().map(annotated_surface))
}

fn heading_annotation(pre: &Content) -> Annotation {
    Annotation {
        semantic_kind: Some(SemanticKind::Heading),
        span: pre.span(),
        ..Annotation::default()
    }
}

fn annotate_heading(pre: &Content, realized: &Content) -> AnnotatedContent {
    if let Some(styled) = realized.to_packed::<StyledElem>()
        && styled.child.is::<BlockElem>()
    {
        return AnnotatedContent {
            realized: realized.clone(),
            annotation: heading_annotation(pre),
            children: vec![leaf_annotated(&styled.child, heading_annotation(pre))],
        };
    }

    leaf_annotated(realized, heading_annotation(pre))
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
/// children become anonymous leaves. Layout-bearing pre children with no
/// realized partner, such as `ParbreakElem`, are preserved without consuming a
/// realized child. Other unmatched pre children use the existing positional
/// fallback. Trailing realized children become anonymous leaves.
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
    for (pre_index, pre_child) in pre_children.iter().enumerate() {
        if pre_child.is::<ParbreakElem>() {
            out.push(annotate_realized(pre_child, pre_child));
            continue;
        }

        if pre_child.is::<ContextElem>() && cursor < real_children.len() {
            let fallback_end =
                next_pre_child_span_match(pre_children, pre_index + 1, real_children, cursor)
                    .unwrap_or(real_children.len());
            let match_idx = if find_context_descendant_span(&real_children[cursor]).is_some() {
                None
            } else {
                let target = pre_child.span();
                (!target.is_detached()).then(|| {
                    real_children[cursor..fallback_end]
                        .iter()
                        .enumerate()
                        .find_map(|(offset, real_child)| {
                            (context_realized_span_matches(real_child, target)
                                && !is_invisible_realized_child(real_child))
                            .then_some(cursor + offset)
                        })
                        .or_else(|| {
                            real_children[fallback_end..].iter().enumerate().find_map(
                                |(offset, real_child)| {
                                    (effective_span(real_child) == target
                                        && !is_invisible_realized_child(real_child))
                                    .then_some(fallback_end + offset)
                                },
                            )
                        })
                        .or_else(|| {
                            let semantic_text =
                                crate::context_recording::peek(target)?.plain_text();
                            (!semantic_text.is_empty()).then_some(())?;
                            real_children[cursor..].iter().enumerate().find_map(
                                |(offset, real_child)| {
                                    (real_child.plain_text() == semantic_text
                                        && !is_invisible_realized_child(real_child))
                                    .then_some(cursor + offset)
                                },
                            )
                        })
                })
            };
            if let Some(idx) = match_idx.flatten() {
                while cursor < idx {
                    cursor = push_unmatched_realized_child(&mut out, real_children, cursor);
                }
                let end = context_owner_realized_run_end(
                    pre_child,
                    pre_children,
                    pre_index,
                    real_children,
                    idx,
                );
                if end > idx + 1 {
                    let realized_run = Content::sequence(real_children[idx..end].iter().cloned());
                    out.push(annotate_realized(pre_child, &realized_run));
                    cursor = end;
                } else {
                    out.push(annotate_realized(pre_child, &real_children[idx]));
                    cursor = idx + 1;
                }
            } else {
                let realized_run =
                    Content::sequence(real_children[cursor..fallback_end].iter().cloned());
                out.push(annotate_realized(pre_child, &realized_run));
                cursor = fallback_end;
            }
            continue;
        }

        let target = pre_child.span();
        let mut match_idx = None;
        for (idx, real_child) in real_children.iter().enumerate().skip(cursor) {
            if is_invisible_pre_child(pre_child) && !is_invisible_realized_child(real_child) {
                continue;
            }
            if effective_span(real_child) == target {
                match_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = match_idx {
            while cursor < idx {
                cursor = push_unmatched_realized_child(&mut out, real_children, cursor);
            }
            let end = context_owner_realized_run_end(
                pre_child,
                pre_children,
                pre_index,
                real_children,
                idx,
            );
            if end > idx + 1 {
                let realized_run = Content::sequence(real_children[idx..end].iter().cloned());
                out.push(annotate_realized(pre_child, &realized_run));
                cursor = end;
            } else {
                out.push(annotate_realized(pre_child, &real_children[idx]));
                cursor = idx + 1;
            }
            continue;
        }

        // If no span match exists, fall back to positional pairing instead of
        // dropping the pre node. This preserves structural semantics when
        // realization rewrites spans on wrapper-heavy sequences.
        if cursor < real_children.len() && is_invisible_pre_child(pre_child) {
            out.push(annotate_realized(pre_child, pre_child));
            continue;
        }

        if cursor < real_children.len()
            && find_context_descendant_span(&real_children[cursor]).is_some()
        {
            cursor = push_unmatched_realized_child(&mut out, real_children, cursor);
            if is_invisible_pre_child(pre_child) || cursor >= real_children.len() {
                out.push(annotate_realized(pre_child, pre_child));
                continue;
            }
        }

        if cursor < real_children.len() {
            out.push(annotate_realized(pre_child, &real_children[cursor]));
            cursor += 1;
        }
    }
    // Any trailing unmatched realized children become anonymous leaves.
    while cursor < real_children.len() {
        cursor = push_unmatched_realized_child(&mut out, real_children, cursor);
    }
    out
}

fn sequence_needs_context_pairing(pre_children: &[Content], real_children: &[Content]) -> bool {
    pre_children.iter().any(content_contains_context)
        || real_children
            .iter()
            .any(|child| find_context_descendant_span(child).is_some())
}

fn context_realized_span_matches(realized: &Content, target: Span) -> bool {
    effective_span(realized) == target || find_context_descendant_span(realized) == Some(target)
}

fn context_owner_realized_run_end(
    pre_child: &Content,
    pre_children: &[Content],
    pre_index: usize,
    real_children: &[Content],
    match_idx: usize,
) -> usize {
    if !content_contains_context(pre_child) {
        return match_idx + 1;
    }

    let limit =
        next_pre_child_span_match(pre_children, pre_index + 1, real_children, match_idx + 1)
            .unwrap_or(real_children.len());
    let mut end = match_idx + 1;
    let mut included_context_output = false;
    while end < limit {
        let child = &real_children[end];
        let has_context_output = find_context_descendant_span(child).is_some();
        if has_context_output {
            included_context_output = true;
            end += 1;
            break;
        }
        if is_invisible_realized_child(child) {
            end += 1;
            continue;
        }
        break;
    }

    if included_context_output {
        end
    } else {
        match_idx + 1
    }
}

fn push_unmatched_realized_child(
    out: &mut Vec<AnnotatedContent>,
    real_children: &[Content],
    cursor: usize,
) -> usize {
    if let Some((end, semantic, span)) = recorded_context_run_at(real_children, cursor) {
        let realized_run = Content::sequence(real_children[cursor..end].iter().cloned());
        let mut node = annotate_realized(&semantic, &realized_run);
        node.annotation.span = span;
        out.push(node);
        return end;
    }

    out.push(leaf_annotated(
        &real_children[cursor],
        Annotation::default(),
    ));
    cursor + 1
}

fn recorded_context_run_at(
    real_children: &[Content],
    cursor: usize,
) -> Option<(usize, Content, Span)> {
    let start = real_children.get(cursor)?.to_packed::<TagElem>()?;
    let Tag::Start(tagged, _) = &start.tag else {
        return None;
    };
    if !tagged.is::<ContextElem>() {
        return None;
    }
    let span = tagged.span();
    let semantic = crate::context_recording::take(span)?;
    let location = start.tag.location();
    for (index, child) in real_children.iter().enumerate().skip(cursor + 1) {
        if let Some(tag) = child.to_packed::<TagElem>()
            && let Tag::End(end_location, _, _) = tag.tag
            && end_location == location
        {
            return Some((index + 1, semantic, span));
        }
    }
    Some((cursor + 1, semantic, span))
}

fn recorded_context_descendant(realized: &Content) -> Option<(Content, Span)> {
    let span = find_context_descendant_span(realized)?;
    crate::context_recording::take(span).map(|semantic| (semantic, span))
}

fn find_context_descendant_span(realized: &Content) -> Option<Span> {
    if let Some(tag) = realized.to_packed::<TagElem>()
        && let Tag::Start(tagged, _) = &tag.tag
    {
        if tagged.is::<ContextElem>() {
            return Some(tagged.span());
        }
    }

    container_ops::realized_child_contents(realized)
        .iter()
        .find_map(find_context_descendant_span)
}

fn is_invisible_pre_child(content: &Content) -> bool {
    content.plain_text().trim().is_empty()
}

fn is_invisible_realized_child(content: &Content) -> bool {
    content.plain_text().trim().is_empty()
}

fn is_opaque_visual_element_name(name: &str) -> bool {
    matches!(
        name,
        "rect" | "circle" | "ellipse" | "line" | "polygon" | "path" | "image"
    )
}

fn next_pre_child_span_match(
    pre_children: &[Content],
    start_pre: usize,
    real_children: &[Content],
    cursor: usize,
) -> Option<usize> {
    for pre_child in pre_children.iter().skip(start_pre) {
        if pre_child.is::<ParbreakElem>() {
            continue;
        }
        let target = pre_child.span();
        if target.is_detached() {
            continue;
        }
        for (idx, real_child) in real_children.iter().enumerate().skip(cursor) {
            if effective_span(real_child) == target {
                return Some(idx);
            }
        }
    }
    None
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
    if node.annotation.semantic_kind == Some(SemanticKind::Equation) {
        node.annotation.equation_origins = (0..1).filter_map(|_| equations.pop_front()).collect();
        return;
    }

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
    content.is::<EquationElem>() || matches!(content.func().name(), "inline" | "display")
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
    fn empty_block_is_not_an_equation_origin_target() {
        use typst::layout::{BlockBody, BlockElem};

        let pre = Content::new(EquationElem::new(text("x")));
        let mut node = AnnotatedContent {
            realized: Content::new(
                BlockElem::new().with_body(Some(BlockBody::Content(Content::sequence([])))),
            ),
            annotation: Annotation::default(),
            children: vec![],
        };

        annotate_equation_origins(&pre, &mut node);

        assert!(
            node.annotation.equation_origins.is_empty(),
            "empty structural blocks must not consume source equation provenance"
        );
    }

    #[test]
    fn semantic_equation_node_receives_equation_origin_even_when_realized_empty() {
        use typst::layout::{BlockBody, BlockElem};

        let pre = Content::new(EquationElem::new(text("x")));
        let mut node = AnnotatedContent {
            realized: Content::new(
                BlockElem::new().with_body(Some(BlockBody::Content(Content::sequence([])))),
            ),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Equation),
                ..Annotation::default()
            },
            children: vec![],
        };

        annotate_equation_origins(&pre, &mut node);

        assert_eq!(node.annotation.equation_origins.len(), 1);
        assert!(node.annotation.equation_origins[0].is::<EquationElem>());
    }

    fn contains_kind(node: &AnnotatedContent, kind: &SemanticKind) -> bool {
        node.annotation.semantic_kind.as_ref() == Some(kind)
            || node.children.iter().any(|child| contains_kind(child, kind))
    }

    #[test]
    fn effective_text_content_recurses_through_text_empty_surface() {
        use typst::model::ParbreakElem;

        let surface = Content::sequence([Content::new(ParbreakElem::new())]);
        assert!(surface.plain_text().is_empty());
        let node = AnnotatedContent {
            realized: text("realized"),
            annotation: Annotation {
                patch_surface: Some(surface),
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: text("semantic child"),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };

        assert_eq!(effective_text_content(&node).plain_text(), "semantic child");
    }

    #[test]
    fn effective_render_content_preserves_structurally_nonempty_surface() {
        use typst::model::ParbreakElem;

        let surface = Content::sequence([Content::new(ParbreakElem::new())]);
        assert!(!surface.is_empty());
        assert!(surface.plain_text().is_empty());
        let node = AnnotatedContent {
            realized: text("realized"),
            annotation: Annotation {
                patch_surface: Some(surface.clone()),
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: text("semantic child"),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };

        assert_eq!(effective_render_content(&node), surface);
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
    fn annotate_sequence_preserves_unrealized_parbreak_before_list() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem, ParbreakElem};

        let list = Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
            "item",
        )))]));
        let pre = seq([
            text("Intro"),
            Content::new(ParbreakElem::new()),
            list.clone(),
        ]);
        let realized = seq([text("Intro"), list.clone()]);

        let node = annotate_realized(&pre, &realized);
        let surface = node
            .annotation
            .patch_surface
            .as_ref()
            .expect("parbreak-preserving surface should differ from realized");

        assert_eq!(node.children.len(), 3);
        assert!(node.children[1].realized.is::<ParbreakElem>());
        assert!(node.children[2].realized.is::<ListElem>());
        assert!(surface.to_packed::<SequenceElem>().is_some_and(|seq| {
            seq.children.len() == 3
                && seq.children[1].is::<ParbreakElem>()
                && seq.children[2].is::<ListElem>()
        }));
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
    fn annotate_list_item_slot_preserves_nested_list_semantics() {
        use typst::foundations::Packed;
        use typst::model::{ListElem, ListItem};

        let nested_items = seq([
            text("Parent"),
            Content::new(ListItem::new(text("Nested A"))),
            Content::new(ListItem::new(text("Nested B"))),
        ]);
        let pre = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            nested_items,
        ))]));
        let node = annotate_realized(&pre, &pre);
        let child = node.get_path(&node.annotation.slots[0].path).unwrap();

        assert!(contains_kind(child, &SemanticKind::List));
    }

    #[test]
    fn annotate_table_cell_slot_preserves_nested_list_semantics() {
        use typst::foundations::Packed;
        use typst::model::{ListItem, TableCell, TableChild, TableElem, TableItem};

        let cell = seq([
            Content::new(ListItem::new(text("Nested A"))),
            Content::new(ListItem::new(text("Nested B"))),
        ]);
        let pre = Content::new(TableElem::new(vec![TableChild::Item(TableItem::Cell(
            Packed::new(TableCell::new(cell)),
        ))]));
        let node = annotate_realized(&pre, &pre);
        let child = node.get_path(&node.annotation.slots[0].path).unwrap();

        assert!(contains_kind(child, &SemanticKind::List));
    }

    #[test]
    fn annotate_wrapper_slot_preserves_nested_list_semantics() {
        use typst::layout::{BlockBody, BlockElem};
        use typst::model::ListItem;

        let body = seq([
            Content::new(ListItem::new(text("Nested A"))),
            Content::new(ListItem::new(text("Nested B"))),
        ]);
        let pre = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(body))));
        let node = annotate_realized(&pre, &pre);
        let child = node.get_path(&node.annotation.slots[0].path).unwrap();

        assert!(contains_kind(child, &SemanticKind::List));
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
    fn annotate_block_wrapper_body_slot_points_to_whole_body() {
        use typst::layout::{BlockBody, BlockElem};
        use typst::model::StrongElem;

        let body = seq([
            Content::new(StrongElem::new(text("Label"))),
            text(" -- changed body"),
        ]);
        let block = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(body))));
        let node = annotate_realized(&block, &block);

        assert_eq!(
            node.annotation.semantic_kind,
            Some(SemanticKind::Wrapper(WrapperKind::Block))
        );
        assert_eq!(node.annotation.slots.len(), 1);
        assert_eq!(node.annotation.slots[0].label, SlotStep::WrapperBody);
        assert_eq!(node.annotation.slots[0].path, vec![0]);
        assert_eq!(
            node.get_path(&node.annotation.slots[0].path)
                .unwrap()
                .plain_text(),
            "Label -- changed body"
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
    fn annotate_realized_heading_block_remembers_heading_origin() {
        use typst::model::HeadingElem;
        use typst::visualize::Color;

        let pre = Content::new(HeadingElem::new(text("Intro")));
        let block =
            Content::new(BlockElem::new().with_body(Some(BlockBody::Content(text("Intro")))));
        let realized = block.styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let node = annotate_realized(&pre, &realized);

        assert_eq!(node.annotation.semantic_kind, Some(SemanticKind::Heading));
        assert_eq!(node.children.len(), 1);
        assert!(node.children[0].realized.is::<BlockElem>());
        assert_eq!(
            node.children[0].annotation.semantic_kind,
            Some(SemanticKind::Heading)
        );
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

        assert_eq!(node.annotation.semantic_kind, None);
        assert_eq!(node.annotation.slots.len(), 1);
        assert!(matches!(
            node.annotation.slots[0].label,
            SlotStep::FootnoteBody
        ));
        assert_eq!(node.annotation.slots[0].path, vec![0]);
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
