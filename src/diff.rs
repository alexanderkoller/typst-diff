//! Two-level document diff: block-level LCS followed by word-level diff.
//!
//! # Algorithm overview
//!
//! 1. **Block extraction** — [`extract_block_units`] / [`extract_blocks`] walk the
//!    realized `Content` tree and segment it into [`DiffBlock`] values (paragraphs,
//!    headings, raw blocks, display equations, tables, …), carrying the `PageElem`
//!    styles that were active at each block's position.
//!
//! 2. **Block-level LCS** — [`diff_block_units_raw`] wraps each block in
//!    [`HashableContent`] and feeds the slice to `similar::capture_diff_slices`
//!    (Myers algorithm). This produces `Equal / Delete / Insert` operations.
//!
//! 3. **Edit-zone matching** — [`match_edit_zones`] scans the raw ops for contiguous
//!    `Delete + Insert` zones and pairs each delete with its most-similar insert
//!    (similarity ≥ 0.3). Paired blocks become [`BlockOp::Replace`].
//!
//! 4. **Realized-tree edits** — [`diff_annotated`] drives all of the above, then
//!    recurses through semantic slots when a structured container can be matched.
//!    Leaf replacements use [`diff_words`] or an opaque visual replacement for
//!    realized visual leaves.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use similar::{Algorithm, DiffOp, capture_diff_slices};
use typst::World;
use typst::foundations::{
    Content, ContextElem, NativeElement, Repr, SequenceElem, Smart, Style, StyleChain, StyledElem,
    Styles,
};
use typst::introspection::{MetadataElem, StateUpdateElem, Tag, TagElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, Frame, FrameItem, GridChild, GridElem,
    GridItem, PadElem, PageElem, PagebreakElem, PagedDocument, PlaceElem, Point, Rel,
};
use typst::math::EquationElem;
use typst::model::{
    EmphElem, EnumElem, FigureCaption, FigureElem, FootnoteBody, FootnoteElem, HeadingElem,
    LinkElem, ListElem, ParElem, ParbreakElem, RefElem, StrongElem, TableChild, TableElem,
    TableItem,
};
use typst::text::{
    HighlightElem, LinebreakElem, OverlineElem, RawElem, RawLine, SpaceElem, StrikeElem, SubElem,
    SuperElem, TextElem, UnderlineElem,
};
use typst::visualize::{CircleElem, EllipseElem, RectElem};

use crate::annotated::{
    AnnotatedContent, SemanticKind, SemanticSlot, SlotStep, WrapperKind, annotate_realized,
    effective_render_content, effective_text_content,
};
use crate::container_ops;
use crate::trace::{
    DebugEventSink, FrameTraceEvent, PipelineTraceEvent, RenderedRegionTraceEnd,
    RenderedRegionTraceStart, emit_pipeline_trace_event,
};

/// A block-level unit of content together with the page styles active at its position.
///
/// `page_styles` is "sticky": if a block carries no page-style update of its own it
/// inherits the styles of the nearest preceding block that did. This means every block
/// always knows which `#set page(…)` context it belongs to, even across section breaks.
#[derive(Clone)]
pub struct DiffBlock {
    pub content: Content,
    pub page_styles: Styles,
}

/// Stable semantic identity for pairing authored owners before text similarity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SemanticOwnerKey {
    kind: SemanticKind,
    slot_labels: Vec<SlotStep>,
    ordinal: usize,
}

/// Segment a `Content` tree into block-level units (page styles discarded).
///
/// Convenience wrapper around [`extract_block_units`] for callers (tests, etc.)
/// that don't need the accompanying `page_styles`.
pub fn extract_blocks(content: &Content) -> Vec<Content> {
    extract_block_units(content)
        .into_iter()
        .map(|block| block.content)
        .collect()
}

fn extract_block_units(content: &Content) -> Vec<DiffBlock> {
    let mut blocks = extract_block_units_with_styles(content, Styles::new());
    make_page_styles_sticky(&mut blocks);
    blocks
}

/// Propagate page styles forward so every block has the most-recently-set page context.
///
/// Blocks that originate from a `#set page(…)` call carry their own page styles;
/// sibling blocks that follow without any page-style update inherit the last seen one.
fn make_page_styles_sticky(blocks: &mut [DiffBlock]) {
    let mut current = Styles::new();
    for block in &mut *blocks {
        if !block.page_styles.is_empty() {
            current = block.page_styles.clone();
        }
        block.page_styles = current.clone();
    }
}

fn extract_block_units_with_styles(
    content: &Content,
    inherited_page_styles: Styles,
) -> Vec<DiffBlock> {
    let children: Vec<Content> = if let Some(seq) = content.to_packed::<SequenceElem>() {
        seq.children.clone()
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        let mut page_styles = inherited_page_styles;
        page_styles.apply(page_styles_from(&styled.styles));
        let styles = non_page_styles(&styled.styles);

        if let Some(seq) = styled.child.to_packed::<SequenceElem>() {
            if is_inline_sequence(seq) {
                return vec![DiffBlock {
                    content: apply_block_styles(paragraph_block(styled.child.clone()), &styles),
                    page_styles,
                }];
            }

            let extracted = extract_block_units_with_styles(&styled.child, page_styles);
            return extracted
                .into_iter()
                .map(|block| DiffBlock {
                    content: apply_block_styles(block.content, &styles),
                    page_styles: block.page_styles,
                })
                .collect();
        }

        return vec![DiffBlock {
            content: apply_block_styles(styled.child.clone(), &styles),
            page_styles,
        }];
    } else {
        return vec![DiffBlock {
            content: content.clone(),
            page_styles: inherited_page_styles,
        }];
    };

    let mut blocks: Vec<DiffBlock> = Vec::new();
    let mut para: Vec<Content> = Vec::new();

    collect_blocks_from_children(
        children,
        inherited_page_styles.clone(),
        &mut para,
        &mut blocks,
    );
    flush_para(&mut para, &mut blocks, &inherited_page_styles);
    blocks
}

/// Iterate `children`, flushing accumulated inline content into paragraph blocks
/// whenever a block-level element is encountered.
///
/// Inline content accumulates in `para`; block-level triggers (`ParbreakElem`,
/// `HeadingElem`, `RawElem`, display equations, or any unknown element) flush `para`
/// first, then push themselves. `StyledElem` wrappers are unwrapped and their styles
/// are pushed down onto their children.
fn collect_blocks_from_children(
    children: Vec<Content>,
    page_styles: Styles,
    para: &mut Vec<Content>,
    blocks: &mut Vec<DiffBlock>,
) {
    for child in children {
        if let Some(styled) = child.to_packed::<StyledElem>() {
            let mut child_page_styles = page_styles.clone();
            let child_page_style_updates = page_styles_from(&styled.styles);
            let has_page_style_updates = !child_page_style_updates.is_empty();
            child_page_styles.apply(child_page_style_updates);
            let styles = non_page_styles(&styled.styles);

            if let Some(seq) = styled.child.to_packed::<SequenceElem>() {
                if is_inline_sequence(seq) {
                    if has_page_style_updates {
                        flush_para(para, blocks, &page_styles);
                        blocks.push(DiffBlock {
                            content: apply_block_styles(
                                paragraph_block(styled.child.clone()),
                                &styles,
                            ),
                            page_styles: child_page_styles.clone(),
                        });
                    } else {
                        para.push(child);
                    }
                    continue;
                }

                flush_para(para, blocks, &page_styles);
                let extracted =
                    extract_block_units_with_styles(&styled.child, child_page_styles.clone());
                blocks.extend(extracted.into_iter().map(|block| DiffBlock {
                    content: apply_block_styles(block.content, &styles),
                    page_styles: block.page_styles,
                }));
            } else if !has_page_style_updates && is_known_inline(&styled.child) {
                para.push(child);
            } else {
                flush_para(para, blocks, &page_styles);
                blocks.push(DiffBlock {
                    content: apply_block_styles(styled.child.clone(), &styles),
                    page_styles: child_page_styles.clone(),
                });
            }
        } else if let Some(seq) = child.to_packed::<SequenceElem>() {
            if is_inline_sequence(seq) {
                para.push(child);
            } else {
                collect_blocks_from_children(
                    seq.children.clone(),
                    page_styles.clone(),
                    para,
                    blocks,
                );
            }
        } else if child.is::<ParbreakElem>()
            || child.is::<HeadingElem>()
            || child.is::<RawElem>()
            || is_display_equation(&child)
        {
            flush_para(para, blocks, &page_styles);
            blocks.push(DiffBlock {
                content: child,
                page_styles: page_styles.clone(),
            });
        } else if is_known_inline(&child) {
            para.push(child);
        } else {
            flush_para(para, blocks, &page_styles);
            blocks.push(DiffBlock {
                content: child,
                page_styles: page_styles.clone(),
            });
        }
    }
}

/// Drain `para` into a single `ParElem` block if it contains any non-space content.
fn flush_para(para: &mut Vec<Content>, blocks: &mut Vec<DiffBlock>, page_styles: &Styles) {
    let nonempty = para.iter().any(|c| !c.is::<SpaceElem>());
    if nonempty {
        let content = paragraph_block(Content::sequence(para.drain(..)));
        blocks.push(DiffBlock {
            content,
            page_styles: page_styles.clone(),
        });
    } else {
        para.clear();
    }
}

fn paragraph_block(content: Content) -> Content {
    if content.is::<ParElem>() {
        content
    } else {
        Content::new(ParElem::new(normalize_text_runs(content)))
    }
}

/// Coalesce adjacent `TextElem` and `SpaceElem` nodes into single `TextElem` strings.
///
/// The Myers LCS algorithm hashes block content for equality checks. Without
/// normalization, two identical paragraphs that happen to be split into different
/// numbers of `TextElem` nodes (due to show rules or markup boundaries) would hash
/// differently and be treated as changed. Merging contiguous text runs makes equality
/// hash-stable.
fn normalize_text_runs(content: Content) -> Content {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        let mut children = Vec::new();
        let mut text = String::new();

        for child in &seq.children {
            if let Some(elem) = child.to_packed::<TextElem>() {
                text.push_str(elem.text.as_str());
            } else if child.is::<SpaceElem>() {
                text.push(' ');
            } else {
                flush_text_run(&mut children, &mut text);
                children.push(normalize_text_runs(child.clone()));
            }
        }

        flush_text_run(&mut children, &mut text);
        return Content::sequence(children);
    }

    if let Some(styled) = content.to_packed::<StyledElem>() {
        return normalize_text_runs(styled.child.clone()).styled_with_map(styled.styles.clone());
    }

    content
}

fn flush_text_run(children: &mut Vec<Content>, text: &mut String) {
    if !text.is_empty() {
        children.push(TextElem::packed(text.as_str()));
        text.clear();
    }
}

fn is_display_equation(c: &Content) -> bool {
    c.to_packed::<EquationElem>()
        .is_some_and(|eq| eq.block.get(StyleChain::default()))
}

fn is_known_inline(c: &Content) -> bool {
    use typst::model::{EmphElem, LinkElem, StrongElem};
    use typst::text::{
        HighlightElem, LinebreakElem, OverlineElem, SmartQuoteElem, StrikeElem, SubElem, SuperElem,
        UnderlineElem,
    };
    c.is::<TextElem>()
        || c.is::<SpaceElem>()
        || c.is::<LinebreakElem>()
        || c.is::<StrongElem>()
        || c.is::<EmphElem>()
        || c.is::<LinkElem>()
        || c.is::<SmartQuoteElem>()
        || c.is::<UnderlineElem>()
        || c.is::<OverlineElem>()
        || c.is::<StrikeElem>()
        || c.is::<HighlightElem>()
        || c.is::<SubElem>()
        || c.is::<SuperElem>()
        || is_inline_styled(c)
        || (c.is::<EquationElem>() && !is_display_equation(c))
}

fn is_inline_styled(c: &Content) -> bool {
    c.to_packed::<StyledElem>().is_some_and(|styled| {
        styled.child.to_packed::<SequenceElem>().map_or_else(
            || is_known_inline(&styled.child),
            |seq| is_inline_sequence(seq),
        )
    })
}

fn is_inline_sequence(seq: &SequenceElem) -> bool {
    seq.children.iter().all(is_known_inline)
}

fn apply_block_styles(block: Content, styles: &Styles) -> Content {
    if block.is::<ParbreakElem>() {
        block
    } else {
        block.styled_with_map(styles.clone())
    }
}

/// A single diffable token: either a word/space split from `TextElem`, or an atomic inline.
///
/// Equality and hashing are based on visible text plus presentation identity. This
/// lets Myers treat `regular` → `*regular*`, subscript → superscript, or paragraph
/// → heading as real token edits while still ignoring non-rendering metadata such as
/// link targets.
#[derive(Clone, Debug)]
pub struct Token {
    pub text: String,
    pub content: Content,
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && presentation_key(&self.content) == presentation_key(&other.content)
    }
}
impl Eq for Token {}
impl PartialOrd for Token {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Token {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.text
            .cmp(&other.text)
            .then_with(|| presentation_key(&self.content).cmp(&presentation_key(&other.content)))
    }
}
impl Hash for Token {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
        presentation_key(&self.content).hash(state);
    }
}

fn presentation_key(content: &Content) -> String {
    let mut out = String::new();
    write_presentation_key(content, &mut out);
    out
}

fn write_presentation_key(content: &Content, out: &mut String) {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        out.push_str("seq[");
        for child in &seq.children {
            if is_metadata_tag(child) {
                continue;
            }
            write_presentation_key(child, out);
            out.push(';');
        }
        out.push(']');
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        let styles_key = styles_key(&styled.styles);
        if styles_key.is_empty() {
            write_presentation_key(&styled.child, out);
            return;
        }
        out.push_str("styled(");
        out.push_str(&styles_key);
        out.push_str(")[");
        write_presentation_key(&styled.child, out);
        out.push(']');
    } else if let Some(par) = content.to_packed::<ParElem>() {
        out.push_str("par[");
        write_presentation_key(&par.body, out);
        out.push(']');
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        out.push_str("heading[");
        write_presentation_key(&heading.body, out);
        out.push(']');
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        out.push_str("block(");
        if block_has_visual_decoration(block) {
            out.push_str("visual:");
            out.push_str(content.repr().as_str());
            out.push(')');
            return;
        }
        out.push_str(match block.body.get_cloned(StyleChain::default()) {
            Some(BlockBody::Content(_)) => "content",
            Some(_) => "other",
            None => "auto",
        });
        out.push_str(")[");
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            write_presentation_key(&body, out);
        }
        out.push(']');
    } else if let Some(equation) = content.to_packed::<EquationElem>() {
        out.push_str("equation(");
        out.push_str(if equation.block.get(StyleChain::default()) {
            "block"
        } else {
            "inline"
        });
        out.push_str("):");
        out.push_str(equation.body.repr().as_str());
    } else if let Some(link) = content.to_packed::<LinkElem>() {
        write_presentation_key(&link.body, out);
    } else if let Some(strong) = content.to_packed::<StrongElem>() {
        out.push_str("strong[");
        write_presentation_key(&strong.body, out);
        out.push(']');
    } else if let Some(emph) = content.to_packed::<EmphElem>() {
        out.push_str("emph[");
        write_presentation_key(&emph.body, out);
        out.push(']');
    } else if let Some(highlight) = content.to_packed::<HighlightElem>() {
        out.push_str("highlight(");
        out.push_str(&format!("{highlight:?}"));
        out.push_str(")[");
        write_presentation_key(&highlight.body, out);
        out.push(']');
    } else if let Some(sub) = content.to_packed::<SubElem>() {
        out.push_str("sub[");
        write_presentation_key(&sub.body, out);
        out.push(']');
    } else if let Some(sup) = content.to_packed::<SuperElem>() {
        out.push_str("super[");
        write_presentation_key(&sup.body, out);
        out.push(']');
    } else if let Some(underline) = content.to_packed::<UnderlineElem>() {
        out.push_str("underline[");
        write_presentation_key(&underline.body, out);
        out.push(']');
    } else if let Some(overline) = content.to_packed::<OverlineElem>() {
        out.push_str("overline[");
        write_presentation_key(&overline.body, out);
        out.push(']');
    } else if let Some(strike) = content.to_packed::<StrikeElem>() {
        out.push_str("strike[");
        write_presentation_key(&strike.body, out);
        out.push(']');
    } else if content.is::<TextElem>() {
        out.push_str("text");
    } else if content.is::<SpaceElem>() {
        out.push_str("space");
    } else if is_opaque_visual_element_name(content.func().name()) {
        out.push_str(content.func().name());
        out.push(':');
        out.push_str(content.repr().as_str());
    } else if is_metadata_tag(content) {
    } else {
        let children = container_ops::semantic_diff_child_contents(content);
        if !children.is_empty() {
            out.push_str(content.func().name());
            out.push('[');
            for child in children {
                if is_metadata_tag(&child) {
                    continue;
                }
                write_presentation_key(&child, out);
                out.push(';');
            }
            out.push(']');
        } else {
            out.push_str(content.func().name());
            out.push(':');
            out.push_str(content.plain_text().as_str());
        }
    }
}

fn is_metadata_tag(content: &Content) -> bool {
    content.func().name() == "tag"
}

fn write_styles_key(styles: &Styles, out: &mut String) {
    out.push_str(&styles_key(styles));
}

fn styles_key(styles: &Styles) -> String {
    let mut out = String::new();
    for style in styles.iter() {
        if !is_presentation_style(style) {
            continue;
        }
        // `Style` equality can include realization provenance. The debug
        // signature is the same normalization used for inherited-style stripping.
        out.push_str(&format!("{style:?}"));
        out.push(';');
    }
    out
}

fn is_presentation_style(style: &Style) -> bool {
    style.property().is_some()
        && style.element().is_some_and(|element| {
            element == TextElem::ELEM
                || element == ParElem::ELEM
                || element == HeadingElem::ELEM
                || element == EquationElem::ELEM
                || element == RawElem::ELEM
        })
}

/// Walk a block's inline content and produce a flat list of [`Token`]s.
///
/// - `TextElem` / `SpaceElem` nodes are split on whitespace boundaries.
/// - `EquationElem` nodes become a single token whose text is the equation's `repr`.
///   When annotated equation origins are available, realized math carriers use
///   the source equation as their token content.
/// - Semantic containers recurse through `container_ops` children.
/// - Any other node becomes a single atomic token.
pub fn extract_words(content: &Content) -> Vec<Token> {
    let mut tokens = Vec::new();
    collect_tokens(content, &mut tokens);
    tokens
}

fn extract_words_for_annotated(
    fallback: &Content,
    annotated: Option<&AnnotatedContent>,
) -> Vec<Token> {
    let Some(annotated) = annotated else {
        return extract_words(fallback);
    };
    let mut tokens = Vec::new();
    collect_annotated_tokens(annotated, &mut tokens);
    if tokens.is_empty() {
        extract_words(fallback)
    } else {
        tokens
    }
}

fn extract_words_for_annotated_with_equation_origins(
    fallback: &Content,
    annotated: Option<&AnnotatedContent>,
    equation_origins: &[Content],
) -> Vec<Token> {
    let tokens = extract_words_for_annotated(fallback, annotated);
    if equation_origins.is_empty() {
        return tokens;
    }

    let mut origin_iter = equation_origins.iter();
    let mut origin_tokens = Vec::new();
    collect_tokens_with_equation_origins(fallback, &mut origin_iter, &mut origin_tokens);
    if has_meaningful_tokens(&origin_tokens) {
        origin_tokens
    } else {
        tokens
    }
}

fn has_equation_origins(node: &AnnotatedContent) -> bool {
    !node.annotation.equation_origins.is_empty() || node.children.iter().any(has_equation_origins)
}

fn collect_annotated_tokens(node: &AnnotatedContent, out: &mut Vec<Token>) {
    if !node.annotation.equation_origins.is_empty() {
        let before = out.len();
        let mut origins = node.annotation.equation_origins.iter();
        collect_tokens_with_equation_origins(&node.realized, &mut origins, out);
        if !has_meaningful_tokens(&out[before..]) {
            out.truncate(before);
            for origin in &node.annotation.equation_origins {
                collect_tokens(origin, out);
            }
        }
        return;
    }

    if all_slots_are_footnote_bodies(node) {
        collect_tokens(&node.realized, out);
        return;
    }

    let slots = resolved_slots(node);
    if !slots.is_empty() {
        for (_slot, child) in slots {
            collect_annotated_tokens(child, out);
        }
        return;
    }

    if node.children.iter().any(has_annotated_token_metadata) {
        for child in &node.children {
            collect_annotated_tokens(child, out);
        }
        return;
    }

    collect_tokens(&node.realized, out);
}

fn has_annotated_token_metadata(node: &AnnotatedContent) -> bool {
    has_equation_origins(node) || node.children.iter().any(has_annotated_token_metadata)
}

fn collect_tokens(content: &Content, out: &mut Vec<Token>) {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        for child in &seq.children {
            collect_tokens(child, out);
        }
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        let before = out.len();
        collect_tokens(&styled.child, out);
        for token in &mut out[before..] {
            token.content = token.content.clone().styled_with_map(styled.styles.clone());
        }
    } else if let Some(emph) = content.to_packed::<EmphElem>() {
        let before = out.len();
        collect_tokens(&emph.body, out);
        for token in &mut out[before..] {
            token.content = token.content.clone().emph();
        }
    } else if let Some(par) = content.to_packed::<ParElem>() {
        let before = out.len();
        collect_tokens(&par.body, out);
        wrap_tokens_with_context(content, before, out);
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        let before = out.len();
        collect_tokens(&heading.body, out);
        wrap_tokens_with_context(content, before, out);
    } else if let Some(caption) = content.to_packed::<FigureCaption>() {
        collect_tokens(&caption.body, out);
    } else if let Some(link) = content.to_packed::<LinkElem>() {
        collect_tokens(&link.body, out);
    } else if let Some(equation) = content.to_packed::<EquationElem>() {
        out.push(Token {
            text: equation.body.repr().to_string(),
            content: content.clone(),
        });
    } else if collect_semantic_child_tokens(content, out) {
    } else if let Some(text_elem) = content.to_packed::<TextElem>() {
        collect_text_tokens(text_elem.text.as_str(), out);
    } else if content.is::<SpaceElem>() {
        out.push(Token {
            text: " ".to_string(),
            content: content.clone(),
        });
    } else if is_metadata_tag(content) {
    } else {
        out.push(Token {
            text: content.plain_text().to_string(),
            content: content.clone(),
        });
    }
}

fn collect_tokens_with_equation_origins<'a>(
    content: &Content,
    origins: &mut impl Iterator<Item = &'a Content>,
    out: &mut Vec<Token>,
) {
    if is_realized_equation_carrier(content) {
        if let Some(origin) = origins.next() {
            collect_tokens(origin, out);
        } else {
            collect_tokens(content, out);
        }
    } else if let Some(seq) = content.to_packed::<SequenceElem>() {
        for child in &seq.children {
            collect_tokens_with_equation_origins(child, origins, out);
        }
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        let before = out.len();
        collect_tokens_with_equation_origins(&styled.child, origins, out);
        for token in &mut out[before..] {
            token.content = token.content.clone().styled_with_map(styled.styles.clone());
        }
    } else if let Some(par) = content.to_packed::<ParElem>() {
        let before = out.len();
        collect_tokens_with_equation_origins(&par.body, origins, out);
        wrap_tokens_with_context(content, before, out);
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        let before = out.len();
        collect_tokens_with_equation_origins(&heading.body, origins, out);
        wrap_tokens_with_context(content, before, out);
    } else if let Some(caption) = content.to_packed::<FigureCaption>() {
        collect_tokens_with_equation_origins(&caption.body, origins, out);
    } else if let Some(link) = content.to_packed::<LinkElem>() {
        collect_tokens_with_equation_origins(&link.body, origins, out);
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            let before = out.len();
            collect_tokens_with_equation_origins(&body, origins, out);
            wrap_tokens_with_context(content, before, out);
        } else {
            collect_tokens(content, out);
        }
    } else {
        collect_tokens(content, out);
    }
}

fn is_realized_equation_carrier(content: &Content) -> bool {
    content.is::<EquationElem>() || matches!(content.func().name(), "inline" | "display")
}

fn collect_semantic_child_tokens(content: &Content, out: &mut Vec<Token>) -> bool {
    let children = container_ops::semantic_diff_child_contents(content);
    if children.is_empty() {
        return false;
    }
    for (index, child) in children.into_iter().enumerate() {
        let before = out.len();
        collect_tokens(&child, out);
        if content.is::<BlockElem>() {
            wrap_tokens_with_context(content, before, out);
        } else if content.is::<BoxElem>() {
            for token in &mut out[before..] {
                if let Some(wrapped) =
                    container_ops::replace_realized_child(content, index, token.content.clone())
                {
                    token.content = wrapped;
                }
            }
        }
    }
    true
}

fn wrap_tokens_with_context(context: &Content, before: usize, out: &mut [Token]) {
    for token in &mut out[before..] {
        if let Some(wrapped) = token_content_in_context(context, token.content.clone()) {
            token.content = wrapped;
        }
    }
}

fn token_content_in_context(context: &Content, replacement: Content) -> Option<Content> {
    let mut result = context.clone();

    if let Some(par) = result.to_packed_mut::<ParElem>() {
        par.body = replacement;
        return Some(result);
    }

    if let Some(heading) = result.to_packed_mut::<HeadingElem>() {
        heading.body = replacement;
        return Some(result);
    }

    if let Some(block) = result.to_packed_mut::<BlockElem>() {
        block.body.set(Some(BlockBody::Content(replacement)));
        return Some(result);
    }

    None
}

fn token_content_for_direct_edit(token: &Token) -> Content {
    inline_token_content_for_diff(&token.content, token.text.as_str())
}

fn inline_token_content_for_diff(content: &Content, text: &str) -> Content {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let child = inline_token_content_for_diff(&styled.child, text);
        return child.styled_with_map(styled.styles.clone());
    }

    if let Some(par) = content.to_packed::<ParElem>() {
        return inline_token_content_for_diff(&par.body, text);
    }

    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return inline_token_content_for_diff(&heading.body, text);
    }

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return inline_token_content_for_diff(&caption.body, text);
    }

    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return inline_token_content_for_diff(&body, text);
    }

    if content.plain_text().is_empty() && !text.is_empty() {
        TextElem::packed(text)
    } else {
        content.clone()
    }
}

fn collect_text_tokens(s: &str, out: &mut Vec<Token>) {
    let mut start = 0;
    let mut kind = s.chars().next().map(text_token_kind);
    for (i, ch) in s.char_indices() {
        let next_kind = text_token_kind(ch);
        if Some(next_kind) != kind {
            let slice = &s[start..i];
            if !slice.is_empty() {
                out.push(Token {
                    text: slice.to_string(),
                    content: TextElem::packed(slice),
                });
            }
            start = i;
            kind = Some(next_kind);
        }
    }
    let tail = &s[start..];
    if !tail.is_empty() {
        out.push(Token {
            text: tail.to_string(),
            content: TextElem::packed(tail),
        });
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TextTokenKind {
    Space,
    Punctuation,
    Word,
}

fn text_token_kind(ch: char) -> TextTokenKind {
    if ch.is_whitespace() {
        TextTokenKind::Space
    } else if ch.is_ascii_punctuation() {
        TextTokenKind::Punctuation
    } else {
        TextTokenKind::Word
    }
}

/// Newtype that adds `Eq + Ord` to `Content` so it can be used with `similar`.
///
/// `Content` only implements `PartialEq` and `Hash`; `similar::capture_diff_slices`
/// requires full `Eq + Ord`. Ordering is by plain-text first, then by hash as a
/// tiebreaker — this satisfies the `Ord`/`Eq` consistency contract because two nodes
/// with the same hash (structurally equal) will always compare `Equal`.
#[derive(Clone)]
struct HashableContent(Content);
impl PartialEq for HashableContent {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for HashableContent {}
impl PartialOrd for HashableContent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HashableContent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Primary: plain_text for semantic grouping. Secondary: hash as tiebreaker
        // so that structurally equal Content (same hash) always compares Equal,
        // satisfying the Ord/Eq consistency contract.
        let text_cmp = self.0.plain_text().cmp(&other.0.plain_text());
        if text_cmp != std::cmp::Ordering::Equal {
            return text_cmp;
        }
        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        use std::hash::Hasher as _;
        self.0.hash(&mut h1);
        other.0.hash(&mut h2);
        h1.finish().cmp(&h2.finish())
    }
}
impl Hash for HashableContent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

/// Block-level diff operation produced by [`diff_block_units_raw`] and [`match_edit_zones`].
///
/// `Equal` and `Replace` carry both the old and new block so the caller can
/// choose which version to render. `diff_content` always renders the *new* version.
#[derive(Clone)]
pub enum BlockOp {
    Equal(DiffBlock, DiffBlock),
    Delete(DiffBlock),
    Insert(DiffBlock),
    /// A matched delete/insert pair whose plain-text similarity is ≥ 0.3.
    Replace(DiffBlock, DiffBlock),
}

/// Diff two flat block slices with Myers LCS, returning `Equal / Delete / Insert` ops.
///
/// This is the public entry point for tests. Production code calls the internal
/// `diff_block_units_raw` which accepts [`DiffBlock`] slices (with page styles).
pub fn diff_blocks_raw(old: &[Content], new: &[Content]) -> Vec<BlockOp> {
    let old: Vec<DiffBlock> = old
        .iter()
        .cloned()
        .map(|content| DiffBlock {
            content,
            page_styles: Styles::new(),
        })
        .collect();
    let new: Vec<DiffBlock> = new
        .iter()
        .cloned()
        .map(|content| DiffBlock {
            content,
            page_styles: Styles::new(),
        })
        .collect();
    diff_block_units_raw(&old, &new)
}

fn diff_block_units_raw(old: &[DiffBlock], new: &[DiffBlock]) -> Vec<BlockOp> {
    let old_h: Vec<HashableContent> = old
        .iter()
        .map(|block| HashableContent(block.content.clone()))
        .collect();
    let new_h: Vec<HashableContent> = new
        .iter()
        .map(|block| HashableContent(block.content.clone()))
        .collect();
    let ops = capture_diff_slices(Algorithm::Myers, &old_h, &new_h);
    let mut result = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    result.push(BlockOp::Equal(
                        old[old_index + i].clone(),
                        new[new_index + i].clone(),
                    ));
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    result.push(BlockOp::Delete(old[old_index + i].clone()));
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    result.push(BlockOp::Insert(new[new_index + i].clone()));
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                for i in 0..old_len {
                    result.push(BlockOp::Delete(old[old_index + i].clone()));
                }
                for i in 0..new_len {
                    result.push(BlockOp::Insert(new[new_index + i].clone()));
                }
            }
        }
    }
    result
}

fn block_op_trace_event(
    stage: &'static str,
    event: &'static str,
    index: usize,
    op: &BlockOp,
) -> PipelineTraceEvent {
    let base = PipelineTraceEvent::new(stage, event).reason(format!("op_index={index}"));
    match op {
        BlockOp::Equal(old, new) => base
            .old_content(&old.content)
            .new_content(&new.content)
            .selected_edit_kind("equal"),
        BlockOp::Delete(old) => base.old_content(&old.content).selected_edit_kind("delete"),
        BlockOp::Insert(new) => base.new_content(&new.content).selected_edit_kind("insert"),
        BlockOp::Replace(old, new) => base
            .old_content(&old.content)
            .new_content(&new.content)
            .selected_edit_kind("replace"),
    }
}

/// Scan the raw ops for contiguous `Delete + Insert` zones and pair them by similarity.
///
/// Within each zone every delete is greedily matched to the most-similar unused insert
/// (similarity threshold 0.3). Pairs become [`BlockOp::Replace`]; unmatched deletes and
/// inserts are emitted as-is. Paired inserts are emitted after all deletes (in their
/// original order) to keep the output sequence stable.
pub fn match_edit_zones(ops: Vec<BlockOp>) -> Vec<BlockOp> {
    let mut no_debug_events = None;
    match_edit_zones_inner(ops, &mut no_debug_events).expect("matching without trace cannot fail")
}

pub fn match_edit_zones_with_debug_events(
    ops: Vec<BlockOp>,
    debug_events: &mut dyn DebugEventSink,
) -> anyhow::Result<Vec<BlockOp>> {
    let mut debug_events = Some(debug_events);
    match_edit_zones_inner(ops, &mut debug_events)
}

fn match_edit_zones_inner(
    ops: Vec<BlockOp>,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<Vec<BlockOp>> {
    let mut result: Vec<BlockOp> = Vec::new();
    let mut i = 0;
    let n = ops.len();

    while i < n {
        match &ops[i] {
            BlockOp::Equal(_, _) | BlockOp::Replace(_, _) => {
                result.push(ops[i].clone());
                i += 1;
            }
            BlockOp::Delete(_) | BlockOp::Insert(_) => {
                // Collect the entire contiguous Delete/Insert zone regardless of ordering.
                let start = i;
                while i < n && matches!(&ops[i], BlockOp::Delete(_) | BlockOp::Insert(_)) {
                    i += 1;
                }
                let deletes: Vec<DiffBlock> = ops[start..i]
                    .iter()
                    .filter_map(|op| match op {
                        BlockOp::Delete(c) => Some(c.clone()),
                        _ => None,
                    })
                    .collect();
                let inserts: Vec<DiffBlock> = ops[start..i]
                    .iter()
                    .filter_map(|op| match op {
                        BlockOp::Insert(c) => Some(c.clone()),
                        _ => None,
                    })
                    .collect();
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "zone").reason(format!(
                        "deletes={} inserts={}",
                        deletes.len(),
                        inserts.len()
                    )),
                )?;
                pair_edit_zone(deletes, inserts, &mut result, debug_events)?;
            }
        }
    }
    Ok(result)
}

fn pair_edit_zone(
    deletes: Vec<DiffBlock>,
    inserts: Vec<DiffBlock>,
    out: &mut Vec<BlockOp>,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<()> {
    if deletes.is_empty() {
        if debug_events.is_some() {
            for (index, insert) in inserts.iter().enumerate() {
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "unmatched_insert")
                        .new_block_index(index)
                        .new_content(&insert.content)
                        .selected_edit_kind("insert"),
                )?;
            }
        }
        out.extend(inserts.into_iter().map(BlockOp::Insert));
        return Ok(());
    }
    if inserts.is_empty() {
        if debug_events.is_some() {
            for (index, delete) in deletes.iter().enumerate() {
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "unmatched_delete")
                        .old_block_index(index)
                        .old_content(&delete.content)
                        .selected_edit_kind("delete"),
                )?;
            }
        }
        out.extend(deletes.into_iter().map(BlockOp::Delete));
        return Ok(());
    }

    // Match each delete to its best insert (greedy, in delete order).
    let mut used_inserts = vec![false; inserts.len()];
    // paired_insert_idx[i] = Some(j) if deletes[i] is paired with inserts[j]
    let mut paired_insert_idx: Vec<Option<usize>> = Vec::with_capacity(deletes.len());

    for del in &deletes {
        let del_text = del.content.plain_text();
        let best = inserts
            .iter()
            .enumerate()
            .filter(|(j, _)| !used_inserts[*j])
            .map(|(j, ins)| {
                (
                    j,
                    similarity(del_text.as_str(), ins.content.plain_text().as_str()),
                )
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        if debug_events.is_some() {
            for (j, ins) in inserts
                .iter()
                .enumerate()
                .filter(|(j, _)| !used_inserts[*j])
            {
                let sim = similarity(del_text.as_str(), ins.content.plain_text().as_str());
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "similarity_candidate")
                        .old_block_index(paired_insert_idx.len())
                        .new_block_index(j)
                        .old_content(&del.content)
                        .new_content(&ins.content)
                        .similarity(sim)
                        .threshold(0.3),
                )?;
            }
        }

        match best {
            Some((j, sim)) if sim >= 0.3 => {
                used_inserts[j] = true;
                paired_insert_idx.push(Some(j));
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "selected_replacement")
                        .old_block_index(paired_insert_idx.len() - 1)
                        .new_block_index(j)
                        .similarity(sim)
                        .threshold(0.3)
                        .selected_edit_kind("replace"),
                )?;
            }
            Some((j, sim)) => {
                paired_insert_idx.push(None);
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "rejected_candidate")
                        .old_block_index(paired_insert_idx.len() - 1)
                        .new_block_index(j)
                        .similarity(sim)
                        .threshold(0.3)
                        .selected_edit_kind("delete"),
                )?;
            }
            None => {
                paired_insert_idx.push(None);
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/edit-zone", "no_candidate")
                        .old_block_index(paired_insert_idx.len() - 1)
                        .selected_edit_kind("delete"),
                )?;
            }
        }
    }

    // Emit deletes (as Delete or Replace) in their original order.
    for (i, del) in deletes.into_iter().enumerate() {
        match paired_insert_idx[i] {
            Some(j) => out.push(BlockOp::Replace(del, inserts[j].clone())),
            None => out.push(BlockOp::Delete(del)),
        }
    }

    // Emit unpaired inserts after all deletes (in original insert order).
    for (j, ins) in inserts.into_iter().enumerate() {
        if !used_inserts[j] {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/edit-zone", "unmatched_insert")
                    .new_block_index(j)
                    .new_content(&ins.content)
                    .selected_edit_kind("insert"),
            )?;
            out.push(BlockOp::Insert(ins));
        }
    }
    Ok(())
}

/// Compute a [0, 1] similarity score between two plain-text strings.
///
/// For short strings (≤ 2 000 chars) uses normalized Levenshtein distance.
/// For longer strings falls back to Sørensen–Dice word overlap, which is O(n)
/// rather than O(n²) and avoids timeout on large blocks.
fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    if max_len > 2_000 {
        return word_overlap_similarity(a, b);
    }
    let min_similarity = 0.3;
    let max_distance = ((1.0 - min_similarity) * max_len as f64).floor() as usize;
    let distance = match edit_distance_with_limit(a, b, max_distance) {
        Some(distance) => distance,
        None => return 0.0,
    };
    1.0 - distance as f64 / max_len as f64
}

fn word_overlap_similarity(a: &str, b: &str) -> f64 {
    let mut a_counts: HashMap<&str, usize> = HashMap::new();
    let mut a_len = 0usize;
    for word in a.split_whitespace() {
        *a_counts.entry(word).or_default() += 1;
        a_len += 1;
    }

    let mut b_len = 0usize;
    let mut overlap = 0usize;
    for word in b.split_whitespace() {
        b_len += 1;
        if let Some(count) = a_counts.get_mut(word)
            && *count > 0
        {
            *count -= 1;
            overlap += 1;
        }
    }

    if a_len == 0 && b_len == 0 {
        1.0
    } else if a_len == 0 || b_len == 0 {
        0.0
    } else {
        2.0 * overlap as f64 / (a_len + b_len) as f64
    }
}

/// Levenshtein distance between `a` and `b`, returning `None` if the distance
/// exceeds `max_distance`.
///
/// The early-exit lets the caller quickly discard pairs that are too dissimilar
/// without paying the full O(n²) DP cost.
fn edit_distance_with_limit(a: &str, b: &str, max_distance: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());

    if m.abs_diff(n) > max_distance {
        return None;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        let mut row_min = curr[0];

        for j in 1..=n {
            curr[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1]
            } else {
                1 + prev[j].min(curr[j - 1]).min(prev[j - 1])
            };
            row_min = row_min.min(curr[j]);
        }

        if row_min > max_distance {
            return None;
        }

        std::mem::swap(&mut prev, &mut curr);
    }

    (prev[n] <= max_distance).then_some(prev[n])
}

/// A word-level diff operation over [`Token`] sequences.
#[derive(Clone, Debug, PartialEq)]
pub enum WordOp {
    Equal(Vec<Token>),
    Delete(Vec<Token>),
    Insert(Vec<Token>),
}

fn has_textual_word_change(word_ops: &[WordOp]) -> bool {
    word_ops.iter().any(|op| match op {
        WordOp::Delete(tokens) | WordOp::Insert(tokens) => tokens
            .iter()
            .any(|token| token.text.chars().any(|ch| !ch.is_whitespace())),
        WordOp::Equal(_) => false,
    })
}

fn has_meaningful_tokens(tokens: &[Token]) -> bool {
    tokens
        .iter()
        .any(|token| token.text.chars().any(|ch| !ch.is_whitespace()))
}

/// Diff two [`Token`] sequences with Myers LCS, coalescing adjacent same-kind ops.
///
/// Adjacent `Delete Delete` or `Insert Insert` chunks from `similar` are merged into
/// a single op so that annotate can treat them as one run (important for separator
/// insertion between delete and insert runs).
pub fn diff_words(old: &[Token], new: &[Token]) -> Vec<WordOp> {
    let raw_ops = capture_diff_slices(Algorithm::Myers, old, new);
    let mut result: Vec<WordOp> = Vec::new();

    for op in raw_ops {
        match op {
            DiffOp::Equal { new_index, len, .. } => {
                coalesce(
                    &mut result,
                    WordOp::Equal(new[new_index..new_index + len].to_vec()),
                );
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                coalesce(
                    &mut result,
                    WordOp::Delete(old[old_index..old_index + old_len].to_vec()),
                );
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                coalesce(
                    &mut result,
                    WordOp::Insert(new[new_index..new_index + new_len].to_vec()),
                );
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                coalesce(
                    &mut result,
                    WordOp::Delete(old[old_index..old_index + old_len].to_vec()),
                );
                coalesce(
                    &mut result,
                    WordOp::Insert(new[new_index..new_index + new_len].to_vec()),
                );
            }
        }
    }
    merge_substitution_zones(result)
}

/// True if `op` is an `Equal` whose tokens are all whitespace characters.
fn is_whitespace_only_equal(op: &WordOp) -> bool {
    match op {
        WordOp::Equal(tokens) => tokens
            .iter()
            .all(|t| t.text.chars().all(|c| c.is_whitespace())),
        _ => false,
    }
}

/// Absorb whitespace-only `Equal` ops into adjacent `Delete`/`Insert` runs.
///
/// A *zone* is a maximal contiguous run of ops that are `Delete`, `Insert`, or
/// whitespace-only `Equal`. Within each zone, whitespace-only `Equal` tokens are
/// distributed into both the preceding Delete side and the Insert side, so that
/// the final output is at most one `Delete` followed by at most one `Insert`
/// (with spaces embedded). Trailing whitespace in a zone is dropped.
///
/// Non-whitespace `Equal` ops are never touched.
fn merge_substitution_zones(ops: Vec<WordOp>) -> Vec<WordOp> {
    let mut result: Vec<WordOp> = Vec::new();
    let mut i = 0;
    let n = ops.len();

    while i < n {
        if matches!(&ops[i], WordOp::Delete(_) | WordOp::Insert(_)) {
            // Extend the zone as far as Delete / Insert / whitespace-Equal ops reach.
            let zone_start = i;
            while i < n
                && (matches!(&ops[i], WordOp::Delete(_) | WordOp::Insert(_))
                    || is_whitespace_only_equal(&ops[i]))
            {
                i += 1;
            }
            // Trim trailing whitespace-only Equals (they'd only add dangling space).
            while i > zone_start && is_whitespace_only_equal(&ops[i - 1]) {
                i -= 1;
            }
            result.extend(merge_zone(&ops[zone_start..i]));
        } else {
            result.push(ops[i].clone());
            i += 1;
        }
    }

    result
}

/// Merge the ops of a single substitution zone into at most one Delete + one Insert.
fn merge_zone(zone: &[WordOp]) -> Vec<WordOp> {
    let mut delete_tokens: Vec<Token> = Vec::new();
    let mut insert_tokens: Vec<Token> = Vec::new();
    // Whitespace pending to be prepended before the next Delete or Insert on each side.
    let mut pending_del: Vec<Token> = Vec::new();
    let mut pending_ins: Vec<Token> = Vec::new();

    for op in zone {
        match op {
            WordOp::Delete(tokens) => {
                delete_tokens.append(&mut pending_del);
                delete_tokens.extend_from_slice(tokens);
            }
            WordOp::Insert(tokens) => {
                insert_tokens.append(&mut pending_ins);
                insert_tokens.extend_from_slice(tokens);
            }
            WordOp::Equal(tokens) => {
                // Whitespace-only equal: stage a copy for each side.
                pending_del.extend_from_slice(tokens);
                pending_ins.extend_from_slice(tokens);
            }
        }
    }

    let mut result = Vec::new();
    if !delete_tokens.is_empty() {
        result.push(WordOp::Delete(delete_tokens));
    }
    if !insert_tokens.is_empty() {
        result.push(WordOp::Insert(insert_tokens));
    }
    result
}

fn coalesce(ops: &mut Vec<WordOp>, next: WordOp) {
    match (ops.last_mut(), &next) {
        (Some(WordOp::Equal(v)), WordOp::Equal(w)) => v.extend_from_slice(w),
        (Some(WordOp::Delete(v)), WordOp::Delete(w)) => v.extend_from_slice(w),
        (Some(WordOp::Insert(v)), WordOp::Insert(w)) => v.extend_from_slice(w),
        _ => ops.push(next),
    }
}

fn old_display_tokens(tokens: Vec<Token>) -> Vec<Token> {
    tokens
        .into_iter()
        .map(|token| Token {
            text: token.text,
            content: retain_old_display_content(&token.content),
        })
        .collect()
}

fn old_display_delete_ops(ops: Vec<WordOp>) -> Vec<WordOp> {
    ops.into_iter()
        .map(|op| match op {
            WordOp::Delete(tokens) => WordOp::Delete(old_display_tokens(tokens)),
            WordOp::Equal(tokens) => WordOp::Equal(tokens),
            WordOp::Insert(tokens) => WordOp::Insert(tokens),
        })
        .collect()
}

fn push_log_entry(log: &mut String, index: usize, kind: &str, fields: &[(&str, String)]) {
    log.push_str(&format!("## {index}: {kind}\n"));
    for (name, value) in fields {
        log.push_str(name);
        log.push_str(": ");
        log.push_str(&single_line(value));
        log.push('\n');
    }
    log.push('\n');
}

fn collect_word_op_text(word_ops: &[WordOp], select: fn(&WordOp) -> Option<&Vec<Token>>) -> String {
    word_ops
        .iter()
        .filter_map(select)
        .map(|tokens| {
            tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn content_log_text(content: &Content) -> String {
    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return content_log_text(&caption.body);
    }

    let plain = content.plain_text();
    if plain.chars().any(|ch| !ch.is_whitespace()) {
        return plain.to_string();
    }

    extract_words(content)
        .into_iter()
        .filter(|token| token.text.chars().any(|ch| !ch.is_whitespace()))
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join("")
}

fn single_line(text: &str) -> String {
    let mut result = String::new();
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(ch);
            last_was_space = false;
        }
        if result.len() >= 1_000 {
            result.push_str("...");
            break;
        }
    }
    result.trim().to_string()
}

fn content_signature(content: &Content) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn root_page_styles_raw(content: &Content) -> Styles {
    if let Some(styled) = content.to_packed::<StyledElem>()
        && styled.child.to_packed::<SequenceElem>().is_some()
    {
        return page_styles_raw(&styled.styles);
    }

    let Some(seq) = content.to_packed::<SequenceElem>() else {
        return Styles::new();
    };

    seq.children
        .iter()
        .filter_map(|child| {
            let styled = child.to_packed::<StyledElem>()?;
            styled
                .child
                .to_packed::<SequenceElem>()
                .is_some()
                .then(|| page_styles_raw(&styled.styles))
        })
        .find(|styles| !styles.is_empty())
        .unwrap_or_default()
}

fn page_styles_raw(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect()
}

fn page_styles(styles: &Styles) -> Styles {
    sanitize_page_styles(page_styles_raw(styles))
}

fn sanitize_page_styles(mut styles: Styles) -> Styles {
    if !styles.is_empty() {
        sanitize_page_marginals(&mut styles);
    }
    styles
}

fn sanitize_page_marginals(styles: &mut Styles) {
    let chain = StyleChain::new(styles);
    let header = sanitize_marginal(chain.get_cloned(PageElem::header));
    let footer = sanitize_marginal(chain.get_cloned(PageElem::footer));

    if header.is_custom() {
        styles.push(PageElem::header.set(header));
    }
    if footer.is_custom() {
        styles.push(PageElem::footer.set(footer));
    }
}

fn sanitize_marginal(marginal: Smart<Option<Content>>) -> Smart<Option<Content>> {
    marginal.map(|content| {
        content.map(|content| {
            Content::new(
                BlockElem::new()
                    .with_width(Smart::Custom(Rel::one()))
                    .with_body(Some(BlockBody::Content(
                        content.styled(ParElem::justify.set(false)),
                    ))),
            )
        })
    })
}

fn page_styles_from(styles: &Styles) -> Styles {
    page_styles(styles)
}

fn non_page_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| !is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect()
}

fn is_page_style(style: &Style) -> bool {
    style
        .element()
        .is_some_and(|element| element == PageElem::ELEM)
}

// ──────────────────────────────────────────────────────────────────────────────
// Realized edit diff types
// ──────────────────────────────────────────────────────────────────────────────

/// Diff result for the annotated realized-tree pipeline.
pub struct DiffResult {
    pub blocks: Vec<DiffBlockEdit>,
    pub root_styles: Styles,
    pub regions: Vec<DiffRegionEdit>,
    pub rendered_regions: Vec<RenderedRegionEdit>,
}

pub struct DiffBlockEdit {
    /// New-side realized block for normal edits; old-side block for pure deletes.
    pub base: AnnotatedContent,
    /// Provenance of `base.realized`; annotation must not treat old bases as live
    /// Typst content.
    pub base_provenance: BlockBaseProvenance,
    /// Edits to apply to `base.realized`.
    pub edits: Vec<RealizedEdit>,
    /// Page styles active at this block's position (for output grouping).
    pub page_styles: Styles,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockBaseProvenance {
    LiveNew,
    InertOld,
    MixedInertOldLiveNew,
    Layout,
}

pub struct DiffBlockDebug {
    pub old_blocks: Vec<DiffBlock>,
    pub new_blocks: Vec<DiffBlock>,
    pub raw_ops: Vec<BlockOp>,
    pub matched_ops: Vec<BlockOp>,
}

struct PreparedDiffInputs {
    new_layout_blocks: Vec<DiffBlock>,
    old_realized_blocks: Vec<DiffBlock>,
    new_realized_blocks: Vec<DiffBlock>,
    matched_ops: Vec<BlockOp>,
    debug: Option<DiffBlockDebug>,
}

pub struct DiffRegionEdit {
    pub path: RegionPath,
    pub base: AnnotatedContent,
    pub edits: Vec<RealizedEdit>,
}

pub struct RenderedRegionEdit {
    pub kind: PageRegionKind,
    pub wrapper: RenderedRegionWrapper,
    pub pages: Vec<RenderedRegionPageEdit>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderedRegionWrapper {
    #[default]
    None,
    Align(RenderedRegionAlignment),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderedRegionAlignment {
    Left,
    Center,
    Right,
    Start,
    End,
}

pub struct RenderedRegionPageEdit {
    pub page: usize,
    pub base: Content,
    pub word_ops: Vec<WordOp>,
    pub segments: Vec<RenderedRegionSegmentEdit>,
    pub changed: bool,
}

pub struct RenderedRegionSegmentEdit {
    pub base: Content,
    pub word_ops: Vec<WordOp>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OldDisplaySurface {
    pub content: Content,
}

impl OldDisplaySurface {
    pub fn new(content: Content) -> Self {
        Self { content }
    }

    pub fn as_content(&self) -> &Content {
        &self.content
    }

    pub fn plain_text(&self) -> typst::diag::EcoString {
        self.content.plain_text()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionPath {
    RootPage(PageRegionKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageRegionKind {
    Header,
    Footer,
    Background,
    Foreground,
}

pub enum RealizedEdit {
    ReplaceAt {
        path: Vec<usize>,
        content: EditContent,
    },
    InsertBefore {
        anchor: Vec<usize>,
        content: EditContent,
    },
    InsertAfter {
        anchor: Vec<usize>,
        content: EditContent,
    },
    Append {
        content: EditContent,
    },
    WholeBlock(EditContent),
    LogOnly(EditContent),
    MarkBaseInserted(EditContent),
}

pub enum EditContent {
    Inserted(Content),
    Deleted(OldDisplaySurface),
    OpaqueReplacement {
        old: OldDisplaySurface,
        new: Content,
    },
    Modified {
        base: Content,
        word_ops: Vec<WordOp>,
    },
    Nested {
        base: AnnotatedContent,
        edits: Vec<RealizedEdit>,
    },
}

fn old_display_surface(content: &Content) -> OldDisplaySurface {
    OldDisplaySurface::new(retain_old_display_content(content))
}

fn old_display_surface_for_annotated(
    fallback: &Content,
    annotated: Option<&AnnotatedContent>,
) -> OldDisplaySurface {
    let content = annotated
        .map(effective_render_content)
        .unwrap_or_else(|| fallback.clone());
    old_display_surface(&content)
}

fn retain_old_display_content(content: &Content) -> Content {
    if content.is::<StateUpdateElem>()
        || content.is::<MetadataElem>()
        || content.is::<TagElem>()
        || content.is::<PagebreakElem>()
    {
        return Content::sequence([]);
    }

    if content.is::<ContextElem>() || content.is::<RefElem>() {
        let text = content.plain_text();
        return if text.is_empty() {
            Content::sequence([])
        } else {
            TextElem::packed(text.as_str())
        };
    }

    if content.is::<TextElem>()
        || content.is::<SpaceElem>()
        || content.is::<LinebreakElem>()
        || content.is::<ParbreakElem>()
        || content.is::<RawLine>()
        || content.is::<EquationElem>()
    {
        return strip_old_display_styles(content);
    }

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return Content::sequence(seq.children.iter().map(retain_old_display_content));
    }

    if let Some(styled) = content.to_packed::<StyledElem>() {
        let child = retain_old_display_content(&styled.child);
        let styles = old_display_styles(&styled.styles);
        return if styles.is_empty() {
            child
        } else {
            child.styled_with_map(styles)
        };
    }

    if let Some(strong) = content.to_packed::<StrongElem>() {
        return retain_old_display_content(&strong.body).strong();
    }

    if let Some(emph) = content.to_packed::<EmphElem>() {
        return retain_old_display_content(&emph.body).emph();
    }

    if let Some(highlight) = content.to_packed::<HighlightElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<HighlightElem>()
            .expect("checked highlight")
            .body = retain_old_display_content(&highlight.body);
        return kept;
    }

    if let Some(link) = content.to_packed::<LinkElem>() {
        return retain_old_display_content(&link.body);
    }

    if let Some(par) = content.to_packed::<ParElem>() {
        return Content::new(ParElem::new(retain_old_display_content(&par.body)));
    }

    if let Some(heading) = content.to_packed::<HeadingElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<HeadingElem>()
            .expect("checked heading")
            .body = retain_old_display_content(&heading.body);
        return kept;
    }

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<FigureCaption>()
            .expect("checked caption")
            .body = retain_old_display_content(&caption.body);
        return kept;
    }

    if let Some(list) = content.to_packed::<ListElem>() {
        let mut kept = content.clone();
        let kept_list = kept.to_packed_mut::<ListElem>().expect("checked list");
        for (kept_item, original_item) in kept_list.children.iter_mut().zip(&list.children) {
            kept_item.body = retain_old_display_content(&original_item.body);
        }
        return kept;
    }

    if let Some(enm) = content.to_packed::<EnumElem>() {
        let mut kept = content.clone();
        let kept_enum = kept.to_packed_mut::<EnumElem>().expect("checked enum");
        for (kept_item, original_item) in kept_enum.children.iter_mut().zip(&enm.children) {
            kept_item.body = retain_old_display_content(&original_item.body);
        }
        return kept;
    }

    if let Some(figure) = content.to_packed::<FigureElem>() {
        let mut kept = content.clone();
        let kept_figure = kept.to_packed_mut::<FigureElem>().expect("checked figure");
        kept_figure.body = retain_old_display_content(&figure.body);
        if let Some(original_caption) = figure.caption.get_cloned(StyleChain::default())
            && let Some(Some(kept_caption)) = kept_figure.caption.as_option_mut().as_mut()
        {
            kept_caption.body = retain_old_display_content(&original_caption.body);
        }
        return kept;
    }

    if let Some(footnote) = content.to_packed::<FootnoteElem>() {
        if let FootnoteBody::Content(body) = &footnote.body {
            let mut kept = content.clone();
            kept.to_packed_mut::<FootnoteElem>()
                .expect("checked footnote")
                .body = FootnoteBody::Content(retain_old_display_content(body));
            return kept;
        }
    }

    if content.is::<TableElem>() {
        let mut kept = content.clone();
        let table = kept.to_packed_mut::<TableElem>().expect("checked table");
        for child in &mut table.children {
            retain_old_display_table_child(child);
        }
        return kept;
    }

    if content.is::<GridElem>() {
        let mut kept = content.clone();
        let grid = kept.to_packed_mut::<GridElem>().expect("checked grid");
        for child in &mut grid.children {
            retain_old_display_grid_child(child);
        }
        return kept;
    }

    if let Some(box_elem) = content.to_packed::<BoxElem>() {
        let mut kept = content.clone();
        if let Some(body) = box_elem.body.get_cloned(StyleChain::default()) {
            kept.to_packed_mut::<BoxElem>()
                .expect("checked box")
                .body
                .set(Some(retain_old_display_content(&body)));
        }
        return kept;
    }

    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        let mut kept = content.clone();
        kept.to_packed_mut::<BlockElem>()
            .expect("checked block")
            .body
            .set(Some(BlockBody::Content(retain_old_display_content(&body))));
        return kept;
    }

    if let Some(align) = content.to_packed::<AlignElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<AlignElem>()
            .expect("checked align")
            .body = retain_old_display_content(&align.body);
        return kept;
    }

    if let Some(pad) = content.to_packed::<PadElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<PadElem>().expect("checked pad").body =
            retain_old_display_content(&pad.body);
        return kept;
    }

    if let Some(place) = content.to_packed::<PlaceElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<PlaceElem>()
            .expect("checked place")
            .body = retain_old_display_content(&place.body);
        return kept;
    }

    if let Some(columns) = content.to_packed::<ColumnsElem>() {
        let mut kept = content.clone();
        kept.to_packed_mut::<ColumnsElem>()
            .expect("checked columns")
            .body = retain_old_display_content(&columns.body);
        return kept;
    }

    if let Some(rect) = content.to_packed::<RectElem>() {
        let mut kept = content.clone();
        if let Some(body) = rect.body.get_cloned(StyleChain::default()) {
            kept.to_packed_mut::<RectElem>()
                .expect("checked rect")
                .body
                .set(Some(retain_old_display_content(&body)));
        }
        return kept;
    }

    if let Some(circle) = content.to_packed::<CircleElem>() {
        let mut kept = content.clone();
        if let Some(body) = circle.body.get_cloned(StyleChain::default()) {
            kept.to_packed_mut::<CircleElem>()
                .expect("checked circle")
                .body
                .set(Some(retain_old_display_content(&body)));
        }
        return kept;
    }

    if let Some(ellipse) = content.to_packed::<EllipseElem>() {
        let mut kept = content.clone();
        if let Some(body) = ellipse.body.get_cloned(StyleChain::default()) {
            kept.to_packed_mut::<EllipseElem>()
                .expect("checked ellipse")
                .body
                .set(Some(retain_old_display_content(&body)));
        }
        return kept;
    }

    let text = content.plain_text();
    if text.is_empty() {
        Content::sequence([])
    } else {
        TextElem::packed(text.as_str())
    }
}

fn retain_old_display_table_child(child: &mut TableChild) {
    match child {
        TableChild::Header(header) => {
            for item in &mut header.children {
                retain_old_display_table_item(item);
            }
        }
        TableChild::Footer(footer) => {
            for item in &mut footer.children {
                retain_old_display_table_item(item);
            }
        }
        TableChild::Item(item) => retain_old_display_table_item(item),
    }
}

fn retain_old_display_table_item(item: &mut TableItem) {
    if let TableItem::Cell(cell) = item {
        cell.body = retain_old_display_content(&cell.body);
    }
}

fn retain_old_display_grid_child(child: &mut GridChild) {
    match child {
        GridChild::Header(header) => {
            for item in &mut header.children {
                retain_old_display_grid_item(item);
            }
        }
        GridChild::Footer(footer) => {
            for item in &mut footer.children {
                retain_old_display_grid_item(item);
            }
        }
        GridChild::Item(item) => retain_old_display_grid_item(item),
    }
}

fn retain_old_display_grid_item(item: &mut GridItem) {
    if let GridItem::Cell(cell) = item {
        cell.body = retain_old_display_content(&cell.body);
    }
}

fn strip_old_display_styles(content: &Content) -> Content {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let child = strip_old_display_styles(&styled.child);
        let styles = old_display_styles(&styled.styles);
        return if styles.is_empty() {
            child
        } else {
            child.styled_with_map(styles)
        };
    }
    content.clone()
}

fn old_display_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| !is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect()
}

impl DiffResult {
    pub fn modification_log(&self) -> String {
        let mut log = format!(
            "generated_by: {}\n\n",
            crate::build_info::build_report_line()
        );
        for (index, block) in self.blocks.iter().enumerate() {
            log_block_edit(&mut log, block, index);
        }
        for region in &self.regions {
            log_region_edit(&mut log, region);
        }
        for region in &self.rendered_regions {
            log_rendered_region_edit(&mut log, region);
        }
        log
    }
}

fn log_rendered_region_edit(log: &mut String, region: &RenderedRegionEdit) {
    let index = region_log_index(RegionPath::RootPage(region.kind));
    for page in &region.pages {
        if !page.changed {
            continue;
        }
        log_edit_content(
            log,
            &EditContent::Modified {
                base: page.base.clone(),
                word_ops: page.word_ops.clone(),
            },
            index,
        );
    }
}

fn log_region_edit(log: &mut String, region: &DiffRegionEdit) {
    for edit in &region.edits {
        log_realized_edit(log, edit, region_log_index(region.path));
    }
}

fn region_log_index(path: RegionPath) -> usize {
    match path {
        RegionPath::RootPage(PageRegionKind::Header) => 0,
        RegionPath::RootPage(PageRegionKind::Footer) => 1,
        RegionPath::RootPage(PageRegionKind::Background) => 2,
        RegionPath::RootPage(PageRegionKind::Foreground) => 3,
    }
}

fn log_block_edit(log: &mut String, block: &DiffBlockEdit, index: usize) {
    for edit in &block.edits {
        log_realized_edit(log, edit, index);
    }
}

fn log_realized_edit(log: &mut String, edit: &RealizedEdit, index: usize) {
    match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content)
        | RealizedEdit::LogOnly(content)
        | RealizedEdit::MarkBaseInserted(content) => log_edit_content(log, content, index),
    }
}

fn realized_edit_kind(edit: &RealizedEdit) -> &'static str {
    match edit {
        RealizedEdit::ReplaceAt { content, .. } => edit_content_kind(content),
        RealizedEdit::InsertBefore { content, .. } => edit_content_kind(content),
        RealizedEdit::InsertAfter { content, .. } => edit_content_kind(content),
        RealizedEdit::Append { content } => edit_content_kind(content),
        RealizedEdit::WholeBlock(content) => edit_content_kind(content),
        RealizedEdit::LogOnly(content) => edit_content_kind(content),
        RealizedEdit::MarkBaseInserted(content) => edit_content_kind(content),
    }
}

fn edit_content_kind(content: &EditContent) -> &'static str {
    match content {
        EditContent::Inserted(_) => "inserted",
        EditContent::Deleted(_) => "deleted",
        EditContent::OpaqueReplacement { .. } => "opaque_replacement",
        EditContent::Modified { .. } => "modified",
        EditContent::Nested { .. } => "nested",
    }
}

fn log_edit_content(log: &mut String, content: &EditContent, index: usize) {
    match content {
        EditContent::Inserted(content) => {
            let text = content_log_text(content);
            if text.chars().any(|ch| !ch.is_whitespace()) {
                push_log_entry(log, index, "insert", &[("text", text)]);
            }
        }
        EditContent::Deleted(content) => {
            let text = content_log_text(content.as_content());
            if text.chars().any(|ch| !ch.is_whitespace()) {
                push_log_entry(log, index, "delete", &[("text", text)]);
            }
        }
        EditContent::OpaqueReplacement { .. } => push_log_entry(
            log,
            index,
            "modify",
            &[
                ("block", "[opaque visual content]".to_string()),
                ("deleted", "[old visual]".to_string()),
                ("inserted", "[new visual]".to_string()),
            ],
        ),
        EditContent::Modified { base, word_ops } => {
            if has_textual_word_change(word_ops) {
                let deletes = collect_word_op_text(word_ops, |op| match op {
                    WordOp::Delete(t) => Some(t),
                    _ => None,
                });
                let inserts = collect_word_op_text(word_ops, |op| match op {
                    WordOp::Insert(t) => Some(t),
                    _ => None,
                });
                push_log_entry(
                    log,
                    index,
                    "modify",
                    &[
                        ("block", modified_block_log_text(base, word_ops)),
                        ("deleted", deletes),
                        ("inserted", inserts),
                    ],
                );
            }
        }
        EditContent::Nested { edits, .. } => {
            for edit in edits {
                log_realized_edit(log, edit, index);
            }
        }
    }
}

fn modified_block_log_text(base: &Content, word_ops: &[WordOp]) -> String {
    if base.is::<FigureCaption>() {
        let text = collect_word_op_text(word_ops, |op| match op {
            WordOp::Equal(tokens) | WordOp::Insert(tokens) => Some(tokens),
            WordOp::Delete(_) => None,
        });
        if text.chars().any(|ch| !ch.is_whitespace()) {
            return text;
        }
    }
    content_log_text(base)
}

fn prepare_diff_inputs(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    capture_debug: bool,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<PreparedDiffInputs> {
    let new_surface = effective_text_content(new);
    let new_layout_blocks = extract_block_units(&new_surface);
    let old_realized_blocks = extract_block_units(&old.realized);
    let new_realized_blocks = extract_block_units(&new.realized);
    let old_blocks = non_parbreak_blocks(&old_realized_blocks);
    let new_blocks = non_parbreak_blocks(&new_realized_blocks);
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/block-extraction", "complete").reason(format!(
            "old_blocks={} new_blocks={} new_layout_blocks={}",
            old_blocks.len(),
            new_blocks.len(),
            new_layout_blocks.len()
        )),
    )?;
    let raw = diff_block_units_raw(&old_blocks, &new_blocks);
    if debug_events.is_some() {
        for (index, op) in raw.iter().enumerate() {
            emit_pipeline_trace_event(
                debug_events,
                block_op_trace_event("diff/block-myers", "raw_op", index, op),
            )?;
        }
    }
    let (matched, debug) = if capture_debug {
        let matched = match_edit_zones_inner(raw.clone(), debug_events)?;
        let debug = DiffBlockDebug {
            old_blocks,
            new_blocks,
            raw_ops: raw,
            matched_ops: matched.clone(),
        };
        (matched, Some(debug))
    } else {
        (match_edit_zones(raw), None)
    };
    Ok(PreparedDiffInputs {
        new_layout_blocks,
        old_realized_blocks,
        new_realized_blocks,
        matched_ops: matched,
        debug,
    })
}

pub fn diff_annotated(old: &AnnotatedContent, new: &AnnotatedContent) -> DiffResult {
    diff_annotated_inner(old, new, false)
        .expect("diff without trace cannot fail")
        .0
}

pub fn diff_annotated_with_block_debug(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
) -> (DiffResult, DiffBlockDebug) {
    let (result, debug) =
        diff_annotated_inner(old, new, true).expect("diff without trace cannot fail");
    (result, debug.expect("debug capture requested"))
}

pub fn diff_annotated_with_block_debug_events(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    debug_events: &mut dyn DebugEventSink,
) -> anyhow::Result<(DiffResult, DiffBlockDebug)> {
    let mut debug_events = Some(debug_events);
    let (result, debug) = diff_annotated_inner_with_events(old, new, true, &mut debug_events)?;
    Ok((result, debug.expect("debug capture requested")))
}

fn replace_block_edit(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    old_block: &DiffBlock,
    new_block: &DiffBlock,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<DiffBlockEdit> {
    let page_styles = new_block.page_styles.clone();
    let unique_changed_pair = match (old_ann, new_ann) {
        (Some(old_ann), Some(new_ann)) if can_recurse_via_slots(old_ann, new_ann) => None,
        (Some(old_ann), Some(new_ann)) => find_unique_changed_slot_pair(old_ann, new_ann),
        _ => find_unique_changed_slot_pair(old, new),
    };

    if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann)
        && can_recurse_via_slots(old_ann, new_ann)
    {
        if annotated_subtree_equal(old_ann, new_ann) {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("slot-recursive owners are equal")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind("noop"),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, None),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits: vec![],
                page_styles,
            });
        }
        let mut edits = footnote_visible_text_edits(old_ann, new_ann);
        edits.extend(diff_slot_edits_with_events(old_ann, new_ann, debug_events)?);
        if !edits.is_empty() {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("slot recursion produced edits")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind("slot_edits"),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, Some(new_ann)),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits,
                page_styles,
            });
        }
        if let Some(edit) = owned_surface_modified_edit(old_ann, new_ann) {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("owned surface changed after empty slot edits")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind(realized_edit_kind(&edit)),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, Some(new_ann)),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits: vec![edit],
                page_styles,
            });
        }
        emit_pipeline_trace_event(
            debug_events,
            PipelineTraceEvent::new("diff/replace-block", "selected")
                .reason("slot-recursive owners changed but produced no visible edits")
                .old_content(&old_block.content)
                .new_content(&new_block.content)
                .selected_edit_kind("noop"),
        )?;
        return Ok(DiffBlockEdit {
            base: annotated_block_from(&new_block.content, None),
            base_provenance: BlockBaseProvenance::LiveNew,
            edits: vec![],
            page_styles,
        });
    }

    if let Some((old_ann, new_ann)) = unique_changed_pair {
        let edits = diff_slot_edits_with_events(old_ann, new_ann, debug_events)?;
        if !edits.is_empty() {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("unique changed slot pair")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind("unique_slot_edits"),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, Some(new_ann)),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits,
                page_styles,
            });
        }
    }

    if let Some(content) =
        raw_block_modified_content(&old_block.content, &new_block.content, old_ann, new_ann)
    {
        let edit = RealizedEdit::WholeBlock(content);
        emit_pipeline_trace_event(
            debug_events,
            PipelineTraceEvent::new("diff/replace-block", "selected")
                .reason("raw block line diff")
                .old_content(&old_block.content)
                .new_content(&new_block.content)
                .selected_edit_kind(realized_edit_kind(&edit)),
        )?;
        return Ok(DiffBlockEdit {
            base: annotated_block_from(&new_block.content, new_ann),
            base_provenance: BlockBaseProvenance::LiveNew,
            edits: vec![edit],
            page_styles,
        });
    }

    if let Some(new_ann) = new_ann
        && all_slots_are_footnote_bodies(new_ann)
        && old_ann.is_none_or(|old_ann| !all_slots_are_footnote_bodies(old_ann))
    {
        let mut edits = inserted_footnote_body_edits(new_ann);
        edits.extend(deleted_visible_text_before_first_footnote(
            &old_block.content,
            new_ann,
        ));
        if !edits.is_empty() {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("inline text and inserted footnote body are separate edits")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind("footnote_body_insert"),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, Some(new_ann)),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits,
                page_styles,
            });
        }
    }

    if let Some(old_ann) = old_ann
        && all_slots_are_footnote_bodies(old_ann)
        && new_ann.is_none_or(|new_ann| !all_slots_are_footnote_bodies(new_ann))
    {
        let old_visible = footnote_owner_content_without_footnotes(old_ann);
        let old_tokens = extract_words(&old_visible);
        let new_tokens = extract_words_for_annotated_with_equation_origins(
            &new_block.content,
            new_ann,
            new_equation_origins,
        );
        let word_ops = diff_words(&old_tokens, &new_tokens);
        if has_textual_word_change(&word_ops) {
            emit_pipeline_trace_event(
                debug_events,
                PipelineTraceEvent::new("diff/replace-block", "selected")
                    .reason("deleted footnote body and inline text are separate edits")
                    .old_content(&old_block.content)
                    .new_content(&new_block.content)
                    .selected_edit_kind("footnote_body_to_inline"),
            )?;
            return Ok(DiffBlockEdit {
                base: annotated_block_from(&new_block.content, new_ann),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits: vec![RealizedEdit::WholeBlock(EditContent::Modified {
                    base: new_block.content.clone(),
                    word_ops,
                })],
                page_styles,
            });
        }
    }

    if block_context_changed(&old_block.content, &new_block.content, old_ann, new_ann)
        && replacement_has_word_change(
            old_block,
            new_block,
            old_ann,
            new_ann,
            old_equation_origins,
            new_equation_origins,
        )
    {
        emit_pipeline_trace_event(
            debug_events,
            PipelineTraceEvent::new("diff/replace-block", "selected")
                .reason("word diff across changed block context")
                .old_content(&old_block.content)
                .new_content(&new_block.content)
                .selected_edit_kind("context_split_replacement"),
        )?;
        return Ok(context_split_replacement_block_edit(
            old_block,
            new_block,
            old_ann,
            new_ann,
            old_equation_origins,
            new_equation_origins,
            page_styles,
        ));
    }

    let edits = word_or_opaque_replacement_edits(
        old_block,
        new_block,
        old_ann,
        new_ann,
        old_equation_origins,
        new_equation_origins,
    );
    let selected_edit_kind = edits.first().map(realized_edit_kind).unwrap_or("noop");
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/replace-block", "selected")
            .reason("word diff or opaque fallback")
            .old_content(&old_block.content)
            .new_content(&new_block.content)
            .selected_edit_kind(selected_edit_kind),
    )?;
    Ok(DiffBlockEdit {
        base: annotated_block_from(&new_block.content, None),
        base_provenance: BlockBaseProvenance::LiveNew,
        edits,
        page_styles,
    })
}

fn word_or_opaque_replacement_edits(
    old_block: &DiffBlock,
    new_block: &DiffBlock,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
) -> Vec<RealizedEdit> {
    modified_fragment_edit_content(
        &old_block.content,
        &new_block.content,
        old_ann,
        new_ann,
        old_equation_origins,
        new_equation_origins,
    )
    .map(|content| vec![RealizedEdit::WholeBlock(content)])
    .unwrap_or_default()
}

fn modified_fragment_edit_content(
    old_content: &Content,
    new_content: &Content,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
) -> Option<EditContent> {
    if let Some(content) = raw_block_modified_content(old_content, new_content, old_ann, new_ann) {
        return Some(content);
    }

    let (old_word_ann, new_word_ann) = balanced_word_annotations(old_ann, new_ann);
    let old_tokens = extract_words_for_annotated_with_equation_origins(
        old_content,
        old_word_ann,
        old_equation_origins,
    );
    let new_tokens = extract_words_for_annotated_with_equation_origins(
        new_content,
        new_word_ann,
        new_equation_origins,
    );
    let mut word_ops = diff_words(&old_tokens, &new_tokens);
    if !has_meaningful_equal(&word_ops) {
        let old_inline_tokens = tokens_without_outer_context(&old_tokens);
        let new_inline_tokens = tokens_without_outer_context(&new_tokens);
        let inline_word_ops = diff_words(&old_inline_tokens, &new_inline_tokens);
        if has_meaningful_equal(&inline_word_ops) {
            word_ops = inline_word_ops;
        }
    }
    word_ops = old_display_delete_ops(word_ops);
    if has_textual_word_change(&word_ops) {
        return Some(
            if contains_non_token_display_container_for(old_content, old_ann)
                || contains_non_token_display_container_for(new_content, new_ann)
            {
                EditContent::OpaqueReplacement {
                    old: old_display_surface_for_annotated(old_content, old_ann),
                    new: new_content.clone(),
                }
            } else {
                EditContent::Modified {
                    base: new_content.clone(),
                    word_ops,
                }
            },
        );
    }

    if opaque_visual_surface_changed(old_content, new_content, old_ann, new_ann) {
        return Some(EditContent::OpaqueReplacement {
            old: old_opaque_display_surface(old_content, old_ann),
            new: new_content.clone(),
        });
    }

    None
}

fn opaque_visual_surface_changed(
    old_content: &Content,
    new_content: &Content,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
) -> bool {
    old_content.plain_text().is_empty()
        && new_content.plain_text().is_empty()
        && old_content != new_content
        && (contains_opaque_visual_surface(old_content)
            || contains_opaque_visual_surface(new_content)
            || contains_text_empty_opaque_layouter_surface(old_content)
            || contains_text_empty_opaque_layouter_surface(new_content)
            || old_ann.is_some_and(is_opaque_visual_owner)
            || new_ann.is_some_and(is_opaque_visual_owner))
}

fn old_opaque_display_surface(
    fallback: &Content,
    annotated: Option<&AnnotatedContent>,
) -> OldDisplaySurface {
    let annotated_surface = annotated.map(effective_render_content);
    let content = annotated_surface
        .as_ref()
        .filter(|surface| contains_opaque_visual_surface(surface))
        .unwrap_or(fallback);
    OldDisplaySurface::new(content.clone())
}

fn is_opaque_visual_owner(node: &AnnotatedContent) -> bool {
    matches!(
        node.annotation.semantic_kind,
        Some(SemanticKind::Wrapper(
            WrapperKind::Rect | WrapperKind::Circle | WrapperKind::Ellipse
        ))
    ) || contains_opaque_visual_surface(&effective_render_content(node))
}

fn contains_opaque_visual_surface(content: &Content) -> bool {
    if content.is::<ContextElem>()
        || content.is::<MetadataElem>()
        || content.is::<StateUpdateElem>()
        || content.is::<TagElem>()
        || content.is::<RefElem>()
        || content.is::<PagebreakElem>()
        || content.is::<ParbreakElem>()
    {
        return false;
    }

    if is_opaque_visual_element_name(content.func().name()) {
        return true;
    }

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.iter().any(contains_opaque_visual_surface);
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return contains_opaque_visual_surface(&styled.child);
    }
    if let Some(block) = content.to_packed::<BlockElem>() {
        if block_has_visual_decoration(block) {
            return true;
        }
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            return contains_opaque_visual_surface(&body);
        }
    }
    container_ops::semantic_diff_child_contents(content)
        .iter()
        .any(contains_opaque_visual_surface)
}

fn contains_text_empty_opaque_layouter_surface(content: &Content) -> bool {
    if content.is::<ContextElem>()
        || content.is::<MetadataElem>()
        || content.is::<StateUpdateElem>()
        || content.is::<TagElem>()
        || content.is::<RefElem>()
        || content.is::<PagebreakElem>()
        || content.is::<ParbreakElem>()
    {
        return false;
    }

    if let Some(block) = content.to_packed::<BlockElem>()
        && content.plain_text().is_empty()
        && block_has_opaque_layouter_body(block)
    {
        return true;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .any(contains_text_empty_opaque_layouter_surface);
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return contains_text_empty_opaque_layouter_surface(&styled.child);
    }
    container_ops::semantic_diff_child_contents(content)
        .iter()
        .any(contains_text_empty_opaque_layouter_surface)
}

fn block_has_visual_decoration(block: &typst::foundations::Packed<BlockElem>) -> bool {
    let styles = StyleChain::default();
    if block.fill.get_cloned(styles).is_some() {
        return true;
    }
    let stroke = block.stroke.resolve(styles);
    stroke.left.is_some()
        || stroke.top.is_some()
        || stroke.right.is_some()
        || stroke.bottom.is_some()
}

fn block_has_opaque_layouter_body(block: &typst::foundations::Packed<BlockElem>) -> bool {
    matches!(
        block.body.get_cloned(StyleChain::default()),
        Some(body) if !matches!(body, BlockBody::Content(_))
    )
}

fn is_opaque_visual_element_name(name: &str) -> bool {
    matches!(
        name,
        "rect" | "circle" | "ellipse" | "line" | "polygon" | "path" | "image"
    )
}

fn raw_block_modified_content(
    old_content: &Content,
    new_content: &Content,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
) -> Option<EditContent> {
    let old_text = raw_fragment_text(old_ann, old_content)?;
    let new_text = raw_fragment_text(new_ann, new_content)?;
    if old_text == new_text {
        return None;
    }
    let word_ops = diff_raw_block_lines(&old_text, &new_text);
    Some(EditContent::Modified {
        base: new_content.clone(),
        word_ops,
    })
}

fn raw_fragment_text(node: Option<&AnnotatedContent>, fallback: &Content) -> Option<String> {
    if let Some(text) = node.and_then(raw_block_text) {
        return Some(text);
    }
    if let Some(node) = node {
        let mut texts = Vec::new();
        collect_annotated_raw_block_texts(node, &mut texts);
        if texts.len() == 1 {
            return texts.pop();
        }
    }
    single_raw_content_text(fallback)
}

fn raw_block_text(node: &AnnotatedContent) -> Option<String> {
    if node.annotation.semantic_kind != Some(SemanticKind::RawBlock) {
        return None;
    }
    let surface = node
        .annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized);
    Some(surface.plain_text().to_string())
}

fn collect_annotated_raw_block_texts(node: &AnnotatedContent, out: &mut Vec<String>) {
    if let Some(text) = raw_block_text(node) {
        out.push(text);
        return;
    }
    for child in &node.children {
        collect_annotated_raw_block_texts(child, out);
    }
}

fn single_raw_content_text(content: &Content) -> Option<String> {
    let mut texts = Vec::new();
    collect_raw_content_texts(content, &mut texts);
    (texts.len() == 1).then(|| texts.remove(0))
}

fn collect_raw_content_texts(content: &Content, out: &mut Vec<String>) {
    if content.to_packed::<RawElem>().is_some() {
        out.push(content.plain_text().to_string());
        return;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        for child in &seq.children {
            collect_raw_content_texts(child, out);
        }
        return;
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        collect_raw_content_texts(&styled.child, out);
        return;
    }
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        collect_raw_content_texts(&body, out);
    }
}

fn diff_raw_block_lines(old_text: &str, new_text: &str) -> Vec<WordOp> {
    let old_lines = raw_block_lines(old_text);
    let new_lines = raw_block_lines(new_text);
    let mut ops = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        match op {
            DiffOp::Equal {
                old_index: _,
                new_index,
                len,
            } => coalesce(
                &mut ops,
                WordOp::Equal(raw_line_tokens(&new_lines[new_index..new_index + len])),
            ),
            DiffOp::Delete {
                old_index,
                old_len,
                new_index: _,
            } => coalesce(
                &mut ops,
                WordOp::Delete(raw_line_tokens(&old_lines[old_index..old_index + old_len])),
            ),
            DiffOp::Insert {
                old_index: _,
                new_index,
                new_len,
            } => coalesce(
                &mut ops,
                WordOp::Insert(raw_line_tokens(&new_lines[new_index..new_index + new_len])),
            ),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                coalesce(
                    &mut ops,
                    WordOp::Delete(raw_line_tokens(&old_lines[old_index..old_index + old_len])),
                );
                coalesce(
                    &mut ops,
                    WordOp::Insert(raw_line_tokens(&new_lines[new_index..new_index + new_len])),
                );
            }
        }
    }
    ops
}

fn raw_block_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn raw_line_tokens(lines: &[String]) -> Vec<Token> {
    lines
        .iter()
        .map(|line| Token {
            text: line.clone(),
            content: TextElem::packed(line.as_str()),
        })
        .collect()
}

fn has_meaningful_equal(word_ops: &[WordOp]) -> bool {
    word_ops.iter().any(|op| match op {
        WordOp::Equal(tokens) => has_meaningful_tokens(tokens),
        _ => false,
    })
}

fn tokens_without_outer_context(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .map(|token| Token {
            text: token.text.clone(),
            content: token_content_for_direct_edit(token),
        })
        .collect()
}

fn balanced_word_annotations<'a>(
    old_ann: Option<&'a AnnotatedContent>,
    new_ann: Option<&'a AnnotatedContent>,
) -> (Option<&'a AnnotatedContent>, Option<&'a AnnotatedContent>) {
    let old_key = old_ann.and_then(|node| block_context_key(&node.realized));
    let new_key = new_ann.and_then(|node| block_context_key(&node.realized));
    if old_ann.is_some() == new_ann.is_some() && old_key == new_key {
        (old_ann, new_ann)
    } else {
        (None, None)
    }
}

fn replacement_has_word_change(
    old_block: &DiffBlock,
    new_block: &DiffBlock,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
) -> bool {
    let (old_word_ann, new_word_ann) = balanced_word_annotations(old_ann, new_ann);
    let old_tokens = extract_words_for_annotated_with_equation_origins(
        &old_block.content,
        old_word_ann,
        old_equation_origins,
    );
    let new_tokens = extract_words_for_annotated_with_equation_origins(
        &new_block.content,
        new_word_ann,
        new_equation_origins,
    );
    has_textual_word_change(&diff_words(&old_tokens, &new_tokens))
}

fn context_split_replacement_block_edit(
    old_block: &DiffBlock,
    new_block: &DiffBlock,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
    page_styles: Styles,
) -> DiffBlockEdit {
    let old_base = old_display_surface_for_annotated(&old_block.content, old_ann).content;
    let new_base = new_ann
        .map(|node| node.realized.clone())
        .unwrap_or_else(|| new_block.content.clone());
    let base_content = Content::sequence([old_base, new_base]);
    DiffBlockEdit {
        base: annotated_block_from(&base_content, None),
        base_provenance: BlockBaseProvenance::MixedInertOldLiveNew,
        edits: vec![
            RealizedEdit::ReplaceAt {
                path: vec![0],
                content: context_preserving_deleted_edit(
                    old_block.content.clone(),
                    old_ann,
                    old_equation_origins,
                ),
            },
            RealizedEdit::ReplaceAt {
                path: vec![1],
                content: context_preserving_inserted_edit(
                    new_block.content.clone(),
                    new_ann,
                    new_equation_origins,
                ),
            },
        ],
        page_styles,
    }
}

fn context_preserving_deleted_edit(
    content: Content,
    annotated: Option<&AnnotatedContent>,
    equation_origins: &[Content],
) -> EditContent {
    if contains_non_token_display_container_for(&content, annotated) {
        return deleted_edit_for_annotated(&content, annotated);
    }

    let tokens =
        extract_words_for_annotated_with_equation_origins(&content, annotated, equation_origins);
    if has_meaningful_tokens(&tokens) {
        let tokens = old_display_tokens(tokens);
        let base = old_display_surface_for_annotated(&content, annotated).content;
        EditContent::Modified {
            base,
            word_ops: vec![WordOp::Delete(tokens)],
        }
    } else {
        deleted_edit_for_annotated(&content, annotated)
    }
}

fn context_preserving_inserted_edit(
    content: Content,
    annotated: Option<&AnnotatedContent>,
    equation_origins: &[Content],
) -> EditContent {
    if contains_non_token_display_container_for(&content, annotated) {
        return EditContent::Inserted(content);
    }

    let tokens =
        extract_words_for_annotated_with_equation_origins(&content, annotated, equation_origins);
    if has_meaningful_tokens(&tokens) {
        let base = annotated
            .map(|node| node.realized.clone())
            .unwrap_or_else(|| content.clone());
        EditContent::Modified {
            base,
            word_ops: vec![WordOp::Insert(tokens)],
        }
    } else {
        EditContent::Inserted(content)
    }
}

fn contains_non_token_display_container(content: &Content) -> bool {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return contains_non_token_display_container(&styled.child);
    }

    if content.is::<TableElem>() || content.is::<GridElem>() {
        return true;
    }

    if let Some(box_elem) = content.to_packed::<BoxElem>() {
        return box_elem
            .body
            .get_cloned(StyleChain::default())
            .is_some_and(|body| contains_non_token_display_container(&body));
    }

    if let Some(par) = content.to_packed::<ParElem>() {
        return contains_non_token_display_container(&par.body);
    }

    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return contains_non_token_display_container(&heading.body);
    }

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return contains_non_token_display_container(&caption.body);
    }

    if let Some(footnote) = content.to_packed::<FootnoteElem>()
        && let FootnoteBody::Content(body) = &footnote.body
    {
        return contains_non_token_display_container(body);
    }

    if let Some(figure) = content.to_packed::<FigureElem>() {
        return contains_non_token_display_container(&figure.body)
            || figure
                .caption
                .get_cloned(StyleChain::default())
                .is_some_and(|caption| contains_non_token_display_container(&caption.body));
    }

    if let Some(link) = content.to_packed::<LinkElem>() {
        return contains_non_token_display_container(&link.body);
    }

    if let Some(strong) = content.to_packed::<StrongElem>() {
        return contains_non_token_display_container(&strong.body);
    }

    if let Some(emph) = content.to_packed::<EmphElem>() {
        return contains_non_token_display_container(&emph.body);
    }

    if let Some(highlight) = content.to_packed::<HighlightElem>() {
        return contains_non_token_display_container(&highlight.body);
    }

    if let Some(list) = content.to_packed::<ListElem>() {
        return list
            .children
            .iter()
            .any(|item| contains_non_token_display_container(&item.body));
    }

    if let Some(enm) = content.to_packed::<EnumElem>() {
        return enm
            .children
            .iter()
            .any(|item| contains_non_token_display_container(&item.body));
    }

    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return contains_non_token_display_container(&body);
    }

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .any(contains_non_token_display_container);
    }

    false
}

fn contains_non_token_display_container_for(
    content: &Content,
    annotated: Option<&AnnotatedContent>,
) -> bool {
    annotated
        .map(effective_render_content)
        .is_some_and(|content| contains_non_token_display_container(&content))
        || contains_non_token_display_container(content)
}

fn block_context_changed(
    old: &Content,
    new: &Content,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
) -> bool {
    match (
        block_context_key_for(old, old_ann),
        block_context_key_for(new, new_ann),
    ) {
        (Some(old_key), Some(new_key)) => old_key != new_key,
        _ => false,
    }
}

fn block_context_key_for(
    content: &Content,
    annotated: Option<&AnnotatedContent>,
) -> Option<String> {
    if annotated.is_some_and(|node| node.annotation.semantic_kind == Some(SemanticKind::Heading)) {
        return Some(context_presentation_key(content));
    }
    annotated
        .and_then(annotated_block_context_key)
        .or_else(|| block_context_key(content))
}

fn annotated_block_context_key(node: &AnnotatedContent) -> Option<String> {
    match node.annotation.semantic_kind {
        Some(SemanticKind::Paragraph) => block_context_key(&node.realized),
        Some(SemanticKind::Heading) => Some(context_presentation_key(&node.realized)),
        _ => None,
    }
}

fn semantic_heading_context(
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
) -> bool {
    matches!(
        (old_ann, new_ann),
        (Some(old), Some(new))
            if old.annotation.semantic_kind == Some(SemanticKind::Heading)
                && new.annotation.semantic_kind == Some(SemanticKind::Heading)
    )
}

fn context_presentation_key(content: &Content) -> String {
    let mut out = String::new();
    write_context_presentation_key(content, &mut out);
    out
}

fn write_context_presentation_key(content: &Content, out: &mut String) {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        out.push_str("seq[");
        for child in &seq.children {
            write_context_presentation_key(child, out);
            out.push(';');
        }
        out.push(']');
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        out.push_str("styled(");
        write_styles_key(&styled.styles, out);
        out.push_str(")[");
        write_context_presentation_key(&styled.child, out);
        out.push(']');
    } else if let Some(par) = content.to_packed::<ParElem>() {
        out.push_str("par[");
        write_context_presentation_key(&par.body, out);
        out.push(']');
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        out.push_str("heading(");
        out.push_str(&format!(
            "level={:?}:depth={}:offset={}",
            heading.level.get(StyleChain::default()),
            heading.depth.get(StyleChain::default()).get(),
            heading.offset.get(StyleChain::default())
        ));
        out.push_str(")[");
        write_context_presentation_key(&heading.body, out);
        out.push(']');
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        out.push_str("block[");
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            write_context_presentation_key(&body, out);
        }
        out.push(']');
    } else if is_metadata_tag(content) || content.is::<TextElem>() || content.is::<SpaceElem>() {
    } else {
        let children = container_ops::semantic_diff_child_contents(content);
        if !children.is_empty() {
            out.push_str(content.func().name());
            out.push('[');
            for child in children {
                write_context_presentation_key(&child, out);
                out.push(';');
            }
            out.push(']');
        } else {
            out.push_str(content.func().name());
        }
    }
}

fn block_context_key(content: &Content) -> Option<String> {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return block_context_key(&styled.child);
    }

    if content.is::<ParElem>() {
        return Some("par".to_string());
    }

    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return Some(format!(
            "heading:level={:?}:depth={}:offset={}",
            heading.level.get(StyleChain::default()),
            heading.depth.get(StyleChain::default()).get(),
            heading.offset.get(StyleChain::default())
        ));
    }

    if let Some(block) = content.to_packed::<BlockElem>() {
        return Some(format!(
            "block:{:?}",
            block
                .body
                .get_cloned(StyleChain::default())
                .map(|body| match body {
                    BlockBody::Content(_) => "content",
                    _ => "other",
                })
        ));
    }

    if content.is::<TableElem>() {
        return Some("table".to_string());
    }

    if content.is::<GridElem>() {
        return Some("grid".to_string());
    }

    if content.is::<ListElem>() {
        return Some("list".to_string());
    }

    if content.is::<EnumElem>() {
        return Some("enum".to_string());
    }

    if content.is::<FigureElem>() {
        return Some("figure".to_string());
    }

    None
}

fn is_block_context(content: &Content) -> bool {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return is_block_context(&styled.child);
    }
    content.is::<BlockElem>()
}

fn is_empty_block_shell(content: &Content) -> bool {
    content.plain_text().trim().is_empty() && is_block_context(content)
}

fn presentation_changed(
    old_block: &DiffBlock,
    new_block: &DiffBlock,
    old_ann: Option<&AnnotatedContent>,
    new_ann: Option<&AnnotatedContent>,
    old_equation_origins: &[Content],
    new_equation_origins: &[Content],
) -> bool {
    let old_tokens = extract_words_for_annotated_with_equation_origins(
        &old_block.content,
        old_ann,
        old_equation_origins,
    );
    let new_tokens = extract_words_for_annotated_with_equation_origins(
        &new_block.content,
        new_ann,
        new_equation_origins,
    );
    if !old_tokens.is_empty() || !new_tokens.is_empty() {
        if diff_words(&old_tokens, &new_tokens)
            .iter()
            .any(|op| !matches!(op, WordOp::Equal(_)))
        {
            return true;
        }
    }

    presentation_key(&old_block.content) != presentation_key(&new_block.content)
}

fn inserted_block_edit(
    content: Content,
    annotated: Option<&AnnotatedContent>,
    equation_origins: &[Content],
) -> EditContent {
    if contains_non_token_display_container_for(&content, annotated) {
        return EditContent::Inserted(content);
    }

    let tokens =
        extract_words_for_annotated_with_equation_origins(&content, annotated, equation_origins);
    if content.plain_text().is_empty() && has_meaningful_tokens(&tokens) {
        EditContent::Modified {
            base: content,
            word_ops: vec![WordOp::Insert(tokens)],
        }
    } else {
        EditContent::Inserted(content)
    }
}

fn deleted_block_edit(
    content: Content,
    annotated: Option<&AnnotatedContent>,
    equation_origins: &[Content],
) -> EditContent {
    if contains_non_token_display_container_for(&content, annotated) {
        return deleted_edit_for_annotated(&content, annotated);
    }

    let tokens =
        extract_words_for_annotated_with_equation_origins(&content, annotated, equation_origins);
    if content.plain_text().is_empty() && has_meaningful_tokens(&tokens) {
        let tokens = old_display_tokens(tokens);
        EditContent::Modified {
            base: old_display_surface_for_annotated(&content, annotated).content,
            word_ops: vec![WordOp::Delete(tokens)],
        }
    } else {
        deleted_edit_for_annotated(&content, annotated)
    }
}

fn diff_annotated_inner(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    capture_debug: bool,
) -> anyhow::Result<(DiffResult, Option<DiffBlockDebug>)> {
    let mut no_debug_events = None;
    diff_annotated_inner_with_events(old, new, capture_debug, &mut no_debug_events)
}

fn diff_annotated_inner_with_events(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    capture_debug: bool,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<(DiffResult, Option<DiffBlockDebug>)> {
    let prepared = prepare_diff_inputs(old, new, capture_debug, debug_events)?;
    let old_region_styles = document_page_styles_raw(&old.realized, &prepared.old_realized_blocks);
    let new_region_styles = document_page_styles_raw(&new.realized, &prepared.new_realized_blocks);
    let regions = diff_root_page_regions(&old_region_styles, &new_region_styles);
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/page-region", "semantic_complete")
            .reason(format!("region_count={}", regions.len())),
    )?;
    let root_styles = sanitize_page_styles(root_page_styles_raw(&new.realized));

    let mut layout = LayoutCursor::new(&prepared.new_layout_blocks);
    let mut old_owners = BlockOwnerCursor::new(old);
    let mut new_owners = BlockOwnerCursor::new(new);
    let mut old_equation_origins = EquationOriginBlockCursor::new(old);
    let mut new_equation_origins = EquationOriginBlockCursor::new(new);
    let mut blocks = Vec::new();
    let mut recursed_equal_semantic_nodes = HashSet::new();
    let mut recursed_display_surfaces = Vec::new();
    let mut deferred_old_owner: Option<BlockOwnerMatch<'_>> = None;
    let mut deferred_new_owner: Option<BlockOwnerMatch<'_>> = None;
    let mut deferred_old_equation_origins: Vec<Content> = Vec::new();
    let mut deferred_new_equation_origins: Vec<Content> = Vec::new();
    let mut pending_display_equation_carriers = 0usize;
    let matched_ops = prepared.matched_ops;
    let mut op_index = 0;
    while op_index < matched_ops.len() {
        let op = matched_ops[op_index].clone();
        op_index += 1;
        match op {
            BlockOp::Equal(old_block, new_block) => {
                let page_styles = new_block.page_styles.clone();
                let old_claim = old_owners.take_claim_for(&old_block.content);
                let new_claim = new_owners.take_claim_for(&new_block.content);
                let new_claim_key = new_claim.key.clone();
                let old_ann = old_claim
                    .owner
                    .or_else(|| find_annotated_block_owner(old, &old_block.content));
                let new_ann = new_claim
                    .owner
                    .or_else(|| find_annotated_block_owner(new, &new_block.content));
                emit_pipeline_trace_event(
                    debug_events,
                    PipelineTraceEvent::new("diff/owner-claim", "equal_block")
                        .reason(format!(
                            "old_owner={} new_owner={}",
                            old_ann.is_some(),
                            new_ann.is_some()
                        ))
                        .old_content(&old_block.content)
                        .new_content(&new_block.content),
                )?;
                blocks.extend(layout.take_before(&new_block.content));
                if pending_display_equation_carriers > 0 && is_empty_block_shell(&new_block.content)
                {
                    pending_display_equation_carriers -= 1;
                    continue;
                }
                if old_ann.zip(new_ann).is_some_and(|(old_ann, new_ann)| {
                    should_defer_invisible_owner_edit(&old_block.content, old_ann)
                        && should_defer_invisible_owner_edit(&new_block.content, new_ann)
                }) {
                    deferred_old_owner = Some(old_claim.clone());
                    deferred_new_owner = Some(new_claim.clone());
                    if let Some(old_ann) = old_ann
                        && should_defer_invisible_equation_owner(&old_block.content, old_ann)
                    {
                        deferred_old_equation_origins
                            .extend(old_ann.annotation.equation_origins.iter().cloned());
                    }
                    if let Some(new_ann) = new_ann
                        && should_defer_invisible_equation_owner(&new_block.content, new_ann)
                    {
                        deferred_new_equation_origins
                            .extend(new_ann.annotation.equation_origins.iter().cloned());
                    }
                    blocks.push(DiffBlockEdit {
                        base: annotated_block_from(&new_block.content, None),
                        base_provenance: BlockBaseProvenance::LiveNew,
                        edits: vec![],
                        page_styles,
                    });
                    continue;
                }
                if old_block.content.plain_text().trim().is_empty()
                    && new_block.content.plain_text().trim().is_empty()
                    && !old_ann.is_some_and(is_display_equation_owner)
                    && !new_ann.is_some_and(is_display_equation_owner)
                {
                    blocks.push(DiffBlockEdit {
                        base: annotated_block_from(&new_block.content, None),
                        base_provenance: BlockBaseProvenance::LiveNew,
                        edits: vec![],
                        page_styles,
                    });
                    continue;
                }
                let old_eq_origins = old_equation_origins.take_for(&old_block.content);
                let new_eq_origins = new_equation_origins.take_for(&new_block.content);
                if new_ann.is_none()
                    && consume_recursed_display_surface(
                        &mut recursed_display_surfaces,
                        &new_block.content,
                    )
                {
                    continue;
                }
                if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann)
                    && can_recurse_via_slots(old_ann, new_ann)
                {
                    if owner_already_recursed(&recursed_equal_semantic_nodes, &new_claim_key) {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, None),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits: vec![],
                            page_styles,
                        });
                        continue;
                    }
                    if annotated_subtree_equal(old_ann, new_ann) {
                        mark_recursed_owner(&mut recursed_equal_semantic_nodes, &new_claim_key);
                        mark_recursed_display_surface(&mut recursed_display_surfaces, new_ann);
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, None),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits: vec![],
                            page_styles,
                        });
                        continue;
                    }
                    let mut edits = footnote_visible_text_edits(old_ann, new_ann);
                    edits.extend(diff_slot_edits_with_events(old_ann, new_ann, debug_events)?);
                    if !edits.is_empty() {
                        mark_recursed_owner(&mut recursed_equal_semantic_nodes, &new_claim_key);
                        mark_recursed_display_surface(&mut recursed_display_surfaces, new_ann);
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits,
                            page_styles,
                        });
                        continue;
                    }
                    if new_block.content.plain_text().is_empty()
                        && let Some(edit) = owned_surface_modified_edit(old_ann, new_ann)
                    {
                        mark_recursed_owner(&mut recursed_equal_semantic_nodes, &new_claim_key);
                        mark_recursed_display_surface(&mut recursed_display_surfaces, new_ann);
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits: vec![edit],
                            page_styles,
                        });
                        continue;
                    }
                }
                if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann)
                    && (has_equation_origins(old_ann) || has_equation_origins(new_ann))
                {
                    let old_tokens = extract_words_for_annotated_with_equation_origins(
                        &old_block.content,
                        Some(old_ann),
                        &old_eq_origins,
                    );
                    let new_tokens = extract_words_for_annotated_with_equation_origins(
                        &new_block.content,
                        Some(new_ann),
                        &new_eq_origins,
                    );
                    let word_ops = old_display_delete_ops(diff_words(&old_tokens, &new_tokens));
                    if has_textual_word_change(&word_ops) {
                        if is_display_equation_owner(new_ann) {
                            pending_display_equation_carriers += 1;
                        }
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits: vec![RealizedEdit::WholeBlock(EditContent::Modified {
                                base: new_block.content.clone(),
                                word_ops,
                            })],
                            page_styles,
                        });
                        continue;
                    }
                    if is_empty_realized_equation_owner(&new_block.content, new_ann) {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits: vec![],
                            page_styles,
                        });
                        continue;
                    }
                }
                if presentation_changed(
                    &old_block,
                    &new_block,
                    old_ann,
                    new_ann,
                    &old_eq_origins,
                    &new_eq_origins,
                ) {
                    if (block_context_changed(
                        &old_block.content,
                        &new_block.content,
                        old_ann,
                        new_ann,
                    ) || semantic_heading_context(old_ann, new_ann)
                        || (is_block_context(&old_block.content)
                            && is_block_context(&new_block.content)))
                        && replacement_has_word_change(
                            &old_block,
                            &new_block,
                            old_ann,
                            new_ann,
                            &old_eq_origins,
                            &new_eq_origins,
                        )
                    {
                        emit_pipeline_trace_event(
                            debug_events,
                            PipelineTraceEvent::new("diff/equal-block", "selected")
                                .reason("presentation identity changed across block context")
                                .old_content(&old_block.content)
                                .new_content(&new_block.content)
                                .selected_edit_kind("context_split_replacement"),
                        )?;
                        blocks.push(context_split_replacement_block_edit(
                            &old_block,
                            &new_block,
                            old_ann,
                            new_ann,
                            &old_eq_origins,
                            &new_eq_origins,
                            page_styles,
                        ));
                        continue;
                    }
                    let edits = word_or_opaque_replacement_edits(
                        &old_block,
                        &new_block,
                        old_ann,
                        new_ann,
                        &old_eq_origins,
                        &new_eq_origins,
                    );
                    if !edits.is_empty() {
                        emit_pipeline_trace_event(
                            debug_events,
                            PipelineTraceEvent::new("diff/equal-block", "selected")
                                .reason("presentation identity changed")
                                .old_content(&old_block.content)
                                .new_content(&new_block.content)
                                .selected_edit_kind(
                                    edits.first().map(realized_edit_kind).unwrap_or("noop"),
                                ),
                        )?;
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, new_ann),
                            base_provenance: BlockBaseProvenance::LiveNew,
                            edits,
                            page_styles,
                        });
                        continue;
                    }
                }
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&new_block.content, None),
                    base_provenance: BlockBaseProvenance::LiveNew,
                    edits: vec![],
                    page_styles: new_block.page_styles,
                });
            }
            BlockOp::Delete(old_block) => {
                let old_eq_origins = old_equation_origins.take_for(&old_block.content);
                let old_claim = old_owners.take_claim_for(&old_block.content);
                if let Some(BlockOp::Insert(new_block)) = matched_ops.get(op_index).cloned() {
                    let new_claim = new_owners.peek_claim_for(&new_block.content);
                    if semantic_owner_claims_match(&old_claim, &new_claim) {
                        emit_pipeline_trace_event(
                            debug_events,
                            PipelineTraceEvent::new("diff/owner-claim", "semantic_match")
                                .reason("delete/insert owners share semantic kind and ordinal")
                                .old_content(&old_block.content)
                                .new_content(&new_block.content)
                                .selected_edit_kind("replace"),
                        )?;
                        let new_claim = new_owners.take_claim_for(&new_block.content);
                        let new_eq_origins = new_equation_origins.take_for(&new_block.content);
                        if owner_already_recursed(&recursed_equal_semantic_nodes, &new_claim.key) {
                            blocks.extend(layout.take_before(&new_block.content));
                            op_index += 1;
                            continue;
                        }
                        blocks.extend(layout.take_before(&new_block.content));
                        let replaced = replace_block_edit(
                            old,
                            new,
                            &old_block,
                            &new_block,
                            old_claim.owner,
                            new_claim.owner,
                            &old_eq_origins,
                            &new_eq_origins,
                            debug_events,
                        )?;
                        if old_claim
                            .owner
                            .zip(new_claim.owner)
                            .is_some_and(|(old_ann, new_ann)| {
                                can_recurse_via_slots(old_ann, new_ann)
                                    && !replaced.edits.is_empty()
                            })
                        {
                            mark_recursed_owner(&mut recursed_equal_semantic_nodes, &new_claim.key);
                            if let Some(new_ann) = new_claim.owner {
                                mark_recursed_display_surface(
                                    &mut recursed_display_surfaces,
                                    new_ann,
                                );
                            }
                        }
                        blocks.push(replaced);
                        op_index += 1;
                        continue;
                    }
                    if !old_claim
                        .owner
                        .zip(new_claim.owner)
                        .is_some_and(|(old_ann, new_ann)| can_recurse_via_slots(old_ann, new_ann))
                        && consume_recursed_display_surface(
                            &mut recursed_display_surfaces,
                            &new_block.content,
                        )
                    {
                        emit_pipeline_trace_event(
                            debug_events,
                            PipelineTraceEvent::new("diff/display-surface", "suppressed")
                                .reason("changed display surface already recursed through owner")
                                .old_content(&old_block.content)
                                .new_content(&new_block.content)
                                .selected_edit_kind("noop"),
                        )?;
                        blocks.extend(layout.take_before(&new_block.content));
                        new_owners.take_claim_for(&new_block.content);
                        new_equation_origins.take_for(&new_block.content);
                        op_index += 1;
                        continue;
                    }
                }
                let old_ann = old_claim
                    .owner
                    .or_else(|| find_annotated_block_owner(old, &old_block.content));
                let old_base =
                    old_display_surface_for_annotated(&old_block.content, old_ann).content;
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&old_base, None),
                    base_provenance: BlockBaseProvenance::InertOld,
                    edits: vec![RealizedEdit::WholeBlock(deleted_block_edit(
                        old_block.content.clone(),
                        old_ann,
                        &old_eq_origins,
                    ))],
                    page_styles: current_annotated_page_styles(&blocks),
                });
            }
            BlockOp::Insert(new_block) => {
                let new_eq_origins = new_equation_origins.take_for(&new_block.content);
                let new_ann = new_owners
                    .take_owner_for(&new_block.content)
                    .or_else(|| find_annotated_block_owner(new, &new_block.content));
                blocks.extend(layout.take_before(&new_block.content));
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&new_block.content, new_ann),
                    base_provenance: BlockBaseProvenance::LiveNew,
                    edits: if let Some(new_ann) = new_ann
                        && is_empty_realized_equation_owner(&new_block.content, new_ann)
                    {
                        let base = live_display_equation_block(new_ann)
                            .unwrap_or_else(|| new_block.content.clone());
                        let tokens = extract_words(&base);
                        vec![RealizedEdit::MarkBaseInserted(EditContent::Modified {
                            base,
                            word_ops: vec![WordOp::Insert(tokens)],
                        })]
                    } else {
                        vec![RealizedEdit::WholeBlock(inserted_block_edit(
                            new_block.content.clone(),
                            new_ann,
                            &new_eq_origins,
                        ))]
                    },
                    page_styles: new_block.page_styles,
                });
            }
            BlockOp::Replace(old_block, new_block) => {
                let mut old_eq_origins = old_equation_origins.take_for(&old_block.content);
                let mut new_eq_origins = new_equation_origins.take_for(&new_block.content);
                if realized_equation_carrier_count_for_diff(&old_block.content) > 0 {
                    old_eq_origins.append(&mut deferred_old_equation_origins);
                }
                if realized_equation_carrier_count_for_diff(&new_block.content) > 0 {
                    new_eq_origins.append(&mut deferred_new_equation_origins);
                }
                let mut old_claim = old_owners.take_claim_for(&old_block.content);
                let mut new_claim = new_owners.take_claim_for(&new_block.content);
                if old_claim.owner.is_none()
                    && old_block.content.plain_text().trim().is_empty()
                    && let Some(deferred) = deferred_old_owner.take()
                {
                    old_claim = deferred;
                }
                if new_claim.owner.is_none()
                    && new_block.content.plain_text().trim().is_empty()
                    && let Some(deferred) = deferred_new_owner.take()
                {
                    new_claim = deferred;
                }
                let old_ann = old_claim
                    .owner
                    .or_else(|| find_annotated_block_owner(old, &old_block.content));
                let new_ann = new_claim
                    .owner
                    .or_else(|| find_annotated_block_owner(new, &new_block.content));
                if owner_already_recursed(&recursed_equal_semantic_nodes, &new_claim.key) {
                    blocks.extend(layout.take_before(&new_block.content));
                    continue;
                }
                if !old_ann
                    .zip(new_ann)
                    .is_some_and(|(old_ann, new_ann)| can_recurse_via_slots(old_ann, new_ann))
                    && consume_recursed_display_surface(
                        &mut recursed_display_surfaces,
                        &new_block.content,
                    )
                {
                    emit_pipeline_trace_event(
                        debug_events,
                        PipelineTraceEvent::new("diff/display-surface", "suppressed")
                            .reason("changed display surface already recursed through owner")
                            .old_content(&old_block.content)
                            .new_content(&new_block.content)
                            .selected_edit_kind("noop"),
                    )?;
                    blocks.extend(layout.take_before(&new_block.content));
                    continue;
                }
                if pending_display_equation_carriers > 0 && is_empty_block_shell(&new_block.content)
                {
                    pending_display_equation_carriers -= 1;
                    blocks.extend(layout.take_before(&new_block.content));
                    continue;
                }
                blocks.extend(layout.take_before(&new_block.content));
                let replaced = replace_block_edit(
                    old,
                    new,
                    &old_block,
                    &new_block,
                    old_ann,
                    new_ann,
                    &old_eq_origins,
                    &new_eq_origins,
                    debug_events,
                )?;
                if old_ann.zip(new_ann).is_some_and(|(old_ann, new_ann)| {
                    can_recurse_via_slots(old_ann, new_ann) && !replaced.edits.is_empty()
                }) {
                    mark_recursed_owner(&mut recursed_equal_semantic_nodes, &new_claim.key);
                    if let Some(new_ann) = new_ann {
                        mark_recursed_display_surface(&mut recursed_display_surfaces, new_ann);
                    }
                }
                blocks.push(replaced);
            }
        }
    }
    blocks.extend(layout.take_trailing());
    let before_prune_edits = count_block_edits(&blocks);
    prune_duplicate_empty_container_edits(&mut blocks);
    let after_prune_edits = count_block_edits(&blocks);
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/prune-duplicates", "complete").reason(format!(
            "edits_before={before_prune_edits} edits_after={after_prune_edits}"
        )),
    )?;
    let before_footnote_blocks = blocks.len();
    append_footnote_body_edits(old, new, &mut blocks);
    prune_invisible_old_deletions(&mut blocks);
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/footnote-body", "complete").reason(format!(
            "blocks_before={} blocks_after={}",
            before_footnote_blocks,
            blocks.len()
        )),
    )?;

    let result = DiffResult {
        blocks,
        root_styles,
        regions,
        rendered_regions: vec![],
    };
    Ok((result, prepared.debug))
}

fn count_block_edits(blocks: &[DiffBlockEdit]) -> usize {
    blocks.iter().map(|block| block.edits.len()).sum()
}

fn prune_duplicate_empty_container_edits(blocks: &mut [DiffBlockEdit]) {
    let mut owned_empty_signatures = HashSet::new();
    for block in blocks.iter() {
        if !is_owned_empty_edit_block(block) {
            continue;
        }
        for edit in &block.edits {
            collect_modified_signatures(edit, &mut owned_empty_signatures);
        }
    }

    if !owned_empty_signatures.is_empty() {
        for block in &mut *blocks {
            if is_owned_empty_edit_block(block) {
                continue;
            }
            let had_edits = !block.edits.is_empty();
            block.edits.retain(|edit| {
                !edit_modified_signatures(edit)
                    .iter()
                    .any(|sig| owned_empty_signatures.contains(sig))
            });
            if had_edits && block.edits.is_empty() {
                suppress_block_surface(block);
            }
        }
    }

    let mut nonempty_signatures = HashSet::new();
    for block in blocks.iter() {
        if block.base.realized.plain_text().is_empty() {
            continue;
        }
        for edit in &block.edits {
            collect_modified_signatures(edit, &mut nonempty_signatures);
        }
    }

    if !nonempty_signatures.is_empty() {
        for block in &mut *blocks {
            if !block.base.realized.plain_text().is_empty() {
                continue;
            }
            retain_block_edits(block, |edit| {
                !edit_modified_signatures(edit)
                    .iter()
                    .any(|sig| nonempty_signatures.contains(sig))
            });
        }
    }

    let mut previous_wrapper_text_edit = false;
    for block in &mut *blocks {
        if previous_wrapper_text_edit && block.base.realized.plain_text().is_empty() {
            retain_block_edits(block, |edit| !edit_is_opaque_replacement(edit));
            if block.edits.is_empty() {
                suppress_block_surface(block);
            }
        }
        previous_wrapper_text_edit =
            is_owned_empty_wrapper_edit_block(block) && block_has_modified_signature(block);
    }

    let mut seen_signatures = HashSet::new();
    for block in blocks {
        retain_block_edits(block, |edit| {
            let signatures = edit_modified_signatures(edit);
            signatures.is_empty()
                || signatures
                    .iter()
                    .all(|signature| seen_signatures.insert(signature.clone()))
        });
    }
}

fn prune_invisible_old_deletions(blocks: &mut Vec<DiffBlockEdit>) {
    for block in blocks.iter_mut() {
        prune_invisible_old_delete_edits(&mut block.edits);
    }
    blocks.retain(|block| {
        !(block.base_provenance == BlockBaseProvenance::InertOld
            && block.edits.is_empty()
            && effective_render_content(&block.base)
                .plain_text()
                .is_empty()
            && !contains_opaque_visual_surface(&effective_render_content(&block.base)))
    });
}

fn prune_invisible_old_delete_edits(edits: &mut Vec<RealizedEdit>) {
    for edit in edits.iter_mut() {
        prune_nested_invisible_old_deletions(edit);
    }
    edits.retain(|edit| !is_invisible_old_delete_edit(edit));
}

fn prune_nested_invisible_old_deletions(edit: &mut RealizedEdit) {
    let content = match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content)
        | RealizedEdit::LogOnly(content)
        | RealizedEdit::MarkBaseInserted(content) => content,
    };
    if let EditContent::Nested { edits, .. } = content {
        prune_invisible_old_delete_edits(edits);
    }
}

fn is_invisible_old_delete_edit(edit: &RealizedEdit) -> bool {
    match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content) => is_invisible_old_delete_content(content),
        RealizedEdit::LogOnly(_) | RealizedEdit::MarkBaseInserted(_) => false,
    }
}

fn is_invisible_old_delete_content(content: &EditContent) -> bool {
    match content {
        EditContent::Deleted(surface) => {
            let content = surface.as_content();
            content.plain_text().is_empty() && !contains_opaque_visual_surface(content)
        }
        EditContent::Nested { edits, .. } => edits.is_empty(),
        EditContent::Inserted(_)
        | EditContent::OpaqueReplacement { .. }
        | EditContent::Modified { .. } => false,
    }
}

fn is_owned_empty_edit_block(block: &DiffBlockEdit) -> bool {
    block.base.realized.plain_text().is_empty() && !block.base.annotation.slots.is_empty()
}

fn is_owned_empty_wrapper_edit_block(block: &DiffBlockEdit) -> bool {
    block.base.realized.plain_text().is_empty()
        && matches!(
            block.base.annotation.semantic_kind,
            Some(SemanticKind::Wrapper(_) | SemanticKind::Stack)
        )
}

fn block_has_modified_signature(block: &DiffBlockEdit) -> bool {
    block
        .edits
        .iter()
        .any(|edit| !edit_modified_signatures(edit).is_empty())
}

fn edit_is_opaque_replacement(edit: &RealizedEdit) -> bool {
    match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content) => {
            matches!(content, EditContent::OpaqueReplacement { .. })
        }
        RealizedEdit::LogOnly(_) | RealizedEdit::MarkBaseInserted(_) => false,
    }
}

fn suppress_block_surface(block: &mut DiffBlockEdit) {
    block.base.realized = Content::sequence([]);
    block.base.annotation.patch_surface = None;
    block.base.children.clear();
}

fn retain_block_edits(block: &mut DiffBlockEdit, mut keep: impl FnMut(&RealizedEdit) -> bool) {
    let had_edits = !block.edits.is_empty();
    block.edits.retain(|edit| keep(edit));
    if had_edits && block.edits.is_empty() && block.base.realized.plain_text().is_empty() {
        block.base.annotation.patch_surface = None;
    }
}

fn append_footnote_body_edits(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    blocks: &mut Vec<DiffBlockEdit>,
) {
    let old_bodies = footnote_body_contents(old);
    let new_bodies = footnote_body_contents(new);
    if old_bodies.is_empty() {
        return;
    }
    if new_bodies.is_empty() {
        append_deleted_footnote_body_edits(old_bodies, blocks);
    }
}

fn append_deleted_footnote_body_edits(old_bodies: Vec<Content>, blocks: &mut Vec<DiffBlockEdit>) {
    for old_body in old_bodies {
        let tokens = extract_words(&old_body);
        if tokens.is_empty() {
            continue;
        }
        let inert_body = retain_old_display_content(&old_body);
        let footnote = Content::new(FootnoteElem::new(FootnoteBody::Content(inert_body.clone())));
        let base = annotate_realized(&footnote, &footnote);
        let Some(path) = base
            .annotation
            .slots
            .iter()
            .find(|slot| matches!(slot.label, SlotStep::FootnoteBody))
            .map(|slot| slot.path.clone())
        else {
            continue;
        };
        blocks.push(DiffBlockEdit {
            base,
            base_provenance: BlockBaseProvenance::InertOld,
            edits: vec![RealizedEdit::ReplaceAt {
                path,
                content: EditContent::Modified {
                    base: inert_body,
                    word_ops: vec![WordOp::Delete(old_display_tokens(tokens))],
                },
            }],
            page_styles: Styles::new(),
        });
    }
}

fn footnote_body_contents(root: &AnnotatedContent) -> Vec<Content> {
    let mut out = Vec::new();
    collect_footnote_body_contents(root, &mut out);
    out
}

fn collect_footnote_body_contents(node: &AnnotatedContent, out: &mut Vec<Content>) {
    let slots = node
        .annotation
        .slots
        .iter()
        .filter(|slot| matches!(slot.label, SlotStep::FootnoteBody))
        .collect::<Vec<_>>();
    if !slots.is_empty() {
        for slot in slots {
            if let Some(body) = node.get_path(&slot.path) {
                out.push(effective_text_content(body));
            }
        }
        return;
    }

    if let Some(footnote) = &node.annotation.footnote
        && let Some(body) = footnote_body_content(&footnote.body)
    {
        out.push(body);
    }
    for child in &node.children {
        collect_footnote_body_contents(child, out);
    }
}

fn footnote_body_content(content: &Content) -> Option<Content> {
    let footnote = content.to_packed::<FootnoteElem>()?;
    let FootnoteBody::Content(body) = &footnote.body else {
        return None;
    };
    Some(body.clone())
}

fn edit_modified_signatures(edit: &RealizedEdit) -> Vec<String> {
    let mut signatures = HashSet::new();
    collect_modified_signatures(edit, &mut signatures);
    signatures.into_iter().collect()
}

fn collect_modified_signatures(edit: &RealizedEdit, signatures: &mut HashSet<String>) {
    match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content)
        | RealizedEdit::LogOnly(content)
        | RealizedEdit::MarkBaseInserted(content) => {
            collect_edit_content_signature(content, signatures)
        }
    }
}

fn collect_edit_content_signature(content: &EditContent, signatures: &mut HashSet<String>) {
    match content {
        EditContent::Modified { base, word_ops, .. } if has_textual_word_change(word_ops) => {
            signatures.insert(format!(
                "{}\n{}\n{}",
                single_line(&base.plain_text()),
                single_line(&collect_word_op_text(word_ops, |op| match op {
                    WordOp::Delete(t) => Some(t),
                    _ => None,
                })),
                single_line(&collect_word_op_text(word_ops, |op| match op {
                    WordOp::Insert(t) => Some(t),
                    _ => None,
                }))
            ));
        }
        EditContent::Nested { edits, .. } => {
            for edit in edits {
                collect_modified_signatures(edit, signatures);
            }
        }
        EditContent::OpaqueReplacement { old, new } => {
            signatures.insert(format!(
                "opaque\n{}\n{}",
                content_signature(old.as_content()),
                content_signature(new)
            ));
        }
        EditContent::Inserted(_) | EditContent::Deleted(_) | EditContent::Modified { .. } => {}
    }
}

pub fn diff_annotated_with_rendered_regions(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    old_world: &dyn World,
    new_world: &dyn World,
) -> anyhow::Result<DiffResult> {
    Ok(diff_annotated_with_rendered_regions_inner(old, new, old_world, new_world, None, false)?.0)
}

pub fn diff_annotated_with_rendered_regions_and_debug(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    old_world: &dyn World,
    new_world: &dyn World,
) -> anyhow::Result<(DiffResult, DiffBlockDebug)> {
    let (result, debug) =
        diff_annotated_with_rendered_regions_inner(old, new, old_world, new_world, None, true)?;
    Ok((result, debug.expect("debug capture requested")))
}

pub fn diff_annotated_with_rendered_regions_and_debug_events(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    old_world: &dyn World,
    new_world: &dyn World,
    debug_events: &mut dyn DebugEventSink,
) -> anyhow::Result<(DiffResult, DiffBlockDebug)> {
    let (result, debug) = diff_annotated_with_rendered_regions_inner(
        old,
        new,
        old_world,
        new_world,
        Some(debug_events),
        true,
    )?;
    Ok((result, debug.expect("debug capture requested")))
}

fn diff_annotated_with_rendered_regions_inner(
    old: &AnnotatedContent,
    new: &AnnotatedContent,
    old_world: &dyn World,
    new_world: &dyn World,
    mut debug_events: Option<&mut dyn DebugEventSink>,
    capture_debug: bool,
) -> anyhow::Result<(DiffResult, Option<DiffBlockDebug>)> {
    let (mut result, debug) =
        diff_annotated_inner_with_events(old, new, capture_debug, &mut debug_events)?;
    let old_source = crate::eval::eval_to_content(old_world)?;
    let new_source = crate::eval::eval_to_content(new_world)?;
    let old_doc = crate::eval::layout_document(old_world, &old_source)?;
    let new_doc = crate::eval::layout_document(new_world, &new_source)?;
    let old_styles = document_page_styles_raw(&old.realized, &extract_block_units(&old.realized));
    let new_styles = document_page_styles_raw(&new.realized, &extract_block_units(&new.realized));
    let new_source_styles =
        document_page_styles_raw(&new_source, &extract_block_units(&new_source));
    result.rendered_regions = diff_rendered_root_page_regions(
        &old_styles,
        &new_styles,
        &new_source_styles,
        &old_doc,
        &new_doc,
        new_world,
        &result.regions,
        debug_events,
    )?;
    Ok((result, debug))
}

fn diff_root_page_regions(old_styles: &Styles, new_styles: &Styles) -> Vec<DiffRegionEdit> {
    [
        PageRegionKind::Header,
        PageRegionKind::Footer,
        PageRegionKind::Background,
        PageRegionKind::Foreground,
    ]
    .into_iter()
    .filter_map(|kind| {
        diff_page_region(
            kind,
            page_region_content(old_styles, kind),
            page_region_content(new_styles, kind),
        )
    })
    .collect()
}

fn document_page_styles_raw(content: &Content, blocks: &[DiffBlock]) -> Styles {
    let root = root_page_styles_raw(content);
    if !root.is_empty() {
        return root;
    }
    blocks
        .iter()
        .find_map(|block| (!block.page_styles.is_empty()).then(|| block.page_styles.clone()))
        .unwrap_or_default()
}

fn page_region_content(styles: &Styles, kind: PageRegionKind) -> Option<Content> {
    let chain = StyleChain::new(styles);
    match kind {
        PageRegionKind::Header => chain.get_cloned(PageElem::header).custom().flatten(),
        PageRegionKind::Footer => chain.get_cloned(PageElem::footer).custom().flatten(),
        PageRegionKind::Background => chain.get_cloned(PageElem::background),
        PageRegionKind::Foreground => chain.get_cloned(PageElem::foreground),
    }
}

fn diff_page_region(
    kind: PageRegionKind,
    old_content: Option<Content>,
    new_content: Option<Content>,
) -> Option<DiffRegionEdit> {
    match (old_content, new_content) {
        (None, None) => None,
        (None, Some(new_content)) => {
            let base = annotate_realized(&new_content, &new_content);
            Some(DiffRegionEdit {
                path: RegionPath::RootPage(kind),
                edits: vec![RealizedEdit::WholeBlock(EditContent::Inserted(new_content))],
                base,
            })
        }
        (Some(old_content), None) => {
            let base = annotate_realized(&old_content, &old_content);
            Some(DiffRegionEdit {
                path: RegionPath::RootPage(kind),
                edits: vec![RealizedEdit::WholeBlock(deleted_edit(old_content))],
                base,
            })
        }
        (Some(old_content), Some(new_content)) => {
            let old_ann = annotate_realized(&old_content, &old_content);
            let new_ann = annotate_realized(&new_content, &new_content);
            let edits = diff_region_edits(&old_ann, &new_ann);
            (!edits.is_empty()).then_some(DiffRegionEdit {
                path: RegionPath::RootPage(kind),
                base: new_ann,
                edits,
            })
        }
    }
}

fn diff_region_edits(old: &AnnotatedContent, new: &AnnotatedContent) -> Vec<RealizedEdit> {
    if annotated_subtree_equal(old, new) {
        return vec![];
    }

    if can_recurse_via_slots(old, new) {
        return diff_slot_edits(old, new);
    }

    if let Some(content) = recursive_slot_edit_content(old, new) {
        return vec![RealizedEdit::WholeBlock(content)];
    }

    let old_effective = effective_text_content(old);
    let new_effective = effective_text_content(new);
    modified_fragment_edit_content(
        &old_effective,
        &new_effective,
        Some(old),
        Some(new),
        &[],
        &[],
    )
    .map(|content| vec![RealizedEdit::WholeBlock(content)])
    .unwrap_or_default()
}

fn diff_rendered_root_page_regions(
    old_styles: &Styles,
    new_styles: &Styles,
    new_source_styles: &Styles,
    old_doc: &PagedDocument,
    new_doc: &PagedDocument,
    new_world: &dyn World,
    semantic_regions: &[DiffRegionEdit],
    mut debug_events: Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<Vec<RenderedRegionEdit>> {
    let mut rendered_regions = Vec::new();

    for kind in [
        PageRegionKind::Header,
        PageRegionKind::Footer,
        PageRegionKind::Background,
        PageRegionKind::Foreground,
    ]
    .into_iter()
    {
        if semantic_regions
            .iter()
            .any(|region| region.path == RegionPath::RootPage(kind))
        {
            emit_pipeline_trace_event(
                &mut debug_events,
                PipelineTraceEvent::new("diff/rendered-region", "skip").reason(format!(
                    "{} has a semantic page-region diff",
                    page_region_name(kind)
                )),
            )?;
            continue;
        }
        let old_content = page_region_content(old_styles, kind);
        let new_content = page_region_content(new_styles, kind);
        if old_content.is_none() && new_content.is_none() {
            emit_pipeline_trace_event(
                &mut debug_events,
                PipelineTraceEvent::new("diff/rendered-region", "skip")
                    .reason(format!("{} absent on both sides", page_region_name(kind))),
            )?;
            continue;
        }
        if old_content != new_content {
            emit_pipeline_trace_event(
                &mut debug_events,
                PipelineTraceEvent::new("diff/rendered-region", "skip").reason(format!(
                    "{} differs semantically; rendered fallback not needed",
                    page_region_name(kind)
                )),
            )?;
            continue;
        }
        let wrapper = new_content
            .as_ref()
            .and_then(|_| page_region_content(new_source_styles, kind))
            .as_ref()
            .map(|content| rendered_region_wrapper(new_world, content))
            .unwrap_or_default();
        let old_pages = rendered_region_texts("old", old_styles, old_doc, kind, &mut debug_events)?;
        let new_pages = rendered_region_texts("new", new_styles, new_doc, kind, &mut debug_events)?;
        if let Some(region) = diff_rendered_region_texts(kind, wrapper, &old_pages, &new_pages) {
            emit_pipeline_trace_event(
                &mut debug_events,
                PipelineTraceEvent::new("diff/rendered-region", "selected")
                    .reason(format!(
                        "{} rendered text changed on {} pages",
                        page_region_name(kind),
                        region.pages.iter().filter(|page| page.changed).count()
                    ))
                    .selected_edit_kind("rendered_region"),
            )?;
            rendered_regions.push(region);
        } else {
            emit_pipeline_trace_event(
                &mut debug_events,
                PipelineTraceEvent::new("diff/rendered-region", "noop").reason(format!(
                    "{} rendered text unchanged",
                    page_region_name(kind)
                )),
            )?;
        }
    }

    Ok(rendered_regions)
}

fn diff_rendered_region_texts(
    kind: PageRegionKind,
    wrapper: RenderedRegionWrapper,
    old_pages: &[RenderedRegionText],
    new_pages: &[RenderedRegionText],
) -> Option<RenderedRegionEdit> {
    let mut has_change = false;
    let pages = new_pages
        .iter()
        .enumerate()
        .map(|(index, new_page)| {
            let old_page = old_pages.get(index);
            let new_content = rendered_region_page_content(new_page);
            let old_content = old_page
                .map(rendered_region_page_content)
                .unwrap_or_else(|| TextElem::packed(""));
            let old_tokens = extract_words(&old_content);
            let new_tokens = extract_words(&new_content);
            let word_ops = diff_words(&old_tokens, &new_tokens);
            let segments = diff_rendered_region_segments(old_page, new_page);
            let changed = old_pages.get(index).is_some() && has_textual_word_change(&word_ops);
            has_change |= changed;
            RenderedRegionPageEdit {
                page: index + 1,
                base: new_content,
                word_ops,
                segments,
                changed,
            }
        })
        .collect::<Vec<_>>();

    has_change.then_some(RenderedRegionEdit {
        kind,
        wrapper,
        pages,
    })
}

fn diff_rendered_region_segments(
    old_page: Option<&RenderedRegionText>,
    new_page: &RenderedRegionText,
) -> Vec<RenderedRegionSegmentEdit> {
    if new_page.clusters.len() <= 1 {
        return vec![];
    }
    let Some(old_page) = old_page else {
        return vec![];
    };
    if old_page.clusters.len() != new_page.clusters.len() {
        return vec![];
    }

    old_page
        .clusters
        .iter()
        .zip(new_page.clusters.iter())
        .map(|(old_cluster, new_cluster)| {
            let old_tokens = extract_words(&old_cluster.content);
            let new_tokens = extract_words(&new_cluster.content);
            RenderedRegionSegmentEdit {
                base: new_cluster.content.clone(),
                word_ops: diff_words(&old_tokens, &new_tokens),
            }
        })
        .collect()
}

fn rendered_region_page_content(page: &RenderedRegionText) -> Content {
    Content::sequence(page.clusters.iter().map(|cluster| cluster.content.clone()))
}

fn rendered_region_wrapper(world: &dyn World, content: &Content) -> RenderedRegionWrapper {
    let span = content.span();
    let Some(id) = span.id() else {
        return RenderedRegionWrapper::None;
    };
    let Ok(source) = world.source(id) else {
        return RenderedRegionWrapper::None;
    };
    let Some(range) = source.range(span) else {
        return RenderedRegionWrapper::None;
    };
    let Some(snippet) = source.text().get(range) else {
        return RenderedRegionWrapper::None;
    };
    authored_align_wrapper(snippet).unwrap_or_default()
}

fn authored_align_wrapper(source: &str) -> Option<RenderedRegionWrapper> {
    let mut found = None;
    let mut rest = source;
    while let Some(index) = rest.find("align") {
        let after = &rest[index + "align".len()..];
        if !after.starts_with(|c: char| c.is_whitespace() || c == '(') {
            rest = after;
            continue;
        }
        if let Some(alignment) = parse_align_call_alignment(after)
            && found
                .replace(RenderedRegionWrapper::Align(alignment))
                .is_some()
        {
            return None;
        }
        rest = after;
    }
    found
}

fn parse_align_call_alignment(after_align: &str) -> Option<RenderedRegionAlignment> {
    let mut chars = after_align.trim_start().chars();
    if chars.next()? != '(' {
        return None;
    }
    let args = chars.as_str();
    let close = args.find(')')?;
    let first_arg = args[..close].split(',').next()?.trim();
    let first_arg = first_arg.strip_prefix("alignment.").unwrap_or(first_arg);
    match first_arg {
        "left" => Some(RenderedRegionAlignment::Left),
        "center" => Some(RenderedRegionAlignment::Center),
        "right" => Some(RenderedRegionAlignment::Right),
        "start" => Some(RenderedRegionAlignment::Start),
        "end" => Some(RenderedRegionAlignment::End),
        _ => None,
    }
}

fn rendered_region_texts(
    side: &str,
    styles: &Styles,
    document: &PagedDocument,
    kind: PageRegionKind,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<Vec<RenderedRegionText>> {
    let semantic_region_exists = page_region_content(styles, kind).is_some();
    let mut out = Vec::new();
    for (index, page) in document.pages.iter().enumerate() {
        out.push(rendered_region_text(
            side,
            index + 1,
            &page.frame,
            kind,
            semantic_region_exists,
            debug_events,
        )?);
    }
    Ok(out)
}

fn rendered_region_text(
    side: &str,
    page: usize,
    page_frame: &Frame,
    kind: PageRegionKind,
    semantic_region_exists: bool,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<RenderedRegionText> {
    let trace_id = debug_events
        .is_some()
        .then(|| frame_trace_id(side, kind, page));
    if let Some(sink) = debug_events.as_deref_mut() {
        sink.rendered_region_trace_start(&RenderedRegionTraceStart {
            trace_id: trace_id.clone().unwrap(),
            side: side.to_string(),
            kind,
            page,
            page_width_pt: page_frame.width().to_pt(),
            page_height_pt: page_frame.height().to_pt(),
            semantic_region_exists,
        })?;
    }
    let mut runs = Vec::new();
    let mut event_index = 0;
    let mut tag_stack = Vec::new();
    collect_positioned_region_trace(
        trace_id.as_deref(),
        side,
        page,
        page_frame,
        Point::zero(),
        page_frame.width().to_pt(),
        page_frame.height().to_pt(),
        kind,
        &mut tag_stack,
        0,
        &mut Vec::new(),
        &mut runs,
        &mut event_index,
        debug_events,
    )?;
    let extracted = RenderedRegionText::from_runs(runs, page_frame.width().to_pt());
    if let Some(sink) = debug_events.as_deref_mut() {
        sink.rendered_region_trace_end(&RenderedRegionTraceEnd {
            trace_id: trace_id.unwrap(),
            side: side.to_string(),
            kind,
            page,
            extracted_text: extracted.text.clone(),
            event_count: event_index,
        })?;
    }
    Ok(extracted)
}

#[derive(Clone)]
struct RenderedRegionText {
    text: String,
    clusters: Vec<RenderedRegionCluster>,
}

#[derive(Clone)]
struct RenderedRegionCluster {
    text: String,
    content: Content,
}

#[derive(Clone)]
struct RenderedRegionRun {
    x: f64,
    y: f64,
    width: f64,
    text: String,
    content: Content,
}

impl RenderedRegionText {
    fn from_runs(mut runs: Vec<RenderedRegionRun>, page_width_pt: f64) -> Self {
        if runs.is_empty() {
            return Self {
                text: String::new(),
                clusters: Vec::new(),
            };
        }

        runs.sort_by(|a, b| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        });

        let mut clusters = Vec::new();
        let line_y_tolerance = 2.0;
        let gap_threshold = (page_width_pt * 0.10).max(36.0);
        let mut line = Vec::new();
        let mut line_y = runs[0].y;

        for run in runs {
            if (run.y - line_y).abs() > line_y_tolerance {
                push_line_clusters(&mut clusters, &mut line, gap_threshold);
                line_y = run.y;
            }
            line.push(run);
        }
        push_line_clusters(&mut clusters, &mut line, gap_threshold);

        Self {
            text: clusters
                .iter()
                .map(|cluster| cluster.text.as_str())
                .collect(),
            clusters,
        }
    }
}

fn push_line_clusters(
    clusters: &mut Vec<RenderedRegionCluster>,
    line: &mut Vec<RenderedRegionRun>,
    gap_threshold: f64,
) {
    if line.is_empty() {
        return;
    }
    line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));

    let mut current = String::new();
    let mut current_content = Vec::new();
    let mut previous_end = None;
    for run in line.drain(..) {
        if let Some(end) = previous_end {
            let gap = run.x - end;
            if gap > gap_threshold && !current.is_empty() {
                clusters.push(RenderedRegionCluster {
                    text: std::mem::take(&mut current),
                    content: Content::sequence(current_content.drain(..)),
                });
            }
        }
        current.push_str(&run.text);
        current_content.push(run.content);
        previous_end = Some(run.x + run.width);
    }
    if !current.is_empty() {
        clusters.push(RenderedRegionCluster {
            text: current,
            content: Content::sequence(current_content),
        });
    }
}

fn collect_positioned_region_trace(
    trace_id: Option<&str>,
    side: &str,
    page: usize,
    frame: &Frame,
    origin: Point,
    page_width_pt: f64,
    page_height_pt: f64,
    kind: PageRegionKind,
    tag_stack: &mut Vec<FrameTag>,
    group_depth: usize,
    frame_path: &mut Vec<usize>,
    out: &mut Vec<RenderedRegionRun>,
    event_index: &mut usize,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<()> {
    let tracing = debug_events.is_some();
    for (item_index, (pos, item)) in frame.items().enumerate() {
        if tracing {
            frame_path.push(item_index);
        }
        let absolute = origin + *pos;
        match item {
            FrameItem::Tag(Tag::Start(content, _)) => {
                let is_artifact = content.elem().name() == "artifact";
                let before = artifact_depth(tag_stack);
                tag_stack.push(FrameTag {
                    is_artifact,
                    elem_name: content.elem().name().to_string(),
                });
                let after = artifact_depth(tag_stack);
                emit_frame_trace_event(
                    debug_events,
                    trace_id,
                    side,
                    kind,
                    page,
                    event_index,
                    frame_path,
                    group_depth,
                    *pos,
                    absolute,
                    "tag_start",
                    None,
                    None,
                    Some("start"),
                    Some(content.elem().name()),
                    before,
                    after,
                    before != after,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
            }
            FrameItem::Tag(Tag::End(_, _, _)) => {
                let before = artifact_depth(tag_stack);
                tag_stack.pop();
                let after = artifact_depth(tag_stack);
                emit_frame_trace_event(
                    debug_events,
                    trace_id,
                    side,
                    kind,
                    page,
                    event_index,
                    frame_path,
                    group_depth,
                    *pos,
                    absolute,
                    "tag_end",
                    None,
                    None,
                    Some("end"),
                    None,
                    before,
                    after,
                    before != after,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
            }
            FrameItem::Text(text) => {
                let artifact_depth = artifact_depth(tag_stack);
                let in_region_band = point_belongs_to_region(absolute, page_height_pt, kind);
                let included = artifact_depth > 0 && in_region_band;
                let excluded_reason = if included {
                    None
                } else if artifact_depth == 0 {
                    Some("not_artifact")
                } else {
                    Some("outside_region")
                };
                if included {
                    out.push(RenderedRegionRun {
                        x: absolute.x.to_pt(),
                        y: absolute.y.to_pt(),
                        width: text.width().to_pt(),
                        text: text.text.to_string(),
                        content: rendered_region_run_content(text.text.as_str(), tag_stack),
                    });
                }
                emit_frame_trace_event(
                    debug_events,
                    trace_id,
                    side,
                    kind,
                    page,
                    event_index,
                    frame_path,
                    group_depth,
                    *pos,
                    absolute,
                    "text",
                    Some(text.text.as_str()),
                    Some(text.text.chars().count()),
                    None,
                    None,
                    artifact_depth,
                    artifact_depth,
                    false,
                    Some(in_region_band),
                    Some(included),
                    excluded_reason,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
            }
            FrameItem::Group(group) => {
                let artifact_depth = artifact_depth(tag_stack);
                emit_frame_trace_event(
                    debug_events,
                    trace_id,
                    side,
                    kind,
                    page,
                    event_index,
                    frame_path,
                    group_depth,
                    *pos,
                    absolute,
                    "group_start",
                    None,
                    None,
                    None,
                    None,
                    artifact_depth,
                    artifact_depth,
                    false,
                    None,
                    None,
                    None,
                    Some(origin.x.to_pt()),
                    Some(origin.y.to_pt()),
                    Some(absolute.x.to_pt()),
                    Some(absolute.y.to_pt()),
                    Some(pos.x.to_pt()),
                    Some(pos.y.to_pt()),
                )?;
                collect_positioned_region_trace(
                    trace_id,
                    side,
                    page,
                    &group.frame,
                    absolute,
                    page_width_pt,
                    page_height_pt,
                    kind,
                    tag_stack,
                    group_depth + 1,
                    frame_path,
                    out,
                    event_index,
                    debug_events,
                )?;
                emit_frame_trace_event(
                    debug_events,
                    trace_id,
                    side,
                    kind,
                    page,
                    event_index,
                    frame_path,
                    group_depth,
                    *pos,
                    absolute,
                    "group_end",
                    None,
                    None,
                    None,
                    None,
                    artifact_depth,
                    artifact_depth,
                    false,
                    None,
                    None,
                    None,
                    Some(absolute.x.to_pt()),
                    Some(absolute.y.to_pt()),
                    Some(origin.x.to_pt()),
                    Some(origin.y.to_pt()),
                    Some(pos.x.to_pt()),
                    Some(pos.y.to_pt()),
                )?;
            }
            FrameItem::Shape(_, _) => emit_frame_trace_event(
                debug_events,
                trace_id,
                side,
                kind,
                page,
                event_index,
                frame_path,
                group_depth,
                *pos,
                absolute,
                "shape",
                None,
                None,
                None,
                None,
                artifact_depth(tag_stack),
                artifact_depth(tag_stack),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )?,
            FrameItem::Image(_, _, _) => emit_frame_trace_event(
                debug_events,
                trace_id,
                side,
                kind,
                page,
                event_index,
                frame_path,
                group_depth,
                *pos,
                absolute,
                "image",
                None,
                None,
                None,
                None,
                artifact_depth(tag_stack),
                artifact_depth(tag_stack),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )?,
            FrameItem::Link(_, _) => emit_frame_trace_event(
                debug_events,
                trace_id,
                side,
                kind,
                page,
                event_index,
                frame_path,
                group_depth,
                *pos,
                absolute,
                "link",
                None,
                None,
                None,
                None,
                artifact_depth(tag_stack),
                artifact_depth(tag_stack),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )?,
        }
        if tracing {
            frame_path.pop();
        }
    }
    Ok(())
}

struct FrameTag {
    is_artifact: bool,
    elem_name: String,
}

fn artifact_depth(tag_stack: &[FrameTag]) -> usize {
    tag_stack.iter().filter(|tag| tag.is_artifact).count()
}

fn rendered_region_run_content(text: &str, tag_stack: &[FrameTag]) -> Content {
    let mut content = TextElem::packed(text);
    for tag in tag_stack.iter().rev() {
        if tag.elem_name == "emph" {
            content = content.emph();
        }
    }
    content
}

fn emit_frame_trace_event(
    debug_events: &mut Option<&mut dyn DebugEventSink>,
    trace_id: Option<&str>,
    side: &str,
    kind: PageRegionKind,
    page: usize,
    event_index: &mut usize,
    frame_path: &[usize],
    group_depth: usize,
    local: Point,
    absolute: Point,
    item_kind: &'static str,
    text: Option<&str>,
    text_len: Option<usize>,
    tag_direction: Option<&'static str>,
    tag_element: Option<&str>,
    artifact_depth_before: usize,
    artifact_depth_after: usize,
    changed_artifact_state: bool,
    in_region_band: Option<bool>,
    included: Option<bool>,
    excluded_reason: Option<&'static str>,
    group_origin_before_x_pt: Option<f64>,
    group_origin_before_y_pt: Option<f64>,
    group_origin_after_x_pt: Option<f64>,
    group_origin_after_y_pt: Option<f64>,
    group_offset_x_pt: Option<f64>,
    group_offset_y_pt: Option<f64>,
) -> anyhow::Result<()> {
    if let Some(sink) = debug_events.as_deref_mut() {
        sink.rendered_region_trace_event(&FrameTraceEvent {
            trace_id: trace_id
                .expect("trace id is present when tracing")
                .to_string(),
            side: side.to_string(),
            kind,
            page,
            event_index: *event_index,
            frame_path: frame_path.to_vec(),
            group_depth,
            local_x_pt: local.x.to_pt(),
            local_y_pt: local.y.to_pt(),
            absolute_x_pt: absolute.x.to_pt(),
            absolute_y_pt: absolute.y.to_pt(),
            item_kind,
            text: text.map(str::to_string),
            text_len,
            tag_direction,
            tag_element: tag_element.map(str::to_string),
            artifact_depth_before,
            artifact_depth_after,
            changed_artifact_state,
            in_region_band,
            included,
            excluded_reason,
            group_origin_before_x_pt,
            group_origin_before_y_pt,
            group_origin_after_x_pt,
            group_origin_after_y_pt,
            group_offset_x_pt,
            group_offset_y_pt,
        })?;
    }
    *event_index += 1;
    Ok(())
}

fn point_belongs_to_region(point: Point, page_height_pt: f64, kind: PageRegionKind) -> bool {
    let y = point.y.to_pt();
    match kind {
        PageRegionKind::Header => y < page_height_pt * 0.2,
        PageRegionKind::Footer => y > page_height_pt * 0.8,
        PageRegionKind::Background | PageRegionKind::Foreground => true,
    }
}

fn frame_trace_id(side: &str, kind: PageRegionKind, page: usize) -> String {
    format!("{side}/{}/page-{page}", page_region_name(kind))
}

fn page_region_name(kind: PageRegionKind) -> &'static str {
    match kind {
        PageRegionKind::Header => "header",
        PageRegionKind::Footer => "footer",
        PageRegionKind::Background => "background",
        PageRegionKind::Foreground => "foreground",
    }
}

fn non_parbreak_blocks(blocks: &[DiffBlock]) -> Vec<DiffBlock> {
    blocks
        .iter()
        .filter(|block| !block.content.is::<ParbreakElem>())
        .cloned()
        .collect()
}

struct LayoutCursor<'a> {
    blocks: &'a [DiffBlock],
    index: usize,
}

impl<'a> LayoutCursor<'a> {
    fn new(blocks: &'a [DiffBlock]) -> Self {
        Self { blocks, index: 0 }
    }

    fn take_before(&mut self, target: &Content) -> Vec<DiffBlockEdit> {
        let mut out = Vec::new();
        while let Some(block) = self.blocks.get(self.index) {
            self.index += 1;
            if layout_content_matches(&block.content, target) {
                break;
            }
            if block.content.is::<ParbreakElem>() {
                out.push(layout_block_edit(block));
            }
        }
        out
    }

    fn take_trailing(&mut self) -> Vec<DiffBlockEdit> {
        let mut out = Vec::new();
        while let Some(block) = self.blocks.get(self.index) {
            self.index += 1;
            if block.content.is::<ParbreakElem>() {
                out.push(layout_block_edit(block));
            }
        }
        out
    }
}

struct BlockOwnerClaim<'a> {
    content: Content,
    owner: Option<&'a AnnotatedContent>,
    key: Option<SemanticOwnerKey>,
}

#[derive(Clone)]
struct BlockOwnerMatch<'a> {
    owner: Option<&'a AnnotatedContent>,
    key: Option<SemanticOwnerKey>,
}

struct BlockOwnerCursor<'a> {
    claims: Vec<BlockOwnerClaim<'a>>,
    index: usize,
}

impl<'a> BlockOwnerCursor<'a> {
    fn new(root: &'a AnnotatedContent) -> Self {
        let mut claims = Vec::new();
        collect_block_owner_claims(root, &mut claims);
        attach_semantic_owner_keys(&mut claims);
        Self { claims, index: 0 }
    }

    fn take_owner_for(&mut self, target: &Content) -> Option<&'a AnnotatedContent> {
        self.take_claim_for(target).owner
    }

    fn take_claim_for(&mut self, target: &Content) -> BlockOwnerMatch<'a> {
        while let Some(claim) = self.claims.get(self.index) {
            if owned_block_matches(&claim.content, target) {
                self.index += 1;
                return BlockOwnerMatch {
                    owner: claim.owner,
                    key: claim.key.clone(),
                };
            }
            if claim.owner.is_some_and(is_display_equation_owner) {
                break;
            }
            if !claim.content.plain_text().trim().is_empty() {
                break;
            }
            self.index += 1;
        }
        BlockOwnerMatch {
            owner: None,
            key: None,
        }
    }

    fn peek_claim_for(&self, target: &Content) -> BlockOwnerMatch<'a> {
        let mut index = self.index;
        while let Some(claim) = self.claims.get(index) {
            if owned_block_matches(&claim.content, target) {
                return BlockOwnerMatch {
                    owner: claim.owner,
                    key: claim.key.clone(),
                };
            }
            if claim.owner.is_some_and(is_display_equation_owner) {
                break;
            }
            if !claim.content.plain_text().trim().is_empty() {
                break;
            }
            index += 1;
        }
        BlockOwnerMatch {
            owner: None,
            key: None,
        }
    }
}

struct EquationOriginBlockClaim {
    content: Content,
    origins: Vec<Content>,
}

struct EquationOriginBlockCursor {
    claims: Vec<EquationOriginBlockClaim>,
    index: usize,
}

impl EquationOriginBlockCursor {
    fn new(root: &AnnotatedContent) -> Self {
        let mut claims = Vec::new();
        collect_equation_origin_block_claims(root, &mut claims);
        Self { claims, index: 0 }
    }

    fn take_for(&mut self, target: &Content) -> Vec<Content> {
        let mut deferred = Vec::new();
        let target_has_equation_carrier = realized_equation_carrier_count_for_diff(target) > 0;
        while let Some(claim) = self.claims.get(self.index) {
            if claim.content == *target {
                self.index += 1;
                deferred.extend(claim.origins.clone());
                return deferred;
            }
            if !claim.content.plain_text().trim().is_empty() {
                break;
            }
            if !claim.origins.is_empty() && !target_has_equation_carrier {
                break;
            }
            if target_has_equation_carrier {
                deferred.extend(claim.origins.clone());
            }
            self.index += 1;
        }
        deferred
    }
}

fn collect_equation_origin_block_claims(
    node: &AnnotatedContent,
    out: &mut Vec<EquationOriginBlockClaim>,
) {
    let subtree_origins = annotated_equation_origins(node);
    if !subtree_origins.is_empty() {
        let blocks = non_parbreak_blocks(&extract_block_units(&node.realized));
        if blocks.len() == 1 {
            out.push(EquationOriginBlockClaim {
                content: blocks[0].content.clone(),
                origins: subtree_origins,
            });
            return;
        }

        if node.children.is_empty() {
            let mut origins = subtree_origins
                .into_iter()
                .collect::<std::collections::VecDeque<_>>();
            for block in blocks {
                let count =
                    realized_equation_carrier_count_for_diff(&block.content).min(origins.len());
                let block_origins = (0..count)
                    .filter_map(|_| origins.pop_front())
                    .collect::<Vec<_>>();
                if !block_origins.is_empty() {
                    out.push(EquationOriginBlockClaim {
                        content: block.content,
                        origins: block_origins,
                    });
                }
            }
            return;
        }
    }

    for child in &node.children {
        collect_equation_origin_block_claims(child, out);
    }
}

fn annotated_equation_origins(root: &AnnotatedContent) -> Vec<Content> {
    let mut origins = Vec::new();
    collect_annotated_equation_origins(root, &mut origins);
    origins
}

fn collect_annotated_equation_origins(node: &AnnotatedContent, out: &mut Vec<Content>) {
    out.extend(node.annotation.equation_origins.iter().cloned());
    for child in &node.children {
        collect_annotated_equation_origins(child, out);
    }
}

fn realized_equation_carrier_count_for_diff(content: &Content) -> usize {
    if is_realized_equation_carrier(content) {
        return 1;
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .map(realized_equation_carrier_count_for_diff)
            .sum();
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return realized_equation_carrier_count_for_diff(&styled.child);
    }
    if let Some(par) = content.to_packed::<ParElem>() {
        return realized_equation_carrier_count_for_diff(&par.body);
    }
    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return realized_equation_carrier_count_for_diff(&heading.body);
    }
    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return realized_equation_carrier_count_for_diff(&body);
    }
    0
}

fn attach_semantic_owner_keys(claims: &mut [BlockOwnerClaim<'_>]) {
    let mut ordinals: HashMap<SemanticKind, usize> = HashMap::new();
    for claim in claims {
        let Some(owner) = claim.owner else {
            continue;
        };
        let Some(kind) = owner.annotation.semantic_kind.clone() else {
            continue;
        };
        let slot_labels = slot_labels_owned(owner);
        let ordinal = ordinals.entry(kind.clone()).or_default();
        claim.key = Some(SemanticOwnerKey {
            kind,
            slot_labels,
            ordinal: *ordinal,
        });
        *ordinal += 1;
    }
}

fn collect_block_owner_claims<'a>(node: &'a AnnotatedContent, out: &mut Vec<BlockOwnerClaim<'a>>) {
    let blocks = owner_block_units(node);
    if blocks.len() == 1 {
        let owner = if is_owned_diff_region(node)
            || has_equation_origins(node)
            || is_opaque_visual_owner(node)
        {
            Some(node)
        } else {
            unique_enclosed_diff_region_owner(node)
        };
        if owner.is_some() || node.children.is_empty() {
            out.push(BlockOwnerClaim {
                content: blocks[0].content.clone(),
                owner,
                key: None,
            });
            return;
        }
    }

    if node.children.is_empty() {
        out.extend(blocks.into_iter().map(|block| BlockOwnerClaim {
            content: block.content,
            owner: None,
            key: None,
        }));
    } else {
        for child in &node.children {
            collect_block_owner_claims(child, out);
        }
    }
}

fn owner_block_units(node: &AnnotatedContent) -> Vec<DiffBlock> {
    let realized_blocks = non_parbreak_blocks(&extract_block_units(&node.realized));
    if matches!(
        node.annotation.semantic_kind,
        Some(SemanticKind::Table | SemanticKind::Grid)
    ) && realized_blocks
        .iter()
        .all(|block| block.content.plain_text().trim().is_empty())
    {
        let effective = effective_render_content(node);
        let blocks = non_parbreak_blocks(&extract_block_units(&effective));
        let nonempty_blocks = blocks
            .iter()
            .filter(|block| !block.content.plain_text().trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if !nonempty_blocks.is_empty() {
            return nonempty_blocks;
        }
        if !blocks.is_empty() {
            return blocks;
        }
    }
    realized_blocks
}

fn is_owned_diff_region(node: &AnnotatedContent) -> bool {
    !node.annotation.slots.is_empty()
}

fn unique_enclosed_diff_region_owner(node: &AnnotatedContent) -> Option<&AnnotatedContent> {
    let mut owners = slot_bearing_descendants(node);
    (owners.len() == 1).then(|| owners.remove(0).1)
}

fn owned_block_matches(owner_block: &Content, target: &Content) -> bool {
    owner_block == target || normalized_visible_text_matches(owner_block, target)
}

fn semantic_owner_claims_match(
    old_claim: &BlockOwnerMatch<'_>,
    new_claim: &BlockOwnerMatch<'_>,
) -> bool {
    // Ownership noise may change slot shape or realized text, but not the
    // semantic owner kind and document-order identity.
    let (Some(old_key), Some(new_key)) = (&old_claim.key, &new_claim.key) else {
        return false;
    };
    old_claim.owner.is_some()
        && new_claim.owner.is_some()
        && old_key.kind == new_key.kind
        && old_key.ordinal == new_key.ordinal
}

fn layout_block_edit(block: &DiffBlock) -> DiffBlockEdit {
    DiffBlockEdit {
        base: annotated_block_from(&block.content, None),
        base_provenance: BlockBaseProvenance::Layout,
        edits: vec![],
        page_styles: block.page_styles.clone(),
    }
}

fn current_annotated_page_styles(blocks: &[DiffBlockEdit]) -> Styles {
    blocks
        .last()
        .map(|block| block.page_styles.clone())
        .unwrap_or_else(Styles::new)
}

fn should_defer_invisible_owner_edit(block: &Content, owner: &AnnotatedContent) -> bool {
    if !block.plain_text().trim().is_empty() {
        return false;
    }
    if !owner.annotation.slots.is_empty()
        && !effective_text_content(owner).plain_text().trim().is_empty()
    {
        return true;
    }
    if should_defer_invisible_equation_owner(block, owner) {
        return true;
    }
    if is_opaque_visual_owner(owner) {
        return true;
    }
    false
}

fn should_defer_invisible_equation_owner(block: &Content, owner: &AnnotatedContent) -> bool {
    block.plain_text().trim().is_empty()
        && owner.annotation.semantic_kind == Some(SemanticKind::Equation)
        && !owner.annotation.equation_origins.is_empty()
        && owner.annotation.equation_origins.iter().all(|origin| {
            origin
                .to_packed::<EquationElem>()
                .is_some_and(|equation| !equation.block.get(StyleChain::default()))
        })
}

fn is_display_equation_owner(owner: &AnnotatedContent) -> bool {
    owner.annotation.semantic_kind == Some(SemanticKind::Equation)
        && owner.annotation.equation_origins.iter().any(|origin| {
            origin
                .to_packed::<EquationElem>()
                .is_some_and(|equation| equation.block.get(StyleChain::default()))
        })
}

fn is_empty_realized_equation_owner(block: &Content, owner: &AnnotatedContent) -> bool {
    block.plain_text().trim().is_empty()
        && owner.annotation.semantic_kind == Some(SemanticKind::Equation)
        && !owner.annotation.equation_origins.is_empty()
}

fn live_display_equation_block(owner: &AnnotatedContent) -> Option<Content> {
    let origin = owner.annotation.equation_origins.first()?;
    let mut content = origin.clone();
    content
        .to_packed_mut::<EquationElem>()
        .expect("equation origin")
        .block
        .set(true);
    Some(content)
}

fn layout_content_matches(layout: &Content, target: &Content) -> bool {
    if layout == target {
        return true;
    }
    let layout_text = layout.plain_text();
    let target_text = target.plain_text();
    !layout_text.is_empty() && layout_text == target_text
}

fn normalized_visible_text_matches(left: &Content, right: &Content) -> bool {
    let left = normalized_visible_text(left);
    let right = normalized_visible_text(right);
    !left.is_empty() && left == right
}

fn normalized_visible_text(content: &Content) -> String {
    content
        .plain_text()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn annotated_block_from(content: &Content, source: Option<&AnnotatedContent>) -> AnnotatedContent {
    if let Some(source) = source {
        return source.clone();
    }
    AnnotatedContent {
        realized: content.clone(),
        annotation: Default::default(),
        children: vec![],
    }
}

/// Find the annotated descendant whose realized content matches `target`.
fn find_annotated_child<'a>(
    root: &'a AnnotatedContent,
    target: &Content,
) -> Option<&'a AnnotatedContent> {
    let mut exact = Vec::new();
    collect_annotated_matches(root, target, &mut exact);
    exact
        .into_iter()
        .max_by_key(|node| node.annotation.slots.len())
}

fn collect_annotated_matches<'a>(
    node: &'a AnnotatedContent,
    target: &Content,
    out: &mut Vec<&'a AnnotatedContent>,
) {
    if node.realized == *target {
        out.push(node);
    }
    for child in &node.children {
        collect_annotated_matches(child, target, out);
    }
}

fn find_single_block_semantic_owner<'a>(
    root: &'a AnnotatedContent,
    target: &Content,
) -> Option<&'a AnnotatedContent> {
    let mut owners = Vec::new();
    collect_single_block_semantic_owners(root, target, &mut owners);
    owners
        .into_iter()
        .max_by_key(|node| node.annotation.slots.len())
}

fn collect_single_block_semantic_owners<'a>(
    node: &'a AnnotatedContent,
    target: &Content,
    out: &mut Vec<&'a AnnotatedContent>,
) {
    if !node.annotation.slots.is_empty() {
        if matches!(
            node.annotation.semantic_kind,
            Some(SemanticKind::Table | SemanticKind::Grid)
        ) && normalized_visible_text_matches(&effective_render_content(node), target)
        {
            out.push(node);
        }
        let blocks = owner_block_units(node);
        if blocks.len() == 1 && owned_block_matches(&blocks[0].content, target) {
            out.push(node);
        }
    }
    for child in &node.children {
        collect_single_block_semantic_owners(child, target, out);
    }
}

fn find_annotated_block_owner<'a>(
    root: &'a AnnotatedContent,
    target: &Content,
) -> Option<&'a AnnotatedContent> {
    let exact = find_annotated_child(root, target);
    if exact.is_some_and(|node| !node.annotation.slots.is_empty()) {
        return exact;
    }
    find_single_block_semantic_owner(root, target).or(exact)
}

fn find_unique_changed_slot_pair<'a>(
    old: &'a AnnotatedContent,
    new: &'a AnnotatedContent,
) -> Option<(&'a AnnotatedContent, &'a AnnotatedContent)> {
    let old_nodes = slot_bearing_nodes(old);
    let new_nodes = slot_bearing_nodes(new);
    let mut pair = None;

    for old_node in old_nodes {
        for new_node in &new_nodes {
            if !can_recurse_via_slots(old_node, new_node) {
                continue;
            }
            if annotated_subtree_equal(old_node, new_node) {
                continue;
            }
            if diff_slot_edits(old_node, new_node).is_empty() {
                continue;
            }
            if pair.is_some() {
                return None;
            }
            pair = Some((old_node, *new_node));
        }
    }

    pair
}

fn slot_bearing_nodes(root: &AnnotatedContent) -> Vec<&AnnotatedContent> {
    let mut out = Vec::new();
    collect_slot_bearing_nodes(root, &mut out);
    out
}

fn collect_slot_bearing_nodes<'a>(node: &'a AnnotatedContent, out: &mut Vec<&'a AnnotatedContent>) {
    if !node.annotation.slots.is_empty() {
        out.push(node);
    }
    for child in &node.children {
        collect_slot_bearing_nodes(child, out);
    }
}

/// True when both nodes have the same slot-bearing semantic kind.
fn can_recurse_via_slots(old: &AnnotatedContent, new: &AnnotatedContent) -> bool {
    let Some(old_kind) = old.annotation.semantic_kind.as_ref() else {
        return false;
    };
    let Some(new_kind) = new.annotation.semantic_kind.as_ref() else {
        return false;
    };
    if old_kind != new_kind {
        return false;
    }
    if matches!(old_kind, SemanticKind::Equation) {
        return false;
    }
    !old.annotation.slots.is_empty() && !new.annotation.slots.is_empty()
}

struct SlotDescendantPair<'a> {
    old: &'a AnnotatedContent,
    new: &'a AnnotatedContent,
    new_path: Vec<usize>,
}

/// Find a unique matching slot-bearing descendant below a pair of wrapper/body nodes.
///
/// The walk stops at the first slot-bearing node on each branch, so a single
/// nested container can be diffed without accidentally jumping through it to a
/// deeper container. Multiple candidates are ambiguous and keep the existing
/// word-diff fallback.
fn find_slot_bearing_descendant_pair<'a>(
    old: &'a AnnotatedContent,
    new: &'a AnnotatedContent,
) -> Option<SlotDescendantPair<'a>> {
    let old_descendants = slot_bearing_descendants(old);
    let new_descendants = slot_bearing_descendants(new);
    if old_descendants.len() != 1 || new_descendants.len() != 1 {
        return None;
    }

    let (_old_path, old_descendant) = &old_descendants[0];
    let (new_path, new_descendant) = &new_descendants[0];
    can_recurse_via_slots(old_descendant, new_descendant).then(|| SlotDescendantPair {
        old: old_descendant,
        new: new_descendant,
        new_path: new_path.clone(),
    })
}

fn slot_bearing_descendants(node: &AnnotatedContent) -> Vec<(Vec<usize>, &AnnotatedContent)> {
    let mut out = Vec::new();
    for (index, child) in node.children.iter().enumerate() {
        let mut path = vec![index];
        collect_slot_bearing_descendants(child, &mut path, &mut out);
    }
    out
}

fn collect_slot_bearing_descendants<'a>(
    node: &'a AnnotatedContent,
    path: &mut Vec<usize>,
    out: &mut Vec<(Vec<usize>, &'a AnnotatedContent)>,
) {
    if !node.annotation.slots.is_empty() {
        out.push((path.clone(), node));
        return;
    }
    for (index, child) in node.children.iter().enumerate() {
        path.push(index);
        collect_slot_bearing_descendants(child, path, out);
        path.pop();
    }
}

fn recursive_slot_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    if can_recurse_via_slots(old_child, new_child) {
        let edits = diff_slot_edits(old_child, new_child);
        return (!edits.is_empty()).then(|| EditContent::Nested {
            base: new_child.clone(),
            edits,
        });
    }

    if let Some(pair) = find_slot_bearing_descendant_pair(old_child, new_child) {
        let edits = diff_slot_edits(pair.old, pair.new);
        if !edits.is_empty() {
            let content = EditContent::Nested {
                base: pair.new.clone(),
                edits,
            };
            return Some(EditContent::Nested {
                base: new_child.clone(),
                edits: vec![RealizedEdit::ReplaceAt {
                    path: pair.new_path,
                    content,
                }],
            });
        }
    }

    unique_changed_child_edit_content(old_child, new_child)
}

fn unique_changed_child_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    if old_child.children.is_empty() || new_child.children.is_empty() {
        return None;
    }
    if slot_bearing_descendants(old_child).is_empty()
        && slot_bearing_descendants(new_child).is_empty()
    {
        return None;
    }
    if old_child.children.len() != new_child.children.len() {
        return child_sequence_structural_edit_content(old_child, new_child);
    }

    let mut changed = None;
    for (index, (old_grandchild, new_grandchild)) in old_child
        .children
        .iter()
        .zip(new_child.children.iter())
        .enumerate()
    {
        if annotated_subtree_equal(old_grandchild, new_grandchild) {
            continue;
        }
        if changed.is_some() {
            return None;
        }
        changed = Some((index, old_grandchild, new_grandchild));
    }

    let (index, old_grandchild, new_grandchild) = changed?;
    let content = recursive_slot_edit_content(old_grandchild, new_grandchild)
        .or_else(|| modified_edit_content(old_grandchild, new_grandchild))?;
    Some(EditContent::Nested {
        base: new_child.clone(),
        edits: vec![RealizedEdit::ReplaceAt {
            path: vec![index],
            content,
        }],
    })
}

fn child_sequence_structural_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    let old_children = meaningful_child_sequence(old_child);
    let new_children = meaningful_child_sequence(new_child);
    if old_children.is_empty() || new_children.is_empty() {
        return None;
    }

    let old_keys: Vec<String> = old_children
        .iter()
        .map(|(_, child)| structural_child_key(child))
        .collect();
    let new_keys: Vec<String> = new_children
        .iter()
        .map(|(_, child)| structural_child_key(child))
        .collect();
    let ops = capture_diff_slices(Algorithm::Myers, &old_keys, &new_keys);

    let mut edits = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    let old_grandchild = old_children[old_index + i].1;
                    let new_grandchild = new_children[new_index + i].1;
                    if annotated_subtree_equal(old_grandchild, new_grandchild) {
                        continue;
                    }
                    let content = recursive_slot_edit_content(old_grandchild, new_grandchild)
                        .or_else(|| modified_edit_content(old_grandchild, new_grandchild))?;
                    edits.push(RealizedEdit::ReplaceAt {
                        path: vec![new_children[new_index + i].0],
                        content,
                    });
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => {
                for i in 0..old_len {
                    push_deleted_child_sequence_edit(
                        &mut edits,
                        old_children[old_index + i].1,
                        &new_children,
                        new_index,
                    );
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    let (path_index, child) = new_children[new_index + i];
                    edits.push(RealizedEdit::ReplaceAt {
                        path: vec![path_index],
                        content: EditContent::Inserted(effective_render_content(child)),
                    });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    let old_grandchild = old_children[old_index + i].1;
                    let new_grandchild = new_children[new_index + i].1;
                    let content = recursive_slot_edit_content(old_grandchild, new_grandchild)
                        .or_else(|| modified_edit_content(old_grandchild, new_grandchild))?;
                    edits.push(RealizedEdit::ReplaceAt {
                        path: vec![new_children[new_index + i].0],
                        content,
                    });
                }
                for i in paired..old_len {
                    push_deleted_child_sequence_edit(
                        &mut edits,
                        old_children[old_index + i].1,
                        &new_children,
                        new_index + paired,
                    );
                }
                for i in paired..new_len {
                    let (path_index, child) = new_children[new_index + i];
                    edits.push(RealizedEdit::ReplaceAt {
                        path: vec![path_index],
                        content: EditContent::Inserted(effective_render_content(child)),
                    });
                }
            }
        }
    }

    (!edits.is_empty()).then(|| EditContent::Nested {
        base: new_child.clone(),
        edits,
    })
}

fn meaningful_child_sequence(node: &AnnotatedContent) -> Vec<(usize, &AnnotatedContent)> {
    node.children
        .iter()
        .enumerate()
        .filter(|(_, child)| !is_structural_separator_child(child))
        .collect()
}

fn is_structural_separator_child(child: &AnnotatedContent) -> bool {
    child.annotation.slots.is_empty()
        && slot_bearing_descendants(child).is_empty()
        && effective_render_content(child)
            .plain_text()
            .trim()
            .is_empty()
}

fn structural_child_key(child: &AnnotatedContent) -> String {
    format!(
        "{:?}:{}:{}",
        child.annotation.semantic_kind,
        effective_text_content(child).plain_text(),
        presentation_key(&effective_render_content(child))
    )
}

fn push_deleted_child_sequence_edit(
    edits: &mut Vec<RealizedEdit>,
    old_child: &AnnotatedContent,
    new_children: &[(usize, &AnnotatedContent)],
    new_index: usize,
) {
    let content = deleted_edit(effective_text_content(old_child));
    if let Some((path_index, _)) = new_children.get(new_index) {
        edits.push(RealizedEdit::InsertBefore {
            anchor: vec![*path_index],
            content,
        });
    } else if new_index > 0 {
        if let Some((path_index, _)) = new_children.get(new_index - 1) {
            edits.push(RealizedEdit::InsertAfter {
                anchor: vec![*path_index],
                content,
            });
        } else {
            edits.push(RealizedEdit::Append { content });
        }
    } else {
        edits.push(RealizedEdit::Append { content });
    }
}

fn owned_surface_modified_edit(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Option<RealizedEdit> {
    let old_effective = effective_text_content(old_ann);
    let new_effective = effective_text_content(new_ann);
    if old_effective == new_effective {
        return None;
    }

    modified_fragment_edit_content(
        &old_effective,
        &new_effective,
        Some(old_ann),
        Some(new_ann),
        &[],
        &[],
    )
    .map(RealizedEdit::WholeBlock)
}

fn slot_labels(node: &AnnotatedContent) -> Vec<&SlotStep> {
    node.annotation
        .slots
        .iter()
        .map(|slot| &slot.label)
        .collect()
}

fn slot_labels_owned(node: &AnnotatedContent) -> Vec<SlotStep> {
    node.annotation
        .slots
        .iter()
        .map(|slot| slot.label.clone())
        .collect()
}

fn owner_already_recursed(
    recursed: &HashSet<SemanticOwnerKey>,
    key: &Option<SemanticOwnerKey>,
) -> bool {
    key.as_ref().is_some_and(|key| recursed.contains(key))
}

fn mark_recursed_owner(recursed: &mut HashSet<SemanticOwnerKey>, key: &Option<SemanticOwnerKey>) {
    if let Some(key) = key {
        recursed.insert(key.clone());
    }
}

fn mark_recursed_display_surface(recursed: &mut Vec<Content>, owner: &AnnotatedContent) {
    let surface = effective_render_content(owner);
    if is_display_equation_owner(owner) {
        recursed.push(surface.clone());
    }
    if matches!(
        owner.annotation.semantic_kind,
        Some(SemanticKind::Table | SemanticKind::Grid)
    ) && !surface.plain_text().trim().is_empty()
    {
        recursed.push(surface.clone());
    }
    if contains_non_token_display_container(&surface) {
        recursed.extend(
            non_parbreak_blocks(&extract_block_units(&surface))
                .into_iter()
                .map(|block| block.content),
        );
    }
}

fn consume_recursed_display_surface(recursed: &mut Vec<Content>, content: &Content) -> bool {
    if !contains_non_token_display_container(content) && normalized_visible_text(content).is_empty()
    {
        return false;
    }
    let Some(index) = recursed
        .iter()
        .position(|surface| normalized_visible_text_matches(surface, content))
    else {
        return false;
    };
    recursed.remove(index);
    true
}

fn resolved_slots(node: &AnnotatedContent) -> Vec<(&SemanticSlot, &AnnotatedContent)> {
    node.annotation
        .slots
        .iter()
        .filter_map(|slot| node.get_path(&slot.path).map(|child| (slot, child)))
        .collect()
}

fn annotated_subtree_equal(old: &AnnotatedContent, new: &AnnotatedContent) -> bool {
    old.annotation.semantic_kind == new.annotation.semantic_kind
        && slot_labels(old) == slot_labels(new)
        && effective_render_content(old) == effective_render_content(new)
        && old.children.len() == new.children.len()
        && old
            .children
            .iter()
            .zip(new.children.iter())
            .all(|(old_child, new_child)| annotated_subtree_equal(old_child, new_child))
}

fn diff_slot_edits_same_shape(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Vec<RealizedEdit> {
    let old_slots = resolved_slots(old_ann);
    let new_slots = resolved_slots(new_ann);
    let mut edits = Vec::new();

    for ((_old_slot, old_child), (new_slot, new_child)) in old_slots.into_iter().zip(new_slots) {
        if annotated_subtree_equal(old_child, new_child) {
            continue;
        }

        if let Some(content) = recursive_slot_edit_content(old_child, new_child) {
            edits.push(RealizedEdit::ReplaceAt {
                path: new_slot.path.clone(),
                content,
            });
        } else {
            push_modified_slot_edit(&mut edits, old_child, new_child, new_slot);
        }
    }

    edits
}

fn push_modified_slot_edit(
    edits: &mut Vec<RealizedEdit>,
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
    slot: &SemanticSlot,
) {
    if let Some(content) = modified_slot_edit_content(old_child, new_child, &slot.label) {
        edits.push(RealizedEdit::ReplaceAt {
            path: slot.path.clone(),
            content,
        });
    }
}

fn modified_slot_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
    slot: &SlotStep,
) -> Option<EditContent> {
    modified_edit_content(old_child, new_child)
        .or_else(|| opaque_visual_slot_edit_content(old_child, new_child, slot))
}

fn opaque_visual_slot_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
    slot: &SlotStep,
) -> Option<EditContent> {
    if !matches!(slot, SlotStep::FigureBody) {
        return None;
    }
    let old_surface = effective_render_content(old_child);
    let new_surface = effective_render_content(new_child);
    opaque_visual_surface_changed(&old_surface, &new_surface, Some(old_child), Some(new_child))
        .then(|| EditContent::OpaqueReplacement {
            old: OldDisplaySurface::new(old_surface),
            new: new_surface,
        })
}

fn modified_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    let old_effective = effective_text_content(old_child);
    let new_effective = effective_text_content(new_child);
    modified_fragment_edit_content(
        &old_effective,
        &new_effective,
        Some(old_child),
        Some(new_child),
        &[],
        &[],
    )
}

#[derive(Clone)]
struct VisibleFootnoteUnit {
    text: String,
    content: Content,
    path: Option<Vec<usize>>,
    is_footnote: bool,
}

fn footnote_visible_text_edits(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Vec<RealizedEdit> {
    if !all_slots_are_footnote_bodies(old_ann) || !all_slots_are_footnote_bodies(new_ann) {
        return vec![];
    }

    let Some(old_units) = visible_footnote_units(old_ann) else {
        return vec![];
    };
    let Some(new_units) = visible_footnote_units(new_ann) else {
        return vec![];
    };
    let old_keys = old_units
        .iter()
        .map(visible_footnote_unit_key)
        .collect::<Vec<_>>();
    let new_keys = new_units
        .iter()
        .map(visible_footnote_unit_key)
        .collect::<Vec<_>>();
    let ops = capture_diff_slices(Algorithm::Myers, &old_keys, &new_keys);

    let mut edits = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for unit in &new_units[new_index..new_index + new_len] {
                    push_visible_insert_edit(&mut edits, unit);
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => {
                for unit in &old_units[old_index..old_index + old_len] {
                    push_visible_delete_edit(&mut edits, unit, &new_units, new_index);
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    let old_unit = &old_units[old_index + i];
                    let new_unit = &new_units[new_index + i];
                    if old_unit.is_footnote || new_unit.is_footnote {
                        continue;
                    }
                    if let Some(path) = &new_unit.path {
                        if let Some(content) = modified_fragment_edit_content(
                            &old_unit.content,
                            &new_unit.content,
                            None,
                            None,
                            &[],
                            &[],
                        ) {
                            edits.push(RealizedEdit::ReplaceAt {
                                path: path.clone(),
                                content,
                            });
                        }
                    }
                }
                for unit in &old_units[old_index + paired..old_index + old_len] {
                    push_visible_delete_edit(&mut edits, unit, &new_units, new_index + paired);
                }
                for unit in &new_units[new_index + paired..new_index + new_len] {
                    push_visible_insert_edit(&mut edits, unit);
                }
            }
        }
    }

    edits
}

fn visible_footnote_unit_key(unit: &VisibleFootnoteUnit) -> String {
    if unit.is_footnote {
        format!("footnote:{}", unit.text)
    } else {
        format!("visible:{}:{}", unit.text, presentation_key(&unit.content))
    }
}

fn push_visible_insert_edit(edits: &mut Vec<RealizedEdit>, unit: &VisibleFootnoteUnit) {
    if unit.is_footnote || unit.text.trim().is_empty() {
        return;
    }
    let Some(path) = &unit.path else {
        return;
    };
    let tokens = extract_words(&unit.content);
    if tokens.is_empty() {
        return;
    }
    edits.push(RealizedEdit::ReplaceAt {
        path: path.clone(),
        content: EditContent::Modified {
            base: unit.content.clone(),
            word_ops: vec![WordOp::Insert(tokens)],
        },
    });
}

fn push_visible_delete_edit(
    edits: &mut Vec<RealizedEdit>,
    unit: &VisibleFootnoteUnit,
    new_units: &[VisibleFootnoteUnit],
    new_index: usize,
) {
    if unit.is_footnote || unit.text.trim().is_empty() {
        return;
    }
    let content = deleted_edit(unit.content.clone());
    if let Some(anchor) = new_units
        .get(new_index)
        .and_then(|unit| unit.path.as_ref())
        .cloned()
    {
        edits.push(RealizedEdit::InsertBefore { anchor, content });
    } else if new_index > 0 {
        if let Some(anchor) = new_units
            .get(new_index - 1)
            .and_then(|unit| unit.path.as_ref())
            .cloned()
        {
            edits.push(RealizedEdit::InsertAfter { anchor, content });
        } else {
            edits.push(RealizedEdit::Append { content });
        }
    } else {
        edits.push(RealizedEdit::Append { content });
    }
}

fn visible_footnote_units(node: &AnnotatedContent) -> Option<Vec<VisibleFootnoteUnit>> {
    let surface = node
        .annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized);
    let body_path = paragraph_body_path(surface)?;
    let body = content_at_path(surface, &body_path)?;
    let seq = body.to_packed::<SequenceElem>()?;
    let mut footnote_number = 1;
    let mut out = Vec::new();
    for (index, child) in seq.children.iter().enumerate() {
        let mut path = body_path.clone();
        path.push(index);
        if child.is::<FootnoteElem>() {
            out.push(VisibleFootnoteUnit {
                text: footnote_number.to_string(),
                content: child.clone(),
                path: Some(path),
                is_footnote: true,
            });
            footnote_number += 1;
        } else {
            out.push(VisibleFootnoteUnit {
                text: child.plain_text().to_string(),
                content: child.clone(),
                path: Some(path),
                is_footnote: false,
            });
        }
    }
    Some(out)
}

fn paragraph_body_path(content: &Content) -> Option<Vec<usize>> {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let mut path = paragraph_body_path(&styled.child)?;
        path.insert(0, 0);
        return Some(path);
    }
    content.is::<ParElem>().then_some(vec![0])
}

fn content_at_path<'a>(content: &'a Content, path: &[usize]) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return Some(content.clone());
    };
    let child = container_ops::realized_child_contents(content)
        .get(*index)
        .cloned()?;
    content_at_path(&child, rest)
}

fn deleted_visible_text_before_first_footnote(
    old_content: &Content,
    new_ann: &AnnotatedContent,
) -> Vec<RealizedEdit> {
    let Some(anchor) = first_footnote_call_path(new_ann) else {
        return vec![];
    };
    let new_visible = footnote_owner_content_without_footnotes(new_ann);
    let old_tokens = extract_words(old_content);
    let new_tokens = extract_words(&new_visible);
    let mut deleted = diff_words(&old_tokens, &new_tokens)
        .into_iter()
        .filter_map(|op| match op {
            WordOp::Delete(tokens) => Some(tokens),
            WordOp::Equal(_) | WordOp::Insert(_) => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    trim_whitespace_tokens(&mut deleted);
    if deleted.is_empty() {
        return vec![];
    }
    let content = Content::sequence(deleted.iter().map(token_content_for_direct_edit));

    vec![RealizedEdit::InsertBefore {
        anchor,
        content: deleted_edit(content),
    }]
}

fn trim_whitespace_tokens(tokens: &mut Vec<Token>) {
    while tokens
        .first()
        .is_some_and(|token| token.text.chars().all(char::is_whitespace))
    {
        tokens.remove(0);
    }
    while tokens
        .last()
        .is_some_and(|token| token.text.chars().all(char::is_whitespace))
    {
        tokens.pop();
    }
}

fn footnote_owner_content_without_footnotes(node: &AnnotatedContent) -> Content {
    let surface = node
        .annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized);
    remove_footnote_calls(surface)
}

fn remove_footnote_calls(content: &Content) -> Content {
    if content.is::<FootnoteElem>() {
        return Content::sequence([]);
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return Content::sequence(seq.children.iter().map(remove_footnote_calls));
    }
    if let Some(children) = maybe_map_realized_children(content, remove_footnote_calls) {
        return children;
    }
    content.clone()
}

fn maybe_map_realized_children(
    content: &Content,
    mut map_child: impl FnMut(&Content) -> Content,
) -> Option<Content> {
    let children = container_ops::realized_child_contents(content);
    if children.is_empty() {
        return None;
    }
    let mut mapped = content.clone();
    for (index, child) in children.iter().enumerate() {
        mapped = container_ops::replace_realized_child(&mapped, index, map_child(child))?;
    }
    Some(mapped)
}

fn diff_slot_edits(old_ann: &AnnotatedContent, new_ann: &AnnotatedContent) -> Vec<RealizedEdit> {
    if all_slots_are_footnote_bodies(old_ann) && all_slots_are_footnote_bodies(new_ann) {
        return diff_footnote_body_slot_edits(old_ann, new_ann);
    }
    if slot_labels(old_ann) == slot_labels(new_ann) {
        diff_slot_edits_same_shape(old_ann, new_ann)
    } else {
        diff_slot_edits_lcs(old_ann, new_ann)
    }
}

fn all_slots_are_footnote_bodies(node: &AnnotatedContent) -> bool {
    !node.annotation.slots.is_empty()
        && node
            .annotation
            .slots
            .iter()
            .all(|slot| matches!(slot.label, SlotStep::FootnoteBody))
}

fn diff_footnote_body_slot_edits(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Vec<RealizedEdit> {
    let old_slots = resolved_slots(old_ann);
    let new_slots = resolved_slots(new_ann);

    let mut edits = Vec::new();

    if old_slots.len() == 1 && new_slots.len() == 1 {
        let old_child = old_slots[0].1;
        let (new_slot, new_child) = new_slots[0];
        if !annotated_subtree_equal(old_child, new_child)
            && let Some(content) = recursive_slot_edit_content(old_child, new_child)
                .or_else(|| modified_edit_content(old_child, new_child))
        {
            edits.push(RealizedEdit::ReplaceAt {
                path: new_slot.path.clone(),
                content,
            });
        }
        return edits;
    }

    for (new_slot, new_child) in &new_slots {
        let body = effective_text_content(new_child);
        edits.push(RealizedEdit::ReplaceAt {
            path: new_slot.path.clone(),
            content: inserted_footnote_body_edit_content(body),
        });
    }

    if let Some(edit) = deleted_footnote_call_edit(&old_slots, &new_slots) {
        edits.push(edit);
    }

    edits
}

fn inserted_footnote_body_edits(new_ann: &AnnotatedContent) -> Vec<RealizedEdit> {
    resolved_slots(new_ann)
        .into_iter()
        .filter(|(slot, _)| matches!(slot.label, SlotStep::FootnoteBody))
        .filter_map(|(slot, child)| {
            let body = effective_text_content(child);
            let tokens = extract_words(&body);
            (!tokens.is_empty()).then(|| RealizedEdit::ReplaceAt {
                path: slot.path.clone(),
                content: inserted_footnote_body_edit_content_with_tokens(body, tokens),
            })
        })
        .collect()
}

fn inserted_footnote_body_edit_content(body: Content) -> EditContent {
    let tokens = extract_words(&body);
    inserted_footnote_body_edit_content_with_tokens(body, tokens)
}

fn inserted_footnote_body_edit_content_with_tokens(
    body: Content,
    tokens: Vec<Token>,
) -> EditContent {
    EditContent::Modified {
        base: body,
        word_ops: vec![WordOp::Insert(tokens)],
    }
}

fn deleted_footnote_call_edit(
    old_slots: &[(&SemanticSlot, &AnnotatedContent)],
    new_slots: &[(&SemanticSlot, &AnnotatedContent)],
) -> Option<RealizedEdit> {
    let calls = old_slots
        .iter()
        .map(|(_slot, child)| {
            Content::new(FootnoteElem::new(FootnoteBody::Content(
                retain_old_display_content(&effective_text_content(child)),
            )))
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return None;
    }

    let content = if calls.len() == 1 {
        let call = calls.into_iter().next().expect("single deleted footnote");
        let tokens = old_display_tokens(extract_words(&call));
        EditContent::Modified {
            base: call,
            word_ops: vec![WordOp::Delete(tokens)],
        }
    } else {
        EditContent::Deleted(OldDisplaySurface::new(Content::sequence(calls)))
    };
    if let Some(anchor) = new_slots
        .iter()
        .rev()
        .find_map(|(slot, _child)| footnote_call_path(slot))
    {
        Some(RealizedEdit::InsertAfter { anchor, content })
    } else {
        Some(RealizedEdit::Append { content })
    }
}

fn first_footnote_call_path(node: &AnnotatedContent) -> Option<Vec<usize>> {
    node.annotation.slots.iter().find_map(footnote_call_path)
}

fn footnote_call_path(slot: &SemanticSlot) -> Option<Vec<usize>> {
    if !matches!(slot.label, SlotStep::FootnoteBody) {
        return None;
    }
    let (last, parent) = slot.path.split_last()?;
    (*last == 0).then(|| parent.to_vec())
}

fn diff_slot_edits_with_events(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<Vec<RealizedEdit>> {
    let mode = if slot_labels(old_ann) == slot_labels(new_ann) {
        "same_shape"
    } else {
        "lcs"
    };
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/slot", "start")
            .reason(mode)
            .old_content(&old_ann.realized)
            .new_content(&new_ann.realized),
    )?;
    let edits = diff_slot_edits(old_ann, new_ann);
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("diff/slot", "end")
            .reason(format!("mode={mode} edits={}", edits.len()))
            .selected_edit_kind(if edits.is_empty() {
                "noop".to_string()
            } else {
                edits
                    .iter()
                    .map(realized_edit_kind)
                    .collect::<Vec<_>>()
                    .join(",")
            }),
    )?;
    Ok(edits)
}

fn diff_slot_edits_lcs(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Vec<RealizedEdit> {
    let old_slots = resolved_slots(old_ann);
    let new_slots = resolved_slots(new_ann);
    let old_h: Vec<String> = old_ann
        .annotation
        .slots
        .iter()
        .filter_map(|slot| old_ann.get_path(&slot.path))
        .map(slot_child_match_key)
        .collect();
    let new_h: Vec<String> = new_ann
        .annotation
        .slots
        .iter()
        .filter_map(|slot| new_ann.get_path(&slot.path))
        .map(slot_child_match_key)
        .collect();
    let ops = capture_diff_slices(Algorithm::Myers, &old_h, &new_h);

    let mut edits = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    let old_child = old_slots[old_index + i].1;
                    let new_child = new_slots[new_index + i].1;
                    if annotated_subtree_equal(old_child, new_child) {
                        continue;
                    }
                    if let Some(content) = recursive_slot_edit_content(old_child, new_child) {
                        edits.push(RealizedEdit::ReplaceAt {
                            path: new_slots[new_index + i].0.path.clone(),
                            content,
                        });
                    } else {
                        push_modified_slot_edit(
                            &mut edits,
                            old_child,
                            new_child,
                            new_slots[new_index + i].0,
                        );
                    }
                }
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => {
                for i in 0..old_len {
                    push_deleted_slot_edit(
                        &mut edits,
                        old_slots[old_index + i].1,
                        &new_slots,
                        new_index,
                    );
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    let (slot, child) = new_slots[new_index + i];
                    edits.push(RealizedEdit::ReplaceAt {
                        path: slot.path.clone(),
                        content: EditContent::Inserted(effective_text_content(child)),
                    });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                for i in 0..paired {
                    let old_child = old_slots[old_index + i].1;
                    let new_child = new_slots[new_index + i].1;
                    if let Some(content) = recursive_slot_edit_content(old_child, new_child) {
                        edits.push(RealizedEdit::ReplaceAt {
                            path: new_slots[new_index + i].0.path.clone(),
                            content,
                        });
                    } else {
                        push_modified_slot_edit(
                            &mut edits,
                            old_child,
                            new_child,
                            new_slots[new_index + i].0,
                        );
                    }
                }
                for i in paired..old_len {
                    push_deleted_slot_edit(
                        &mut edits,
                        old_slots[old_index + i].1,
                        &new_slots,
                        new_index + paired,
                    );
                }
                for i in paired..new_len {
                    let (slot, child) = new_slots[new_index + i];
                    edits.push(RealizedEdit::ReplaceAt {
                        path: slot.path.clone(),
                        content: EditContent::Inserted(effective_text_content(child)),
                    });
                }
            }
        }
    }
    edits
}

fn slot_child_match_key(child: &AnnotatedContent) -> String {
    format!(
        "{}:{}",
        effective_text_content(child).plain_text(),
        presentation_key(&effective_render_content(child))
    )
}

fn push_deleted_slot_edit(
    edits: &mut Vec<RealizedEdit>,
    old_child: &AnnotatedContent,
    new_slots: &[(&SemanticSlot, &AnnotatedContent)],
    new_index: usize,
) {
    let content = deleted_edit(effective_render_content(old_child));
    if let Some((slot, _)) = new_slots.get(new_index) {
        edits.push(RealizedEdit::InsertBefore {
            anchor: slot.path.clone(),
            content,
        });
    } else if new_index > 0 {
        if let Some((slot, _)) = new_slots.get(new_index - 1) {
            edits.push(RealizedEdit::InsertAfter {
                anchor: slot.path.clone(),
                content,
            });
        } else {
            edits.push(RealizedEdit::Append { content });
        }
    } else {
        edits.push(RealizedEdit::Append { content });
    }
}

fn deleted_edit(content: Content) -> EditContent {
    EditContent::Deleted(old_display_surface(&content))
}

fn deleted_edit_for_annotated(
    content: &Content,
    annotated: Option<&AnnotatedContent>,
) -> EditContent {
    EditContent::Deleted(old_display_surface_for_annotated(content, annotated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::model::HeadingElem;
    use typst::text::TextElem;

    fn seq(items: impl IntoIterator<Item = Content>) -> Content {
        Content::sequence(items)
    }

    fn annotated(content: &Content) -> AnnotatedContent {
        crate::annotated::annotate_realized(content, content)
    }

    fn contains_style_for<T: NativeElement>(content: &Content) -> bool {
        let mut found = false;
        let _ = content.traverse::<_, ()>(&mut |node| {
            if let Some(styled) = node.to_packed::<StyledElem>()
                && styled
                    .styles
                    .iter()
                    .any(|style| style.element().is_some_and(|element| element == T::ELEM))
            {
                found = true;
            }
            std::ops::ControlFlow::Continue(())
        });
        found
    }

    fn whole_block_modified_ops(result: &DiffResult) -> Option<&[WordOp]> {
        for block in &result.blocks {
            for edit in &block.edits {
                if let RealizedEdit::WholeBlock(EditContent::Modified { word_ops, .. }) = edit {
                    return Some(word_ops);
                }
            }
        }
        None
    }

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
        assert!(result.blocks.iter().all(|block| block.edits.is_empty()));
    }

    #[test]
    fn find_annotated_child_returns_child_with_matching_realized() {
        use crate::annotated::{AnnotatedContent, Annotation};

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
        use crate::annotated::{AnnotatedContent, Annotation};

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
    fn find_annotated_child_does_not_match_empty_text_fallback() {
        use crate::annotated::{
            AnnotatedContent, Annotation, SemanticKind, SemanticSlot, SlotStep, WrapperKind,
        };

        let target = Content::sequence([]);
        let root = AnnotatedContent {
            realized: TextElem::packed("root"),
            annotation: Annotation::default(),
            children: vec![AnnotatedContent {
                realized: TextElem::packed(""),
                annotation: Annotation {
                    semantic_kind: Some(SemanticKind::Wrapper(WrapperKind::Pad)),
                    slots: vec![SemanticSlot {
                        label: SlotStep::WrapperBody,
                        path: vec![0],
                        patch_path: None,
                    }],
                    ..Annotation::default()
                },
                children: vec![AnnotatedContent {
                    realized: TextElem::packed("body"),
                    annotation: Annotation::default(),
                    children: vec![],
                }],
            }],
        };

        assert!(find_annotated_child(&root, &target).is_none());
    }

    #[test]
    fn can_recurse_via_slots_true_for_matching_list_kinds() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make = |kind: SemanticKind| AnnotatedContent {
            realized: TextElem::packed("x"),
            annotation: Annotation {
                semantic_kind: Some(kind),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
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
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let eq = AnnotatedContent {
            realized: TextElem::packed("x"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Equation),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![],
        };
        assert!(!can_recurse_via_slots(&eq, &eq));
    }

    #[test]
    fn diff_slot_edits_same_shape_marks_changed_item_modified() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

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
                    .map(|(i, _)| SemanticSlot {
                        label: SlotStep::ListItem(i),
                        path: vec![i],
                        patch_path: None,
                    })
                    .collect(),
                ..Annotation::default()
            },
            children: texts.iter().map(|text| make_child(text)).collect(),
        };

        let old = make_list(&["Item A", "Old item", "Item C"]);
        let new = make_list(&["Item A", "New item", "Item C"]);
        let result = diff_slot_edits_same_shape(&old, &new);
        assert_eq!(result.len(), 1);
        assert!(matches!(
            result[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![1]
        ));
    }

    #[test]
    fn diff_slot_edits_same_shape_all_unchanged_when_equal() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make_list = |texts: &[&str]| AnnotatedContent {
            realized: TextElem::packed("list"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: texts
                    .iter()
                    .enumerate()
                    .map(|(i, _)| SemanticSlot {
                        label: SlotStep::ListItem(i),
                        path: vec![i],
                        patch_path: None,
                    })
                    .collect(),
                ..Annotation::default()
            },
            children: texts
                .iter()
                .map(|text| AnnotatedContent {
                    realized: TextElem::packed(*text),
                    annotation: Annotation::default(),
                    children: vec![],
                })
                .collect(),
        };

        let list = make_list(&["A", "B", "C"]);
        let result = diff_slot_edits_same_shape(&list, &list);
        assert!(result.is_empty());
    }

    #[test]
    fn identical_non_leaf_subtree_emits_no_edits() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let inner_list = AnnotatedContent {
            realized: TextElem::packed(""),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed("Nested item"),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };
        let parent = AnnotatedContent {
            realized: TextElem::packed("parent"),
            annotation: Annotation::default(),
            children: vec![inner_list],
        };
        let container = AnnotatedContent {
            realized: TextElem::packed("outer"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![parent],
        };

        let result = diff_slot_edits_same_shape(&container, &container);
        assert!(result.is_empty());
    }

    #[test]
    fn diff_slot_edits_same_shape_recurses_into_nested_descendant() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make_inner_list = |item_text: &str| AnnotatedContent {
            realized: TextElem::packed(item_text),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed(item_text),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };

        let make_item_body = |inner_text: &str, body_id: &str| AnnotatedContent {
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
                    SemanticSlot {
                        label: SlotStep::ListItem(0),
                        path: vec![0],
                        patch_path: None,
                    },
                    SemanticSlot {
                        label: SlotStep::ListItem(1),
                        path: vec![1],
                        patch_path: None,
                    },
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

        let result = diff_slot_edits_same_shape(&outer_old, &outer_new);
        assert_eq!(result.len(), 1);
        let RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Nested { edits, .. },
        } = &result[0]
        else {
            panic!("expected outer slot to contain a nested descendant edit");
        };
        assert_eq!(path, &vec![0]);

        let RealizedEdit::ReplaceAt {
            path: inner_container_path,
            content: EditContent::Nested {
                edits: inner_edits, ..
            },
        } = &edits[0]
        else {
            panic!("expected nested slot-bearing descendant edit");
        };
        assert_eq!(inner_container_path, &vec![1]);
        assert!(matches!(
            inner_edits[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![0]
        ));
    }

    #[test]
    fn diff_slot_edits_same_shape_recurses_to_arbitrary_depth() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make_leaf_list = |text: &str| AnnotatedContent {
            realized: TextElem::packed(text),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed(text),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };
        let make_wrapped_leaf = |text: &str| AnnotatedContent {
            realized: TextElem::packed("wrapper"),
            annotation: Annotation::default(),
            children: vec![AnnotatedContent {
                realized: TextElem::packed("inner-wrapper"),
                annotation: Annotation::default(),
                children: vec![make_leaf_list(text)],
            }],
        };
        let make_outer = |text: &str| AnnotatedContent {
            realized: TextElem::packed("outer"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::List),
                slots: vec![SemanticSlot {
                    label: SlotStep::ListItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![make_wrapped_leaf(text)],
        };

        let result = diff_slot_edits_same_shape(&make_outer("Old"), &make_outer("New"));
        let RealizedEdit::ReplaceAt {
            content: EditContent::Nested { edits, .. },
            ..
        } = &result[0]
        else {
            panic!("expected nested edit through wrappers");
        };
        let RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Nested {
                edits: inner_edits, ..
            },
        } = &edits[0]
        else {
            panic!("expected nested slot-bearing descendant edit");
        };

        assert_eq!(path, &vec![0, 0]);
        assert!(matches!(
            inner_edits[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![0]
        ));
    }

    #[test]
    fn diff_slot_edits_same_shape_recurses_inside_non_list_container_slot() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make_cell = |text: &str| AnnotatedContent {
            realized: TextElem::packed("cell"),
            annotation: Annotation::default(),
            children: vec![AnnotatedContent {
                realized: TextElem::packed(text),
                annotation: Annotation {
                    semantic_kind: Some(SemanticKind::List),
                    slots: vec![SemanticSlot {
                        label: SlotStep::ListItem(0),
                        path: vec![0],
                        patch_path: None,
                    }],
                    ..Annotation::default()
                },
                children: vec![AnnotatedContent {
                    realized: TextElem::packed(text),
                    annotation: Annotation::default(),
                    children: vec![],
                }],
            }],
        };
        let make_table = |text: &str| AnnotatedContent {
            realized: TextElem::packed("table"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Table),
                slots: vec![SemanticSlot {
                    label: SlotStep::TableCell(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![make_cell(text)],
        };

        let result = diff_slot_edits_same_shape(&make_table("Old"), &make_table("New"));
        let RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Nested { edits, .. },
        } = &result[0]
        else {
            panic!("expected table cell to contain nested descendant edit");
        };

        assert_eq!(path, &vec![0]);
        let RealizedEdit::ReplaceAt {
            path: nested_path,
            content: EditContent::Nested {
                edits: inner_edits, ..
            },
        } = &edits[0]
        else {
            panic!("expected table cell nested list edit");
        };
        assert_eq!(nested_path, &vec![0]);
        assert!(matches!(
            inner_edits[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![0]
        ));
    }

    #[test]
    fn diff_slot_edits_same_shape_preserves_nested_enum_container() {
        use crate::annotated::SlotStep;
        use crate::annotated::{
            AnnotatedContent, Annotation, SemanticKind, SemanticSlot, WrapperKind,
        };

        let make_enum = |text: &str| AnnotatedContent {
            realized: TextElem::packed(text),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Enum),
                slots: vec![SemanticSlot {
                    label: SlotStep::EnumItem(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed(text),
                annotation: Annotation::default(),
                children: vec![],
            }],
        };
        let make_wrapper = |text: &str| AnnotatedContent {
            realized: TextElem::packed("wrapper"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Wrapper(WrapperKind::Block)),
                slots: vec![SemanticSlot {
                    label: SlotStep::WrapperBody,
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed("body"),
                annotation: Annotation::default(),
                children: vec![make_enum(text)],
            }],
        };

        let result =
            diff_slot_edits_same_shape(&make_wrapper("Old step"), &make_wrapper("New step"));
        let RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Nested { edits, .. },
        } = &result[0]
        else {
            panic!("expected wrapper body nested edit");
        };
        assert_eq!(path, &vec![0]);
        let RealizedEdit::ReplaceAt {
            path: enum_path,
            content: EditContent::Nested {
                edits: enum_edits, ..
            },
        } = &edits[0]
        else {
            panic!("expected nested enum container edit");
        };
        assert_eq!(enum_path, &vec![0]);
        assert!(matches!(
            enum_edits[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![0]
        ));
    }

    #[test]
    fn diff_slot_edits_same_shape_preserves_nested_terms_container() {
        use crate::annotated::SlotStep;
        use crate::annotated::{AnnotatedContent, Annotation, SemanticKind, SemanticSlot};

        let make_terms = |description: &str| AnnotatedContent {
            realized: TextElem::packed(description),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Terms),
                slots: vec![
                    SemanticSlot {
                        label: SlotStep::Term(0),
                        path: vec![0],
                        patch_path: None,
                    },
                    SemanticSlot {
                        label: SlotStep::TermDescription(0),
                        path: vec![1],
                        patch_path: None,
                    },
                ],
                ..Annotation::default()
            },
            children: vec![
                AnnotatedContent {
                    realized: TextElem::packed("Habitat"),
                    annotation: Annotation::default(),
                    children: vec![],
                },
                AnnotatedContent {
                    realized: TextElem::packed(description),
                    annotation: Annotation::default(),
                    children: vec![],
                },
            ],
        };
        let make_table = |description: &str| AnnotatedContent {
            realized: TextElem::packed("table"),
            annotation: Annotation {
                semantic_kind: Some(SemanticKind::Table),
                slots: vec![SemanticSlot {
                    label: SlotStep::TableCell(0),
                    path: vec![0],
                    patch_path: None,
                }],
                ..Annotation::default()
            },
            children: vec![AnnotatedContent {
                realized: TextElem::packed("cell"),
                annotation: Annotation::default(),
                children: vec![make_terms(description)],
            }],
        };

        let result = diff_slot_edits_same_shape(
            &make_table("Old forest range"),
            &make_table("New wetland range"),
        );
        let RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Nested { edits, .. },
        } = &result[0]
        else {
            panic!("expected table cell nested edit");
        };
        assert_eq!(path, &vec![0]);
        let RealizedEdit::ReplaceAt {
            path: terms_path,
            content: EditContent::Nested {
                edits: terms_edits, ..
            },
        } = &edits[0]
        else {
            panic!("expected nested terms container edit");
        };
        assert_eq!(terms_path, &vec![0]);
        assert!(matches!(
            terms_edits[0],
            RealizedEdit::ReplaceAt {
                path: ref p,
                content: EditContent::Modified { .. }
            } if p == &vec![1]
        ));
    }

    // --- extract_blocks tests ---

    #[test]
    fn two_paragraphs_become_two_blocks() {
        use typst::model::ParbreakElem;
        let content = seq([
            TextElem::packed("First"),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ]);
        let blocks = extract_blocks(&content);
        assert_eq!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .count(),
            2
        );
    }

    #[test]
    fn nested_sequences_are_flattened_into_blocks() {
        use typst::model::ParbreakElem;
        let nested = seq([
            TextElem::packed("First"),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ]);
        let content = seq([nested]);
        let blocks = extract_blocks(&content);
        assert_eq!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .count(),
            2
        );
    }

    #[test]
    fn styled_sequences_are_split_into_blocks() {
        use typst::model::ParbreakElem;
        use typst::visualize::Color;

        let styled = seq([
            TextElem::packed("First"),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ])
        .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let blocks = extract_blocks(&styled);
        assert_eq!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .count(),
            2
        );
        assert!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .all(|block| block.is::<StyledElem>())
        );
    }

    #[test]
    fn inline_styled_wrapper_does_not_fragment_paragraph_into_multiple_blocks() {
        use typst::visualize::Color;

        // A paragraph body with an inline-styled wrapper between two text runs
        // (the shape Typst's realization produces for "text _emph_ text").
        // The styled element wraps a single TextElem (not a SequenceElem) — the
        // exact case that previously caused fragmentation and led to text loss
        // when the diff recursed into a ParBody slot.
        let par_body = seq([
            TextElem::packed("The species is known as "),
            TextElem::packed("Felis domesticus")
                .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into())),
            TextElem::packed(" in older literature."),
        ]);
        let blocks = extract_block_units(&par_body);
        assert_eq!(
            blocks.len(),
            1,
            "inline-styled wrapper inside a paragraph must not fragment the para into multiple blocks"
        );
        // The single block must include all three pieces of text.
        let text = blocks[0].content.plain_text();
        assert!(text.contains("The species is known as"), "{text}");
        assert!(text.contains("Felis domesticus"), "{text}");
        assert!(text.contains("in older literature"), "{text}");
    }

    #[test]
    fn diff_annotated_on_paragraph_with_inline_styling_produces_single_modified_edit() {
        use typst::visualize::Color;

        let emph_style = TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into());
        let old = seq([
            TextElem::packed("The species is known as "),
            TextElem::packed("Felis domesticus").styled(emph_style.clone()),
            TextElem::packed(" in older literature."),
        ]);
        let new = seq([
            TextElem::packed("The species is known as "),
            TextElem::packed("Felis catus").styled(emph_style),
            TextElem::packed(" in modern taxonomy."),
        ]);

        let result = diff_annotated(&annotated(&old), &annotated(&new));

        assert_eq!(
            result.blocks.len(),
            1,
            "expected 1 block edit, got {}",
            result.blocks.len()
        );
        let word_ops = whole_block_modified_ops(&result).expect("expected modified edit");
        let mut deletes: Vec<&str> = Vec::new();
        let mut inserts: Vec<&str> = Vec::new();
        for op in word_ops {
            match op {
                WordOp::Delete(tokens) => {
                    for t in tokens {
                        deletes.push(t.text.as_str());
                    }
                }
                WordOp::Insert(tokens) => {
                    for t in tokens {
                        inserts.push(t.text.as_str());
                    }
                }
                _ => {}
            }
        }
        let joined_del = deletes.join(" ");
        let joined_ins = inserts.join(" ");
        assert!(joined_del.contains("domesticus"), "deletes: {joined_del:?}");
        assert!(joined_del.contains("older"), "deletes: {joined_del:?}");
        assert!(joined_ins.contains("catus"), "inserts: {joined_ins:?}");
        assert!(joined_ins.contains("modern"), "inserts: {joined_ins:?}");
    }

    #[test]
    fn huge_styled_sequences_keep_non_page_styles() {
        use typst::model::ParbreakElem;
        use typst::visualize::Color;

        let first = "First ".repeat(20_000);
        let second = "Second ".repeat(20_000);
        let styled = seq([
            TextElem::packed(first.as_str()),
            Content::new(ParbreakElem::new()),
            TextElem::packed(second.as_str()),
        ])
        .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let blocks = extract_blocks(&styled);

        assert_eq!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .count(),
            2
        );
        assert!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .all(|block| block.is::<StyledElem>())
        );
    }

    #[test]
    fn page_styles_persist_across_sibling_blocks() {
        use typst::model::ParbreakElem;

        let content = seq([
            seq([TextElem::packed("First")]).styled(PageElem::flipped.set(true)),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ]);
        let blocks = extract_block_units(&content);

        let text_blocks: Vec<_> = blocks
            .iter()
            .filter(|block| !block.content.is::<ParbreakElem>())
            .collect();
        assert_eq!(text_blocks.len(), 2);
        assert!(!blocks[0].page_styles.is_empty());
        assert_eq!(text_blocks[0].page_styles, text_blocks[1].page_styles);
    }

    #[test]
    fn old_display_surfaces_keep_non_page_styles_but_drop_page_styles() {
        use typst::visualize::Color;

        let old = TextElem::packed("Deleted heading")
            .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()))
            .styled(ParElem::justify.set(false))
            .styled(PageElem::flipped.set(true));
        let surface = old_display_surface(&old);

        assert_eq!(surface.plain_text().as_str(), "Deleted heading");
        assert!(contains_style_for::<TextElem>(&surface.content));
        assert!(contains_style_for::<ParElem>(&surface.content));
        assert!(!contains_style_for::<PageElem>(&surface.content));
    }

    #[test]
    fn standalone_old_delete_inherits_current_new_page_styles() {
        use typst::model::ParbreakElem;

        let old = seq([
            TextElem::packed("Keep").styled(PageElem::flipped.set(true)),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Delete"),
        ]);
        let new = TextElem::packed("Keep").styled(PageElem::flipped.set(true));

        let result = diff_annotated(&annotated(&old), &annotated(&new));
        let deleted = result
            .blocks
            .iter()
            .find(|block| block.base_provenance == BlockBaseProvenance::InertOld)
            .expect("expected standalone old deletion");

        assert!(!deleted.page_styles.is_empty());
        assert!(deleted.page_styles.has(PageElem::flipped));
    }

    #[test]
    fn boundary_pagebreak_replaces_sticky_page_styles() {
        use typst::layout::PagebreakElem;
        use typst::model::ParbreakElem;

        let content = seq([
            seq([TextElem::packed("First")]).styled(PageElem::flipped.set(true)),
            Content::new(PagebreakElem::new().with_boundary(true))
                .styled(PageElem::flipped.set(false)),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ]);
        let blocks = extract_block_units(&content);

        let text_blocks: Vec<_> = blocks
            .iter()
            .filter(|block| {
                !block.content.is::<ParbreakElem>() && !block.content.is::<PagebreakElem>()
            })
            .collect();
        assert_eq!(text_blocks.len(), 2);
        assert_ne!(text_blocks[0].page_styles, text_blocks[1].page_styles);
    }

    #[test]
    fn heading_is_own_block() {
        use typst::model::ParbreakElem;
        let content = seq([
            Content::new(HeadingElem::new(TextElem::packed("Title"))),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Body"),
        ]);
        let blocks = extract_blocks(&content);
        assert_eq!(
            blocks
                .iter()
                .filter(|block| !block.is::<ParbreakElem>())
                .count(),
            2
        );
        assert!(blocks[0].is::<HeadingElem>());
    }

    #[test]
    fn trailing_content_without_parbreak_becomes_block() {
        let content = seq([TextElem::packed("Only paragraph")]);
        let blocks = extract_blocks(&content);
        assert_eq!(blocks.len(), 1);
    }

    // --- extract_words tests ---

    #[test]
    fn text_elem_splits_into_words() {
        let content = TextElem::packed("hello world foo");
        let tokens = extract_words(&content);
        let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert!(texts.contains(&"hello"));
        assert!(texts.contains(&"world"));
        assert!(texts.contains(&"foo"));
    }

    #[test]
    fn strong_elem_is_atomic_token() {
        use typst::model::StrongElem;
        let strong = Content::new(StrongElem::new(TextElem::packed("bold")));
        let para = seq([
            TextElem::packed("before "),
            strong,
            TextElem::packed(" after"),
        ]);
        let tokens = extract_words(&para);
        assert!(
            tokens
                .iter()
                .any(|t| t.text == "bold" || t.content.is::<StrongElem>())
        );
    }

    #[test]
    fn large_atomic_content_stays_atomic_without_semantic_children() {
        use typst::model::StrongElem;

        let text = "alpha beta gamma ".repeat(40);
        let strong = Content::new(StrongElem::new(TextElem::packed(text.as_str())));
        let tokens = extract_words(&strong);

        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, text);
        assert!(tokens[0].content.is::<StrongElem>());
    }

    #[test]
    fn semantic_wrapper_body_splits_into_words() {
        use typst::layout::{BlockBody, BlockElem};

        let block = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(
            TextElem::packed("alpha beta gamma"),
        ))));
        let tokens = extract_words(&block);
        let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();

        assert!(texts.contains(&"alpha"), "tokens: {texts:?}");
        assert!(texts.contains(&"beta"), "tokens: {texts:?}");
        assert!(texts.contains(&"gamma"), "tokens: {texts:?}");
    }

    #[test]
    fn empty_block_does_not_consume_equation_origin_tokens() {
        use typst::layout::{BlockBody, BlockElem};

        let empty_block = Content::new(
            BlockElem::new().with_body(Some(BlockBody::Content(Content::sequence([])))),
        );
        let origin = Content::new(EquationElem::new(TextElem::packed("x")));

        let tokens = extract_words_for_annotated_with_equation_origins(
            &empty_block,
            None,
            &[origin.clone()],
        );
        assert!(
            !has_meaningful_tokens(&tokens),
            "empty structural block must not surface unrelated equation origins as word tokens"
        );

        let edit = context_preserving_inserted_edit(empty_block.clone(), None, &[origin]);
        assert!(
            matches!(edit, EditContent::Inserted(content) if content == empty_block),
            "empty structural inserted content should remain live structural content, not a formula diff"
        );
    }

    // --- diff_blocks_raw tests ---

    #[test]
    fn identical_blocks_all_equal() {
        let a = vec![TextElem::packed("Hello"), TextElem::packed("World")];
        let b = a.clone();
        let ops = diff_blocks_raw(&a, &b);
        assert!(ops.iter().all(|op| matches!(op, BlockOp::Equal(_, _))));
    }

    #[test]
    fn added_block_detected() {
        let old = vec![TextElem::packed("Only old")];
        let new = vec![TextElem::packed("Only old"), TextElem::packed("New block")];
        let ops = diff_blocks_raw(&old, &new);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Insert(_))));
    }

    #[test]
    fn deleted_block_detected() {
        let old = vec![TextElem::packed("A"), TextElem::packed("B")];
        let new = vec![TextElem::packed("A")];
        let ops = diff_blocks_raw(&old, &new);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Delete(_))));
    }

    // --- match_edit_zones tests ---

    #[test]
    fn similar_blocks_become_replace() {
        let old = vec![TextElem::packed("The quick brown fox jumps.")];
        let new = vec![TextElem::packed("The quick brown fox leaps.")];
        let raw = diff_blocks_raw(&old, &new);
        let ops = match_edit_zones(raw);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Replace(_, _))));
    }

    #[test]
    fn dissimilar_blocks_stay_delete_insert() {
        let old = vec![TextElem::packed("Completely unrelated old content.")];
        let new = vec![TextElem::packed("xyz")];
        let raw = diff_blocks_raw(&old, &new);
        let ops = match_edit_zones(raw);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Insert(_))));
    }

    #[test]
    fn edit_distance_respects_limit() {
        assert_eq!(edit_distance_with_limit("kitten", "sitting", 3), Some(3));
        assert_eq!(edit_distance_with_limit("kitten", "sitting", 2), None);
    }

    #[test]
    fn similarity_handles_large_dissimilar_texts() {
        let a = "a".repeat(10_000);
        let b = "b".repeat(10_000);
        assert_eq!(similarity(&a, &b), 0.0);
    }

    #[test]
    fn similarity_handles_large_insertions() {
        let old = "alpha beta gamma ".repeat(1_000);
        let new = format!("Foo {old}");
        assert!(similarity(&old, &new) > 0.99);
    }

    // --- diff_words tests ---

    #[test]
    fn changed_word_produces_delete_and_insert() {
        let old = extract_words(&TextElem::packed("The quick brown fox jumps."));
        let new = extract_words(&TextElem::packed("The quick brown fox leaps."));
        let ops = diff_words(&old, &new);
        assert!(
            ops.iter().any(|op| matches!(op, WordOp::Delete(_))),
            "expected delete op"
        );
        assert!(
            ops.iter().any(|op| matches!(op, WordOp::Insert(_))),
            "expected insert op"
        );
    }

    #[test]
    fn same_text_with_style_change_produces_delete_and_insert() {
        use typst::visualize::Color;

        let old = extract_words(&TextElem::packed("important"));
        let new = extract_words(
            &TextElem::packed("important")
                .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into())),
        );
        let ops = diff_words(&old, &new);

        assert!(ops.iter().any(|op| matches!(op, WordOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, WordOp::Insert(_))));
    }

    #[test]
    fn same_text_subscript_to_superscript_produces_delete_and_insert() {
        use typst::text::{SubElem, SuperElem};

        let old = extract_words(&Content::new(SubElem::new(TextElem::packed("2"))));
        let new = extract_words(&Content::new(SuperElem::new(TextElem::packed("2"))));
        let ops = diff_words(&old, &new);

        assert!(ops.iter().any(|op| matches!(op, WordOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, WordOp::Insert(_))));
    }

    #[test]
    fn same_text_paragraph_to_heading_produces_delete_and_insert() {
        use typst::model::ParElem;

        let old = extract_words(&Content::new(ParElem::new(TextElem::packed("Background"))));
        let new = extract_words(&Content::new(HeadingElem::new(TextElem::packed(
            "Background",
        ))));
        let ops = diff_words(&old, &new);

        assert!(ops.iter().any(|op| matches!(op, WordOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, WordOp::Insert(_))));
    }

    #[test]
    fn same_equation_repr_with_different_blockness_produces_delete_and_insert() {
        let old = extract_words(&Content::new(
            EquationElem::new(TextElem::packed("x")).with_block(false),
        ));
        let new = extract_words(&Content::new(
            EquationElem::new(TextElem::packed("x")).with_block(true),
        ));
        let ops = diff_words(&old, &new);

        assert!(ops.iter().any(|op| matches!(op, WordOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, WordOp::Insert(_))));
    }

    #[test]
    fn block_context_token_identity_does_not_include_sibling_text() {
        let old = extract_words(&Content::new(BlockElem::new().with_body(Some(
            BlockBody::Content(TextElem::packed("Definition 2 -- A tree is acyclic.")),
        ))));
        let new = extract_words(&Content::new(BlockElem::new().with_body(Some(
            BlockBody::Content(TextElem::packed("Definition 2 -- A forest is acyclic.")),
        ))));
        let ops = diff_words(&old, &new);
        let equals = collect_word_op_text(&ops, |op| match op {
            WordOp::Equal(tokens) => Some(tokens),
            _ => None,
        });

        assert!(equals.contains("Definition 2"), "ops={ops:?}");
        assert!(equals.contains("acyclic"), "ops={ops:?}");
    }

    #[test]
    fn identical_words_all_equal() {
        let tokens = extract_words(&TextElem::packed("Hello world."));
        let ops = diff_words(&tokens, &tokens.clone());
        assert!(ops.iter().all(|op| matches!(op, WordOp::Equal(_))));
    }

    #[test]
    fn sentence_substitution_merges_into_one_delete_one_insert() {
        let old = extract_words(&TextElem::packed("The quick brown fox jumps."));
        let new = extract_words(&TextElem::packed("A slow red dog leaps."));
        let ops = diff_words(&old, &new);
        let n_del = ops
            .iter()
            .filter(|op| matches!(op, WordOp::Delete(_)))
            .count();
        let n_ins = ops
            .iter()
            .filter(|op| matches!(op, WordOp::Insert(_)))
            .count();
        assert_eq!(n_del, 1, "expected exactly one merged Delete run");
        assert_eq!(n_ins, 1, "expected exactly one merged Insert run");
    }

    #[test]
    fn partial_substitution_preserves_equal_words() {
        // "The fox leaps." — only "leaps" changes; "The" and "fox" stay equal.
        let old = extract_words(&TextElem::packed("The fox jumps."));
        let new = extract_words(&TextElem::packed("The fox leaps."));
        let ops = diff_words(&old, &new);
        let n_equal = ops
            .iter()
            .filter(|op| matches!(op, WordOp::Equal(_)))
            .count();
        let n_del = ops
            .iter()
            .filter(|op| matches!(op, WordOp::Delete(_)))
            .count();
        let n_ins = ops
            .iter()
            .filter(|op| matches!(op, WordOp::Insert(_)))
            .count();
        assert!(n_equal >= 1, "expected Equal ops for unchanged prefix");
        assert_eq!(n_del, 1);
        assert_eq!(n_ins, 1);
    }

    // --- diff_annotated tests ---

    #[test]
    fn diff_annotated_detects_word_change() {
        let old = seq([TextElem::packed("The fox jumps.")]);
        let new = seq([TextElem::packed("The fox leaps.")]);
        let result = diff_annotated(&annotated(&old), &annotated(&new));
        let has_word_change = whole_block_modified_ops(&result).is_some_and(|word_ops| {
            word_ops
                .iter()
                .any(|w| matches!(w, WordOp::Delete(_)) || matches!(w, WordOp::Insert(_)))
        });
        assert!(has_word_change);
    }

    #[test]
    fn extract_blocks_keeps_structured_containers_as_single_blocks() {
        use typst::foundations::Packed;
        use typst::model::{FigureElem, ListElem, ListItem, TableCell, TableElem};

        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Beta"))),
        ]));
        let table = Content::new(TableElem::new(vec![typst::model::TableChild::Item(
            typst::model::TableItem::Cell(Packed::new(TableCell::new(TextElem::packed("Cell")))),
        )]));
        let figure = Content::new(FigureElem::new(TextElem::packed("Body")));
        let content = seq([list, table, figure]);

        let blocks = extract_blocks(&content);

        assert_eq!(blocks.len(), 3);
        assert!(blocks[0].is::<ListElem>());
        assert!(blocks[1].is::<TableElem>());
        assert!(blocks[2].is::<FigureElem>());
    }

    #[test]
    fn extract_words_preserves_punctuation_with_non_whitespace_runs_and_unicode_words() {
        let tokens = extract_words(&TextElem::packed("Hello, café 世界!"));
        let texts: Vec<_> = tokens.iter().map(|token| token.text.as_str()).collect();

        assert!(texts.contains(&"Hello"));
        assert!(texts.contains(&","));
        assert!(texts.contains(&"café"));
        assert!(texts.contains(&"世界"));
        assert!(texts.contains(&"!"));
    }

    #[test]
    fn extract_words_preserves_styles_on_split_tokens() {
        use typst::visualize::Color;

        let styled = TextElem::packed("old technical concept")
            .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let tokens = extract_words(&styled);

        assert!(tokens.iter().any(|token| token.text == "technical"));
        assert!(
            tokens.iter().all(|token| token.content.is::<StyledElem>()),
            "{tokens:?}"
        );
    }

    #[test]
    fn modification_log_collapses_multiline_text_to_single_line() {
        let old = seq([TextElem::packed("Old\nvalue")]);
        let new = seq([TextElem::packed("New\nvalue")]);

        let log = diff_annotated(&annotated(&old), &annotated(&new)).modification_log();

        assert!(log.contains("block: New value"), "{log}");
        assert!(log.contains("deleted: Old"), "{log}");
        assert!(log.contains("inserted: New"), "{log}");
        assert!(!log.contains("Old\nvalue"), "{log}");
        assert!(!log.contains("New\nvalue"), "{log}");
    }

    #[test]
    fn match_edit_zones_pairs_best_similar_blocks() {
        let old = vec![
            TextElem::packed("Alpha beta gamma delta epsilon old zeta eta theta."),
            TextElem::packed("Completely different old paragraph."),
        ];
        let new = vec![
            TextElem::packed("Completely different new paragraph."),
            TextElem::packed("Alpha beta gamma delta epsilon new zeta eta theta."),
        ];

        let ops = match_edit_zones(diff_blocks_raw(&old, &new));

        assert!(ops.iter().any(|op| match op {
            BlockOp::Replace(old, new) => {
                old.content.plain_text().contains("epsilon")
                    && new.content.plain_text().contains("epsilon")
            }
            _ => false,
        }));
    }
}
