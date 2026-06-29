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
//!    Leaf replacements fall back to [`diff_words`].

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use similar::{Algorithm, DiffOp, capture_diff_slices};
use typst::World;
use typst::foundations::{
    Content, NativeElement, Repr, SequenceElem, Smart, Style, StyleChain, StyledElem, Styles,
};
use typst::introspection::Tag;
use typst::layout::{
    BlockBody, BlockElem, BoxElem, Frame, FrameItem, PageElem, PagedDocument, Point, Rel,
};
use typst::math::EquationElem;
use typst::model::{EmphElem, FootnoteBody, FootnoteElem, HeadingElem, ParElem, ParbreakElem};
use typst::text::{RawElem, SpaceElem, TextElem};

use crate::annotated::{
    AnnotatedContent, SemanticKind, SemanticSlot, SlotStep, annotate_realized,
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
/// Equality and hashing are based solely on `text` so that the Myers LCS algorithm
/// can match tokens by visible text content regardless of their structural `content`
/// representation. `content` carries the original `Content` node so that unchanged
/// tokens can be reconstructed faithfully in the annotated output.
#[derive(Clone, Debug)]
pub struct Token {
    pub text: String,
    pub content: Content,
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
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
        self.text.cmp(&other.text)
    }
}
impl Hash for Token {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state)
    }
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

fn has_equation_origins(node: &AnnotatedContent) -> bool {
    !node.annotation.equation_origins.is_empty() || node.children.iter().any(has_equation_origins)
}

fn collect_annotated_tokens(node: &AnnotatedContent, out: &mut Vec<Token>) {
    if !node.annotation.equation_origins.is_empty() {
        let mut origins = node.annotation.equation_origins.iter();
        collect_tokens_with_equation_origins(&node.realized, &mut origins, out);
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
        collect_tokens(&par.body, out);
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
        collect_tokens_with_equation_origins(&par.body, origins, out);
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            collect_tokens_with_equation_origins(&body, origins, out);
        } else {
            collect_tokens(content, out);
        }
    } else {
        collect_tokens(content, out);
    }
}

fn is_realized_equation_carrier(content: &Content) -> bool {
    content.is::<EquationElem>()
        || matches!(content.func().name(), "inline" | "display")
        || (content.is::<BlockElem>() && content.plain_text().is_empty())
}

fn collect_semantic_child_tokens(content: &Content, out: &mut Vec<Token>) -> bool {
    let children = container_ops::semantic_diff_child_contents(content);
    if children.is_empty() {
        return false;
    }
    for (index, child) in children.into_iter().enumerate() {
        let before = out.len();
        collect_tokens(&child, out);
        if content.is::<BoxElem>() {
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
            DiffOp::Equal { old_index, len, .. } => {
                coalesce(
                    &mut result,
                    WordOp::Equal(old[old_index..old_index + len].to_vec()),
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
    /// Edits to apply to `base.realized`.
    pub edits: Vec<RealizedEdit>,
    /// Page styles active at this block's position (for output grouping).
    pub page_styles: Styles,
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
}

pub enum EditContent {
    Inserted(Content),
    Deleted(Content),
    OpaqueReplacement {
        old: Content,
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
        | RealizedEdit::WholeBlock(content) => log_edit_content(log, content, index),
    }
}

fn realized_edit_kind(edit: &RealizedEdit) -> &'static str {
    match edit {
        RealizedEdit::ReplaceAt { content, .. } => edit_content_kind(content),
        RealizedEdit::InsertBefore { content, .. } => edit_content_kind(content),
        RealizedEdit::InsertAfter { content, .. } => edit_content_kind(content),
        RealizedEdit::Append { content } => edit_content_kind(content),
        RealizedEdit::WholeBlock(content) => edit_content_kind(content),
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
        EditContent::Inserted(content) => push_log_entry(
            log,
            index,
            "insert",
            &[("text", content.plain_text().to_string())],
        ),
        EditContent::Deleted(content) => push_log_entry(
            log,
            index,
            "delete",
            &[("text", content.plain_text().to_string())],
        ),
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
                        ("block", base.plain_text().to_string()),
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
                edits,
                page_styles,
            });
        }
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
        let new_tokens = extract_words_for_annotated(&new_block.content, new_ann);
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
                edits: vec![RealizedEdit::WholeBlock(EditContent::Modified {
                    base: new_block.content.clone(),
                    word_ops,
                })],
                page_styles,
            });
        }
    }

    let old_tokens = extract_words_for_annotated(&old_block.content, old_ann);
    let new_tokens = extract_words_for_annotated(&new_block.content, new_ann);
    let word_ops = diff_words(&old_tokens, &new_tokens);
    let edits = if has_textual_word_change(&word_ops) {
        vec![RealizedEdit::WholeBlock(EditContent::Modified {
            base: new_block.content.clone(),
            word_ops,
        })]
    } else if old_block.content != new_block.content
        && old_block.content.plain_text().is_empty()
        && new_block.content.plain_text().is_empty()
    {
        vec![RealizedEdit::WholeBlock(EditContent::OpaqueReplacement {
            old: old_block.content.clone(),
            new: new_block.content.clone(),
        })]
    } else {
        vec![]
    };
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
        edits,
        page_styles,
    })
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
    let mut blocks = Vec::new();
    let mut recursed_equal_semantic_nodes = HashSet::new();
    let matched_ops = prepared.matched_ops;
    let mut op_index = 0;
    while op_index < matched_ops.len() {
        let op = matched_ops[op_index].clone();
        op_index += 1;
        match op {
            BlockOp::Equal(old_block, new_block) => {
                let page_styles = new_block.page_styles.clone();
                let old_ann = old_owners
                    .take_owner_for(&old_block.content)
                    .or_else(|| find_nonempty_annotated_child(old, &old_block.content));
                let new_ann = new_owners
                    .take_owner_for(&new_block.content)
                    .or_else(|| find_nonempty_annotated_child(new, &new_block.content));
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
                blocks.extend(layout.take_before(&new_block.content, new_ann));
                if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann)
                    && can_recurse_via_slots(old_ann, new_ann)
                {
                    if new_block.content.plain_text().is_empty() {
                        let recursed_key = semantic_edit_claim_key(new_ann);
                        if !recursed_equal_semantic_nodes.insert(recursed_key) {
                            blocks.push(DiffBlockEdit {
                                base: annotated_block_from(&new_block.content, None),
                                edits: vec![],
                                page_styles,
                            });
                            continue;
                        }
                    }
                    if annotated_subtree_equal(old_ann, new_ann) {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, None),
                            edits: vec![],
                            page_styles,
                        });
                        continue;
                    }
                    let mut edits = footnote_visible_text_edits(old_ann, new_ann);
                    edits.extend(diff_slot_edits_with_events(old_ann, new_ann, debug_events)?);
                    if !edits.is_empty() {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            edits,
                            page_styles,
                        });
                        continue;
                    }
                    if new_block.content.plain_text().is_empty()
                        && let Some(edit) = owned_surface_modified_edit(old_ann, new_ann)
                    {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            edits: vec![edit],
                            page_styles,
                        });
                        continue;
                    }
                }
                if let (Some(old_ann), Some(new_ann)) = (old_ann, new_ann)
                    && (has_equation_origins(old_ann) || has_equation_origins(new_ann))
                {
                    let old_tokens = extract_words_for_annotated(&old_block.content, Some(old_ann));
                    let new_tokens = extract_words_for_annotated(&new_block.content, Some(new_ann));
                    let word_ops = diff_words(&old_tokens, &new_tokens);
                    if has_textual_word_change(&word_ops) {
                        blocks.push(DiffBlockEdit {
                            base: annotated_block_from(&new_block.content, Some(new_ann)),
                            edits: vec![RealizedEdit::WholeBlock(EditContent::Modified {
                                base: new_block.content.clone(),
                                word_ops,
                            })],
                            page_styles,
                        });
                        continue;
                    }
                }
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&new_block.content, None),
                    edits: vec![],
                    page_styles: new_block.page_styles,
                });
            }
            BlockOp::Delete(old_block) => {
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
                        blocks.extend(layout.take_before(&new_block.content, new_claim.owner));
                        blocks.push(replace_block_edit(
                            old,
                            new,
                            &old_block,
                            &new_block,
                            old_claim.owner,
                            new_claim.owner,
                            debug_events,
                        )?);
                        op_index += 1;
                        continue;
                    }
                }
                let old_ann = old_claim
                    .owner
                    .or_else(|| find_nonempty_annotated_child(old, &old_block.content));
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&old_block.content, old_ann),
                    edits: vec![RealizedEdit::WholeBlock(deleted_edit(
                        old_block.content.clone(),
                    ))],
                    page_styles: old_block.page_styles,
                });
            }
            BlockOp::Insert(new_block) => {
                let new_ann = new_owners
                    .take_owner_for(&new_block.content)
                    .or_else(|| find_nonempty_annotated_child(new, &new_block.content));
                blocks.extend(layout.take_before(&new_block.content, new_ann));
                blocks.push(DiffBlockEdit {
                    base: annotated_block_from(&new_block.content, new_ann),
                    edits: vec![RealizedEdit::WholeBlock(EditContent::Inserted(
                        new_block.content.clone(),
                    ))],
                    page_styles: new_block.page_styles,
                });
            }
            BlockOp::Replace(old_block, new_block) => {
                let old_ann = old_owners
                    .take_owner_for(&old_block.content)
                    .or_else(|| find_nonempty_annotated_block_owner(old, &old_block.content));
                let new_ann = new_owners
                    .take_owner_for(&new_block.content)
                    .or_else(|| find_nonempty_annotated_block_owner(new, &new_block.content));
                blocks.extend(layout.take_before(&new_block.content, new_ann));
                blocks.push(replace_block_edit(
                    old,
                    new,
                    &old_block,
                    &new_block,
                    old_ann,
                    new_ann,
                    debug_events,
                )?);
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

fn is_owned_empty_edit_block(block: &DiffBlockEdit) -> bool {
    block.base.realized.plain_text().is_empty() && !block.base.annotation.slots.is_empty()
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
        let footnote = Content::new(FootnoteElem::new(FootnoteBody::Content(old_body.clone())));
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
            edits: vec![RealizedEdit::ReplaceAt {
                path,
                content: EditContent::Modified {
                    base: old_body,
                    word_ops: vec![WordOp::Delete(tokens)],
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
        | RealizedEdit::WholeBlock(content) => collect_edit_content_signature(content, signatures),
    }
}

fn collect_edit_content_signature(content: &EditContent, signatures: &mut HashSet<String>) {
    match content {
        EditContent::Modified { base, word_ops } if has_textual_word_change(word_ops) => {
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
                content_signature(old),
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
    let old_tokens = extract_words_for_annotated(&old_effective, Some(old));
    let new_tokens = extract_words_for_annotated(&new_effective, Some(new));
    let word_ops = diff_words(&old_tokens, &new_tokens);
    if has_textual_word_change(&word_ops) {
        vec![RealizedEdit::WholeBlock(EditContent::Modified {
            base: new_effective,
            word_ops,
        })]
    } else if let Some(content) = opaque_replacement_content(old, new) {
        vec![RealizedEdit::WholeBlock(content)]
    } else {
        vec![]
    }
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
            let old_text = old_page.map(|page| page.text.as_str()).unwrap_or("");
            let new_content = TextElem::packed(new_page.text.as_str());
            let old_content = TextElem::packed(old_text);
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

    fn take_before(
        &mut self,
        target: &Content,
        owner: Option<&AnnotatedContent>,
    ) -> Vec<DiffBlockEdit> {
        let mut out = Vec::new();
        while let Some(block) = self.blocks.get(self.index) {
            self.index += 1;
            if block.content.is::<ParbreakElem>() {
                out.push(layout_block_edit(block));
                continue;
            }
            if layout_content_matches(&block.content, target, owner) {
                break;
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
        let Some(claim) = self.claims.get(self.index) else {
            return BlockOwnerMatch {
                owner: None,
                key: None,
            };
        };
        self.index += 1;
        if owned_block_matches(&claim.content, target) {
            BlockOwnerMatch {
                owner: claim.owner,
                key: claim.key.clone(),
            }
        } else {
            BlockOwnerMatch {
                owner: None,
                key: None,
            }
        }
    }

    fn peek_claim_for(&self, target: &Content) -> BlockOwnerMatch<'a> {
        let Some(claim) = self.claims.get(self.index) else {
            return BlockOwnerMatch {
                owner: None,
                key: None,
            };
        };
        if owned_block_matches(&claim.content, target) {
            BlockOwnerMatch {
                owner: claim.owner,
                key: claim.key.clone(),
            }
        } else {
            BlockOwnerMatch {
                owner: None,
                key: None,
            }
        }
    }
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
    let blocks = non_parbreak_blocks(&extract_block_units(&node.realized));
    if blocks.len() == 1 {
        let owner = (is_owned_diff_region(node) || has_equation_origins(node)).then_some(node);
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

fn is_owned_diff_region(node: &AnnotatedContent) -> bool {
    !node.annotation.slots.is_empty()
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
        edits: vec![],
        page_styles: block.page_styles.clone(),
    }
}

fn layout_content_matches(
    layout: &Content,
    target: &Content,
    owner: Option<&AnnotatedContent>,
) -> bool {
    if layout == target {
        return true;
    }
    let layout_text = layout.plain_text();
    let target_text = target.plain_text();
    if !layout_text.is_empty() && layout_text == target_text {
        return true;
    }

    let Some(owner) = owner else {
        return false;
    };
    let owner_surface = effective_text_content(owner);
    if layout == &owner_surface {
        return true;
    }
    normalized_visible_text_matches(layout, &owner_surface)
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

fn find_nonempty_annotated_child<'a>(
    root: &'a AnnotatedContent,
    target: &Content,
) -> Option<&'a AnnotatedContent> {
    (!target.plain_text().is_empty()).then(|| find_annotated_child(root, target))?
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
        let blocks = non_parbreak_blocks(&extract_block_units(&node.realized));
        if blocks.len() == 1 && blocks[0].content == *target {
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

fn find_nonempty_annotated_block_owner<'a>(
    root: &'a AnnotatedContent,
    target: &Content,
) -> Option<&'a AnnotatedContent> {
    (!target.plain_text().is_empty()).then(|| find_annotated_block_owner(root, target))?
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
    if old_child.children.is_empty() || old_child.children.len() != new_child.children.len() {
        return None;
    }
    if slot_bearing_descendants(old_child).is_empty()
        && slot_bearing_descendants(new_child).is_empty()
    {
        return None;
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

fn owned_surface_modified_edit(
    old_ann: &AnnotatedContent,
    new_ann: &AnnotatedContent,
) -> Option<RealizedEdit> {
    let old_effective = effective_text_content(old_ann);
    let new_effective = effective_text_content(new_ann);
    if old_effective == new_effective {
        return None;
    }

    let old_tokens = extract_words(&old_effective);
    let new_tokens = extract_words(&new_effective);
    let word_ops = diff_words(&old_tokens, &new_tokens);
    if has_textual_word_change(&word_ops) {
        return Some(RealizedEdit::WholeBlock(EditContent::Modified {
            base: new_effective,
            word_ops,
        }));
    }

    opaque_replacement_content(old_ann, new_ann).map(RealizedEdit::WholeBlock)
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

fn semantic_edit_claim_key(node: &AnnotatedContent) -> String {
    format!(
        "{:?}|{:?}|{}|{}",
        node.annotation.semantic_kind,
        slot_labels(node),
        effective_text_content(node).plain_text(),
        content_signature(&effective_render_content(node))
    )
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
            push_modified_slot_edit(&mut edits, old_child, new_child, &new_slot.path);
        }
    }

    edits
}

fn push_modified_slot_edit(
    edits: &mut Vec<RealizedEdit>,
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
    path: &[usize],
) {
    if let Some(content) = modified_edit_content(old_child, new_child) {
        edits.push(RealizedEdit::ReplaceAt {
            path: path.to_vec(),
            content,
        });
    }
}

fn modified_edit_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    let old_effective = effective_text_content(old_child);
    let new_effective = effective_text_content(new_child);
    let old_tokens = extract_words_for_annotated(&old_effective, Some(old_child));
    let new_tokens = extract_words_for_annotated(&new_effective, Some(new_child));
    let word_ops = diff_words(&old_tokens, &new_tokens);
    if has_textual_word_change(&word_ops) {
        return Some(EditContent::Modified {
            base: new_effective,
            word_ops,
        });
    }

    opaque_replacement_content(old_child, new_child)
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
        .map(|unit| unit.text.clone())
        .collect::<Vec<_>>();
    let new_keys = new_units
        .iter()
        .map(|unit| unit.text.clone())
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
                        let old_tokens = extract_words(&old_unit.content);
                        let new_tokens = extract_words(&new_unit.content);
                        let word_ops = diff_words(&old_tokens, &new_tokens);
                        if has_textual_word_change(&word_ops) {
                            edits.push(RealizedEdit::ReplaceAt {
                                path: path.clone(),
                                content: EditContent::Modified {
                                    base: new_unit.content.clone(),
                                    word_ops,
                                },
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
    let content = Content::sequence(deleted.iter().map(|token| token.content.clone()));

    vec![RealizedEdit::InsertBefore {
        anchor,
        content: EditContent::Deleted(content),
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

fn opaque_replacement_content(
    old_child: &AnnotatedContent,
    new_child: &AnnotatedContent,
) -> Option<EditContent> {
    let old_visual = effective_render_content(old_child);
    let new_visual = effective_render_content(new_child);
    if old_visual != new_visual
        && old_visual.plain_text().is_empty()
        && new_visual.plain_text().is_empty()
    {
        return Some(EditContent::OpaqueReplacement {
            old: old_visual,
            new: new_visual,
        });
    }
    None
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
                effective_text_content(child),
            )))
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return None;
    }

    let content = if calls.len() == 1 {
        let call = calls.into_iter().next().expect("single deleted footnote");
        let tokens = extract_words(&call);
        EditContent::Modified {
            base: call,
            word_ops: vec![WordOp::Delete(tokens)],
        }
    } else {
        EditContent::Deleted(Content::sequence(calls))
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
        .map(|child| effective_text_content(child).plain_text().to_string())
        .collect();
    let new_h: Vec<String> = new_ann
        .annotation
        .slots
        .iter()
        .filter_map(|slot| new_ann.get_path(&slot.path))
        .map(|child| effective_text_content(child).plain_text().to_string())
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
                            &new_slots[new_index + i].0.path,
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
                            &new_slots[new_index + i].0.path,
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

fn push_deleted_slot_edit(
    edits: &mut Vec<RealizedEdit>,
    old_child: &AnnotatedContent,
    new_slots: &[(&SemanticSlot, &AnnotatedContent)],
    new_index: usize,
) {
    let content = deleted_edit(effective_text_content(old_child));
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
    EditContent::Deleted(content)
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
                    },
                    SemanticSlot {
                        label: SlotStep::ListItem(1),
                        path: vec![1],
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
                    },
                    SemanticSlot {
                        label: SlotStep::TermDescription(0),
                        path: vec![1],
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
