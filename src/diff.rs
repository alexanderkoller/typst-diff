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
//! 4. **Word-level diff** — `diff_content` drives all of the above, then for each
//!    `Replace` pair either:
//!    - diffs the slot contents (lists, tables, …) with [`diff_words`], or
//!    - extracts tokens with [`extract_words`] and diffs them with [`diff_words`].
//!    Only pairs that contain a real textual change become [`DiffResultOp::Modified`];
//!    style-only changes collapse back to `Equal`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use similar::{Algorithm, DiffOp, capture_diff_slices};
use typst::foundations::{
    Content, NativeElement, Repr, SequenceElem, Smart, Style, StyleChain, StyledElem, Styles,
};
use typst::layout::{BlockBody, BlockElem, PageElem, Rel};
use typst::math::EquationElem;
use typst::model::{HeadingElem, ParElem, ParbreakElem};
use typst::text::{RawElem, SpaceElem, TextElem};

use crate::annotated::{AnnotatedContent, Annotation};
use crate::content_slots::SlotStep;

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
    for block in blocks {
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
/// - Slot-container nodes (lists, figures, …) are recursed via [`collect_slot_tokens`].
/// - Any other node becomes a single atomic token. If the node's plain text exceeds
///   500 characters it is split into word/space tokens instead of kept atomic, so
///   that large opaque elements (e.g. huge `StrongElem` runs) are still word-diffable.
pub fn extract_words(content: &Content) -> Vec<Token> {
    let mut tokens = Vec::new();
    collect_tokens(content, &mut tokens);
    tokens
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
    } else if let Some(par) = content.to_packed::<ParElem>() {
        collect_tokens(&par.body, out);
    } else if let Some(equation) = content.to_packed::<EquationElem>() {
        out.push(Token {
            text: equation.body.repr().to_string(),
            content: content.clone(),
        });
    } else if collect_slot_tokens(content, out) {
    } else if let Some(text_elem) = content.to_packed::<TextElem>() {
        collect_text_tokens(text_elem.text.as_str(), out);
    } else if content.is::<SpaceElem>() {
        out.push(Token {
            text: " ".to_string(),
            content: content.clone(),
        });
    } else {
        let text = content.plain_text();
        if text.len() > 500 {
            collect_text_tokens(text.as_str(), out);
        } else {
            out.push(Token {
                text: text.to_string(),
                content: content.clone(),
            });
        }
    }
}

fn collect_slot_tokens(_content: &Content, _out: &mut Vec<Token>) -> bool {
    // Phase A: slot-bearing elements fall through to atomic plain-text
    // tokenization. Slot-level diffing is now handled via annotation.slots
    // in the new tree path (diff_annotated).
    false
}

fn collect_text_tokens(s: &str, out: &mut Vec<Token>) {
    let mut start = 0;
    let mut in_space = s.chars().next().map(|c| c.is_whitespace()).unwrap_or(false);
    for (i, ch) in s.char_indices() {
        if ch.is_whitespace() != in_space {
            let slice = &s[start..i];
            if !slice.is_empty() {
                out.push(Token {
                    text: slice.to_string(),
                    content: TextElem::packed(slice),
                });
            }
            start = i;
            in_space = ch.is_whitespace();
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

/// Scan the raw ops for contiguous `Delete + Insert` zones and pair them by similarity.
///
/// Within each zone every delete is greedily matched to the most-similar unused insert
/// (similarity threshold 0.3). Pairs become [`BlockOp::Replace`]; unmatched deletes and
/// inserts are emitted as-is. Paired inserts are emitted after all deletes (in their
/// original order) to keep the output sequence stable.
pub fn match_edit_zones(ops: Vec<BlockOp>) -> Vec<BlockOp> {
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
                pair_edit_zone(deletes, inserts, &mut result);
            }
        }
    }
    result
}

fn pair_edit_zone(deletes: Vec<DiffBlock>, inserts: Vec<DiffBlock>, out: &mut Vec<BlockOp>) {
    if deletes.is_empty() {
        out.extend(inserts.into_iter().map(BlockOp::Insert));
        return;
    }
    if inserts.is_empty() {
        out.extend(deletes.into_iter().map(BlockOp::Delete));
        return;
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

        match best {
            Some((j, sim)) if sim >= 0.3 => {
                used_inserts[j] = true;
                paired_insert_idx.push(Some(j));
            }
            _ => paired_insert_idx.push(None),
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
            out.push(BlockOp::Insert(ins));
        }
    }
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

/// Final per-block diff classification, passed to `annotate::build_annotated_content`.
///
/// All variants carry the *new* block (or the only block for `Equal`/`Deleted`).
/// `Modified` and `ModifiedSlots` also carry the word-level or slot-level diffs.
#[derive(Clone)]
pub enum DiffResultOp {
    Equal(DiffBlock),
    Deleted(DiffBlock),
    Inserted(DiffBlock),
    /// A block whose text changed; contains word-level ops for inline annotation.
    Modified(DiffBlock, Vec<WordOp>),
    /// A structured container (list, table, …) where only named slots changed.
    ModifiedSlots(DiffBlock, Vec<SlotDiff>),
}

/// The complete diff of two documents: a sequence of per-block operations and the
/// root page styles from the new document (used to wrap the final annotated content).
pub struct DiffResultFlat {
    pub block_ops: Vec<DiffResultOp>,
    pub root_styles: Styles,
}

/// A recursive sub-document diff for one named slot inside a structured container.
///
/// `path` identifies the slot within its parent element (e.g. `[ListItem(1)]`);
/// `ops` is the block-level diff of that slot's content, produced by recursively calling
/// `diff_content` on the old and new slot bodies. For a plain-text slot this will contain a
/// single `Modified` op; for a slot whose body has nested structure (e.g. a list item that
/// itself contains a sub-list) it will contain multiple ops that preserve that structure.
#[derive(Clone)]
pub struct SlotDiff {
    pub path: Vec<SlotStep>,
    pub ops: Vec<DiffResultOp>,
}

impl DiffResultFlat {
    pub fn modification_log(&self) -> String {
        let mut log = String::new();
        log_ops(&mut log, &[], &self.block_ops);
        log
    }
}

fn log_ops(log: &mut String, slot_path_prefix: &[SlotStep], ops: &[DiffResultOp]) {
    for (index, op) in ops.iter().enumerate() {
        match op {
            DiffResultOp::Equal(_) => {}
            DiffResultOp::Deleted(content) => {
                let kind = if slot_path_prefix.is_empty() {
                    "delete".to_string()
                } else {
                    format!("delete in slot {:?}", slot_path_prefix)
                };
                push_log_entry(
                    log,
                    index,
                    &kind,
                    &[("text", content.content.plain_text().to_string())],
                );
            }
            DiffResultOp::Inserted(content) => {
                let kind = if slot_path_prefix.is_empty() {
                    "insert".to_string()
                } else {
                    format!("insert in slot {:?}", slot_path_prefix)
                };
                push_log_entry(
                    log,
                    index,
                    &kind,
                    &[("text", content.content.plain_text().to_string())],
                );
            }
            DiffResultOp::Modified(new_block, word_ops) => {
                let deletes = collect_word_op_text(word_ops, |op| match op {
                    WordOp::Delete(tokens) => Some(tokens),
                    _ => None,
                });
                let inserts = collect_word_op_text(word_ops, |op| match op {
                    WordOp::Insert(tokens) => Some(tokens),
                    _ => None,
                });
                if slot_path_prefix.is_empty() {
                    push_log_entry(
                        log,
                        index,
                        "modify",
                        &[
                            ("block", new_block.content.plain_text().to_string()),
                            ("deleted", deletes),
                            ("inserted", inserts),
                        ],
                    );
                } else {
                    push_log_entry(
                        log,
                        index,
                        "modify slot",
                        &[
                            ("slot", format!("{slot_path_prefix:?}")),
                            ("deleted", deletes),
                            ("inserted", inserts),
                        ],
                    );
                }
            }
            DiffResultOp::ModifiedSlots(_, slot_diffs) => {
                for slot_diff in slot_diffs {
                    let mut sub_prefix = slot_path_prefix.to_vec();
                    sub_prefix.extend_from_slice(&slot_diff.path);
                    log_ops(log, &sub_prefix, &slot_diff.ops);
                }
            }
        }
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

pub fn diff_content(old: &Content, new: &Content) -> DiffResultFlat {
    let old_blocks = extract_block_units(old);
    let new_blocks = extract_block_units(new);
    let raw = diff_block_units_raw(&old_blocks, &new_blocks);
    let matched = match_edit_zones(raw);

    let block_ops = matched
        .into_iter()
        .map(|op| match op {
            BlockOp::Equal(_, new_block) => DiffResultOp::Equal(new_block),
            BlockOp::Delete(old_block) => DiffResultOp::Deleted(old_block),
            BlockOp::Insert(new_block) => DiffResultOp::Inserted(new_block),
            BlockOp::Replace(old_block, new_block) => {
                let old_tokens = extract_words(&old_block.content);
                let new_tokens = extract_words(&new_block.content);
                let word_ops = diff_words(&old_tokens, &new_tokens);
                if has_textual_word_change(&word_ops) {
                    DiffResultOp::Modified(new_block, word_ops)
                } else {
                    DiffResultOp::Equal(new_block)
                }
            }
        })
        .collect();

    DiffResultFlat {
        block_ops,
        root_styles: root_page_styles(new),
    }
}

fn root_page_styles(content: &Content) -> Styles {
    if let Some(styled) = content.to_packed::<StyledElem>()
        && styled.child.to_packed::<SequenceElem>().is_some()
    {
        return page_styles(&styled.styles);
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
                .then(|| page_styles(&styled.styles))
        })
        .find(|styles| !styles.is_empty())
        .unwrap_or_default()
}

fn page_styles(styles: &Styles) -> Styles {
    let mut result: Styles = styles
        .iter()
        .filter(|style| is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect();
    if !result.is_empty() {
        sanitize_page_marginals(&mut result);
    }
    result
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
// Tree-shaped diff types
// ──────────────────────────────────────────────────────────────────────────────

/// Tree-shaped diff result.
pub struct DiffResult {
    pub blocks: Vec<DiffNode>,
    pub root_styles: Styles,
}

pub struct DiffNode {
    pub node: AnnotatedContent,
    pub status: NodeStatus,
    /// Per-slot children, populated when status is `HasChangedDescendants`.
    pub children: Vec<DiffNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeStatus {
    Unchanged,
    HasChangedDescendants,
    Deleted,
    Inserted,
    Modified(Vec<WordOp>),
}

impl DiffResult {
    pub fn modification_log(&self) -> String {
        let mut log = String::new();
        for (index, node) in self.blocks.iter().enumerate() {
            log_diff_node(&mut log, node, index);
        }
        log
    }
}

fn log_diff_node(log: &mut String, node: &DiffNode, index: usize) {
    match &node.status {
        NodeStatus::Unchanged => {}
        NodeStatus::Deleted => push_log_entry(
            log,
            index,
            "delete",
            &[("text", node.node.realized.plain_text().to_string())],
        ),
        NodeStatus::Inserted => push_log_entry(
            log,
            index,
            "insert",
            &[("text", node.node.realized.plain_text().to_string())],
        ),
        NodeStatus::Modified(word_ops) => {
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
                    ("block", node.node.realized.plain_text().to_string()),
                    ("deleted", deletes),
                    ("inserted", inserts),
                ],
            );
        }
        NodeStatus::HasChangedDescendants => {
            for (ci, child) in node.children.iter().enumerate() {
                log_diff_node(log, child, ci);
            }
        }
    }
}

pub fn diff_annotated(old: &AnnotatedContent, new: &AnnotatedContent) -> DiffResult {
    let flat = diff_content(&old.realized, &new.realized);
    let blocks = flat
        .block_ops
        .into_iter()
        .map(|op| diff_result_op_to_node(op, new))
        .collect();
    DiffResult {
        blocks,
        root_styles: flat.root_styles,
    }
}

fn diff_result_op_to_node(op: DiffResultOp, new_ac: &AnnotatedContent) -> DiffNode {
    match op {
        DiffResultOp::Equal(block) => DiffNode {
            node: find_or_wrap_annotated(&block.content, new_ac),
            status: NodeStatus::Unchanged,
            children: vec![],
        },
        DiffResultOp::Deleted(block) => DiffNode {
            node: AnnotatedContent {
                realized: block.content.clone(),
                annotation: Annotation::default(),
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
            let node = find_or_wrap_annotated(&block.content, new_ac);
            // Phase A scaffolding: each SlotDiff's `path` is discarded and child nodes are
            // approximated from the ops. Slot identity will be wired from `annotation.slots`
            // in Phase B.
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
                })
                .collect();
            DiffNode {
                node,
                status: NodeStatus::HasChangedDescendants,
                children,
            }
        }
    }
}

/// Wrap a block-level `content` (from `extract_block_units`) as an `AnnotatedContent`
/// for the new tree-shaped diff result.
///
/// In an earlier draft this function also searched the annotated tree for a node
/// whose `realized` matched `content` and returned a clone of that node (so that
/// the resulting `DiffNode` could carry `semantic_kind` / `slots` from the
/// annotated tree). That path is currently disabled: the realized content stored
/// in the annotated tree carries introspector state (locators, tag references)
/// from the original realize pass, and feeding it back into `layout_document`
/// causes the layout engine to hang. The block content produced by
/// `extract_block_units` goes through `apply_block_styles` which rebuilds the
/// wrapping styles into a fresh `StyledElem`, breaking those references — so we
/// always wrap the block content directly.
///
/// The Phase A statuses (`Unchanged`, `Inserted`, `Modified`) only read
/// `node.realized`, so dropping the annotation here costs nothing in the current
/// pipeline. When `HasChangedDescendants` is wired up in Phase B, the matched-node
/// path will need to be re-introduced — but it will need to surface the
/// annotation without re-using the stored realized content.
fn find_or_wrap_annotated(content: &Content, _root: &AnnotatedContent) -> AnnotatedContent {
    AnnotatedContent {
        realized: content.clone(),
        annotation: Annotation::default(),
        children: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::model::HeadingElem;
    use typst::text::TextElem;

    fn seq(items: impl IntoIterator<Item = Content>) -> Content {
        Content::sequence(items)
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
        assert!(result.blocks.iter().all(|n| matches!(n.status, NodeStatus::Unchanged)));
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
    fn diff_content_on_paragraph_with_inline_styling_produces_single_modified_op() {
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

        let result = diff_content(&old, &new);

        // All edits should be captured in a single top-level Modified op, not
        // fragmented into multiple ops or nested ModifiedSlots at different depths.
        assert_eq!(result.block_ops.len(), 1, "expected 1 block op, got {}", result.block_ops.len());
        match &result.block_ops[0] {
            DiffResultOp::Modified(_, word_ops) => {
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
            DiffResultOp::Equal(_) => panic!("expected Modified, got Equal"),
            DiffResultOp::Deleted(_) => panic!("expected Modified, got Deleted"),
            DiffResultOp::Inserted(_) => panic!("expected Modified, got Inserted"),
            DiffResultOp::ModifiedSlots(_, _) => {
                panic!("expected Modified, got ModifiedSlots — fragmentation bug regression")
            }
        }
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
    fn large_atomic_content_splits_into_words() {
        use typst::model::StrongElem;

        let text = "alpha beta gamma ".repeat(40);
        let strong = Content::new(StrongElem::new(TextElem::packed(text.as_str())));
        let tokens = extract_words(&strong);

        assert!(tokens.len() > 1);
        assert!(tokens.iter().any(|t| t.text == "alpha"));
        assert!(tokens.iter().all(|t| !t.content.is::<StrongElem>()));
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

    // --- diff_content tests ---

    #[test]
    fn diff_content_detects_word_change() {
        let old = seq([TextElem::packed("The fox jumps.")]);
        let new = seq([TextElem::packed("The fox leaps.")]);
        let result = diff_content(&old, &new);
        let has_word_change = result.block_ops.iter().any(|op| match op {
            DiffResultOp::Modified(_, word_ops) => word_ops
                .iter()
                .any(|w| matches!(w, WordOp::Delete(_)) || matches!(w, WordOp::Insert(_))),
            _ => false,
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

        assert!(texts.contains(&"Hello,"));
        assert!(texts.contains(&"café"));
        assert!(texts.contains(&"世界!"));
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

        let log = diff_content(&old, &new).modification_log();

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
