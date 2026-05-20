use std::hash::{Hash, Hasher};

use similar::{Algorithm, DiffOp, capture_diff_slices};
use typst::foundations::{Content, SequenceElem, StyleChain};
use typst::math::EquationElem;
use typst::model::{HeadingElem, ParbreakElem};
use typst::text::{RawElem, SpaceElem, TextElem};

/// Segment a Content tree into block-level units.
pub fn extract_blocks(content: &Content) -> Vec<Content> {
    let children: Vec<Content> = if let Some(seq) = content.to_packed::<SequenceElem>() {
        seq.children.clone()
    } else {
        return vec![content.clone()];
    };

    let mut blocks: Vec<Content> = Vec::new();
    let mut para: Vec<Content> = Vec::new();

    for child in children {
        if child.is::<ParbreakElem>() {
            flush_para(&mut para, &mut blocks);
        } else if child.is::<HeadingElem>() || child.is::<RawElem>() || is_display_equation(&child) {
            flush_para(&mut para, &mut blocks);
            blocks.push(child);
        } else if is_known_inline(&child) {
            para.push(child);
        } else {
            flush_para(&mut para, &mut blocks);
            blocks.push(child);
        }
    }
    flush_para(&mut para, &mut blocks);
    blocks
}

fn flush_para(para: &mut Vec<Content>, blocks: &mut Vec<Content>) {
    let nonempty = para.iter().any(|c| !c.is::<SpaceElem>());
    if nonempty {
        blocks.push(Content::sequence(para.drain(..)));
    } else {
        para.clear();
    }
}

fn is_display_equation(c: &Content) -> bool {
    c.to_packed::<EquationElem>()
        .is_some_and(|eq| eq.block.get(StyleChain::default()))
}

fn is_known_inline(c: &Content) -> bool {
    use typst::model::{EmphElem, LinkElem, StrongElem};
    use typst::text::{
        HighlightElem, LinebreakElem, OverlineElem, SmartQuoteElem, StrikeElem, SubElem,
        SuperElem, UnderlineElem,
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
        || (c.is::<EquationElem>() && !is_display_equation(c))
}

/// A single diffable unit: either a word/space split from TextElem, or an atomic inline.
#[derive(Clone, Debug)]
pub struct Token {
    pub text: String,
    pub content: Content,
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool { self.text == other.text }
}
impl Eq for Token {}
impl PartialOrd for Token {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Token {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.text.cmp(&other.text) }
}
impl Hash for Token {
    fn hash<H: Hasher>(&self, state: &mut H) { self.text.hash(state) }
}

/// Extract a flat list of tokens from a block's inline content.
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
    } else if let Some(text_elem) = content.to_packed::<TextElem>() {
        let s = text_elem.text.as_str();
        let mut start = 0;
        let mut in_space = s.chars().next().map(|c| c.is_whitespace()).unwrap_or(false);
        for (i, ch) in s.char_indices() {
            if ch.is_whitespace() != in_space {
                let slice = &s[start..i];
                if !slice.is_empty() {
                    out.push(Token { text: slice.to_string(), content: TextElem::packed(slice) });
                }
                start = i;
                in_space = ch.is_whitespace();
            }
        }
        let tail = &s[start..];
        if !tail.is_empty() {
            out.push(Token { text: tail.to_string(), content: TextElem::packed(tail) });
        }
    } else if content.is::<SpaceElem>() {
        out.push(Token { text: " ".to_string(), content: content.clone() });
    } else {
        out.push(Token { text: content.plain_text().to_string(), content: content.clone() });
    }
}

/// Wrapper providing Eq + Hash + Ord for Content (which is PartialEq + Hash but not Eq/Ord).
#[derive(Clone)]
struct HashableContent(Content);
impl PartialEq for HashableContent {
    fn eq(&self, other: &Self) -> bool { self.0 == other.0 }
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
    fn hash<H: Hasher>(&self, state: &mut H) { self.0.hash(state) }
}

#[derive(Clone)]
pub enum BlockOp {
    Equal(Content, Content),
    Delete(Content),
    Insert(Content),
    Replace(Content, Content),
}

/// Raw block diff — produces Equal, Delete, Insert (no Replace).
pub fn diff_blocks_raw(old: &[Content], new: &[Content]) -> Vec<BlockOp> {
    let old_h: Vec<HashableContent> = old.iter().cloned().map(HashableContent).collect();
    let new_h: Vec<HashableContent> = new.iter().cloned().map(HashableContent).collect();
    let ops = capture_diff_slices(Algorithm::Myers, &old_h, &new_h);
    let mut result = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for i in 0..len {
                    result.push(BlockOp::Equal(old[old_index + i].clone(), new[new_index + i].clone()));
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for i in 0..old_len {
                    result.push(BlockOp::Delete(old[old_index + i].clone()));
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for i in 0..new_len {
                    result.push(BlockOp::Insert(new[new_index + i].clone()));
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
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

/// Pair adjacent Delete+Insert groups by similarity, converting matched pairs to Replace.
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
                let deletes: Vec<Content> = ops[start..i].iter()
                    .filter_map(|op| match op { BlockOp::Delete(c) => Some(c.clone()), _ => None })
                    .collect();
                let inserts: Vec<Content> = ops[start..i].iter()
                    .filter_map(|op| match op { BlockOp::Insert(c) => Some(c.clone()), _ => None })
                    .collect();
                pair_edit_zone(deletes, inserts, &mut result);
            }
        }
    }
    result
}

fn pair_edit_zone(deletes: Vec<Content>, inserts: Vec<Content>, out: &mut Vec<BlockOp>) {
    if deletes.is_empty() {
        out.extend(inserts.into_iter().map(BlockOp::Insert));
        return;
    }
    if inserts.is_empty() {
        out.extend(deletes.into_iter().map(BlockOp::Delete));
        return;
    }

    let mut used_inserts = vec![false; inserts.len()];
    let mut pairs: Vec<(Content, Content)> = Vec::new();
    let mut unpaired_del: Vec<Content> = Vec::new();

    for del in &deletes {
        let del_text = del.plain_text();
        let best = inserts.iter().enumerate()
            .filter(|(j, _)| !used_inserts[*j])
            .map(|(j, ins)| (j, similarity(del_text.as_str(), ins.plain_text().as_str())))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        match best {
            Some((j, sim)) if sim >= 0.3 => {
                used_inserts[j] = true;
                pairs.push((del.clone(), inserts[j].clone()));
            }
            _ => unpaired_del.push(del.clone()),
        }
    }

    let unpaired_ins: Vec<Content> = inserts.into_iter().enumerate()
        .filter(|(j, _)| !used_inserts[*j])
        .map(|(_, c)| c)
        .collect();

    out.extend(unpaired_del.into_iter().map(BlockOp::Delete));
    out.extend(unpaired_ins.into_iter().map(BlockOp::Insert));
    out.extend(pairs.into_iter().map(|(o, n)| BlockOp::Replace(o, n)));
}

fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() { return 1.0; }
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 { return 1.0; }
    1.0 - edit_distance(a, b) as f64 / max_len as f64
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m { dp[i][0] = i; }
    for j in 0..=n { dp[0][j] = j; }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i-1] == b[j-1] { dp[i-1][j-1] }
                       else { 1 + dp[i-1][j].min(dp[i][j-1]).min(dp[i-1][j-1]) };
        }
    }
    dp[m][n]
}

#[derive(Clone, Debug)]
pub enum WordOp {
    Equal(Vec<Token>),
    Delete(Vec<Token>),
    Insert(Vec<Token>),
}

/// Diff two token sequences, coalescing adjacent same-tag ops.
pub fn diff_words(old: &[Token], new: &[Token]) -> Vec<WordOp> {
    let raw_ops = capture_diff_slices(Algorithm::Myers, old, new);
    let mut result: Vec<WordOp> = Vec::new();

    for op in raw_ops {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                coalesce(&mut result, WordOp::Equal(old[old_index..old_index + len].to_vec()));
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                coalesce(&mut result, WordOp::Delete(old[old_index..old_index + old_len].to_vec()));
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                coalesce(&mut result, WordOp::Insert(new[new_index..new_index + new_len].to_vec()));
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                coalesce(&mut result, WordOp::Delete(old[old_index..old_index + old_len].to_vec()));
                coalesce(&mut result, WordOp::Insert(new[new_index..new_index + new_len].to_vec()));
            }
        }
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

pub enum DiffResultOp {
    Equal(Content),
    Deleted(Content),
    Inserted(Content),
    Modified(Vec<WordOp>),
}

pub struct DiffResult {
    pub block_ops: Vec<DiffResultOp>,
}

pub fn diff_content(old: &Content, new: &Content) -> DiffResult {
    let old_blocks = extract_blocks(old);
    let new_blocks = extract_blocks(new);
    let raw = diff_blocks_raw(&old_blocks, &new_blocks);
    let matched = match_edit_zones(raw);

    let block_ops = matched.into_iter().map(|op| match op {
        BlockOp::Equal(_, new_block) => DiffResultOp::Equal(new_block),
        BlockOp::Delete(old_block) => DiffResultOp::Deleted(old_block),
        BlockOp::Insert(new_block) => DiffResultOp::Inserted(new_block),
        BlockOp::Replace(old_block, new_block) => {
            let old_tokens = extract_words(&old_block);
            let new_tokens = extract_words(&new_block);
            DiffResultOp::Modified(diff_words(&old_tokens, &new_tokens))
        }
    }).collect();

    DiffResult { block_ops }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::model::HeadingElem;
    use typst::text::TextElem;

    fn seq(items: impl IntoIterator<Item = Content>) -> Content {
        Content::sequence(items)
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
        assert_eq!(blocks.len(), 2);
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
        assert_eq!(blocks.len(), 2);
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
        let para = seq([TextElem::packed("before "), strong, TextElem::packed(" after")]);
        let tokens = extract_words(&para);
        assert!(tokens.iter().any(|t| t.text == "bold" || t.content.is::<StrongElem>()));
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

    // --- diff_words tests ---

    #[test]
    fn changed_word_produces_delete_and_insert() {
        let old = extract_words(&TextElem::packed("The quick brown fox jumps."));
        let new = extract_words(&TextElem::packed("The quick brown fox leaps."));
        let ops = diff_words(&old, &new);
        assert!(ops.iter().any(|op| matches!(op, WordOp::Delete(_))), "expected delete op");
        assert!(ops.iter().any(|op| matches!(op, WordOp::Insert(_))), "expected insert op");
    }

    #[test]
    fn identical_words_all_equal() {
        let tokens = extract_words(&TextElem::packed("Hello world."));
        let ops = diff_words(&tokens, &tokens.clone());
        assert!(ops.iter().all(|op| matches!(op, WordOp::Equal(_))));
    }

    // --- diff_content tests ---

    #[test]
    fn diff_content_detects_word_change() {
        use typst::model::ParbreakElem;
        let old = seq([TextElem::packed("The fox jumps.")]);
        let new = seq([TextElem::packed("The fox leaps.")]);
        let result = diff_content(&old, &new);
        let has_word_change = result.block_ops.iter().any(|op| match op {
            DiffResultOp::Modified(word_ops) => word_ops.iter().any(|w| {
                matches!(w, WordOp::Delete(_)) || matches!(w, WordOp::Insert(_))
            }),
            _ => false,
        });
        assert!(has_word_change);
    }
}
