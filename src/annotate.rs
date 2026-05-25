//! Convert a tree-shaped diff into annotated Typst [`Content`] ready for rendering.
//!
//! # Colour conventions
//!
//! | Change type       | Colour                  | Technique                    |
//! |-------------------|-------------------------|------------------------------|
//! | Inserted block    | Green text fill         | `TextElem::fill`             |
//! | Deleted block     | Red strikethrough       | `StrikeElem` on plain text   |
//! | Deleted equation  | Red cancel              | `CancelElem` inside equation |
//! | Modified word ins | Green (or blue if compact) | `TextElem::fill`           |
//! | Modified word del | Red strikethrough       | `StrikeElem`                 |
//!
//! # Page-style grouping
//!
//! Blocks that share the same `page_styles` are accumulated into a group and then
//! wrapped together in a single `styled_with_map(page_styles)` call. A new group is
//! started whenever the page styles change. This preserves `#set page(…)` boundaries
//! (margins, headers, footers) across section breaks in the diff output.

use typst::foundations::{Content, Smart, Styles};
use typst::foundations::{SequenceElem, StyleChain, StyledElem};
use typst::layout::{Abs, BlockBody, BlockElem, PageElem, Rel};
use typst::math::{CancelElem, EquationElem};
use typst::model::{EnumElem, HeadingElem, ListElem, ParElem, ParbreakElem};
use typst::text::{SpaceElem, StrikeElem, TextElem};
use typst::visualize::{Color, Stroke};

use crate::container_ops;
use crate::diff::{
    DiffBlock, DiffBlockEdit, DiffRegionEdit, EditContent, PageRegionKind, RealizedEdit,
    RegionPath, WordOp,
};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}
fn blue() -> Color {
    Color::from_u8(0, 100, 220, 255)
}

fn flush_group(
    groups: &mut Vec<Content>,
    blocks: &mut Vec<Content>,
    page_styles: Option<typst::foundations::Styles>,
) {
    if blocks.is_empty() {
        return;
    }

    let group = Content::sequence(blocks.drain(..));
    groups.push(match page_styles {
        Some(styles) => group.styled_with_map(styles),
        None => group,
    });
}

/// Turn a [`WordOp`] sequence into a flat inline `Content` sequence with colour annotations.
///
/// Deleted tokens become red `StrikeElem` (equations use `CancelElem`). Inserted tokens
/// become green. In compact mode, inserted tokens that are adjacent to a delete are
/// coloured blue and their delete sibling is dropped entirely. A thin space separator is
/// inserted between a delete run and its following insert run when the boundary would
/// otherwise join two non-whitespace tokens.
fn annotated_inline_content(word_ops: &[WordOp], compact_substitutions: bool) -> Content {
    let mut inline: Vec<Content> = Vec::new();
    for (i, wop) in word_ops.iter().enumerate() {
        match wop {
            WordOp::Equal(tokens) => {
                for t in tokens {
                    inline.push(t.content.clone());
                }
            }
            WordOp::Insert(tokens) => {
                let prev = i.checked_sub(1).and_then(|j| word_ops.get(j));
                let next = word_ops.get(i + 1);
                let adjacent_delete = prev.is_some_and(|op| matches!(op, WordOp::Delete(_)))
                    || next.is_some_and(|op| matches!(op, WordOp::Delete(_)));
                let color = if compact_substitutions && adjacent_delete {
                    blue()
                } else {
                    green()
                };
                let joined = changed_token_sequence(
                    tokens,
                    prev.and_then(deleted_tokens),
                    compact_substitutions,
                );
                inline.push(joined.styled(TextElem::fill.set(color.into())));
            }
            WordOp::Delete(tokens) => {
                let prev = i.checked_sub(1).and_then(|j| word_ops.get(j));
                let next = word_ops.get(i + 1);
                let is_substitution = compact_substitutions
                    && (prev.is_some_and(|op| matches!(op, WordOp::Insert(_)))
                        || next.is_some_and(|op| matches!(op, WordOp::Insert(_))));
                if !is_substitution {
                    inline.push(Content::sequence(tokens.iter().map(deleted_token_content)));
                }
            }
        }
    }
    Content::sequence(inline)
}

fn deleted_tokens(op: &WordOp) -> Option<&[crate::diff::Token]> {
    match op {
        WordOp::Delete(tokens) => Some(tokens),
        _ => None,
    }
}

fn changed_token_sequence(
    tokens: &[crate::diff::Token],
    previous_delete: Option<&[crate::diff::Token]>,
    compact_substitutions: bool,
) -> Content {
    let mut content: Vec<Content> = Vec::new();
    if !compact_substitutions && previous_delete.is_some_and(|prev| needs_separator(prev, tokens)) {
        content.push(SpaceElem::shared().clone());
    }
    content.extend(tokens.iter().map(|t| t.content.clone()));
    Content::sequence(content)
}

fn needs_separator(left: &[crate::diff::Token], right: &[crate::diff::Token]) -> bool {
    let Some(left_text) = left.last().map(|token| token.text.as_str()) else {
        return false;
    };
    let Some(right_text) = right.first().map(|token| token.text.as_str()) else {
        return false;
    };
    left_text
        .chars()
        .next_back()
        .is_some_and(|ch| !ch.is_whitespace())
        && right_text
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_whitespace())
}

fn deleted_token_content(token: &crate::diff::Token) -> Content {
    if let Some(equation) = token.content.to_packed::<EquationElem>() {
        let body = equation
            .body
            .clone()
            .styled(TextElem::fill.set(red().into()));
        let cancelled = Content::new(
            CancelElem::new(body).with_stroke(Stroke::from_pair(red(), Abs::pt(0.6).into())),
        );
        return Content::new(
            EquationElem::new(cancelled).with_block(equation.block.get(StyleChain::default())),
        );
    }

    let content = if token.content.plain_text().is_empty() {
        TextElem::packed(token.text.as_str())
    } else {
        token.content.clone()
    };
    let colored = content.styled(TextElem::fill.set(red().into()));
    Content::new(StrikeElem::new(colored))
}

/// Graft `replacement` into the innermost text-bearing position of `template`.
///
/// This preserves the block's outer styling (e.g. heading level, custom paragraph
/// styles) while swapping out only the inline text content. The search recurses through
/// `ParElem`, `StyledElem`, and all-inline `SequenceElem` wrappers.
/// Returns `None` if no suitable injection site is found.
fn replace_text_container(template: &Content, replacement: &Content) -> Option<Content> {
    let mut content = template.clone();

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = replacement.clone();
        return Some(content);
    }

    if let Some(heading) = content.to_packed_mut::<HeadingElem>() {
        heading.body = replacement.clone();
        return Some(content);
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>()
        && let Some(child) = replace_text_container(&styled.child, replacement)
    {
        styled.child = child;
        return Some(content);
    }

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        if seq.children.iter().all(is_inlineish) {
            seq.children = replacement
                .to_packed::<SequenceElem>()
                .map(|seq| seq.children.clone())
                .unwrap_or_else(|| vec![replacement.clone()].into_iter().collect());
            return Some(content);
        }

        for child in &mut seq.children {
            if let Some(replaced) = replace_text_container(child, replacement) {
                *child = replaced;
                return Some(content);
            }
        }
    }

    None
}

fn is_inlineish(content: &Content) -> bool {
    !content.is::<ParElem>() && content.to_packed::<SequenceElem>().is_none()
}

/// Apply `fill` to the text content of `content` at the outer block level.
///
/// The realized-edit pipeline uses this for inserted edit payloads so structural
/// whitespace nodes remain bare.
fn apply_fill_inside(content: &Content, fill: Color) -> Content {
    let mut content = content.clone();

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq
            .children
            .iter()
            .map(|child| apply_fill_inside(child, fill))
            .collect();
        return content;
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = apply_fill_inside(&styled.child, fill);
        return content;
    }

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = apply_fill_inside(&par.body, fill);
        return content;
    }

    if let Some(heading) = content.to_packed_mut::<HeadingElem>() {
        heading.body = apply_fill_inside(&heading.body, fill);
        return content;
    }

    if let Some(list) = content.to_packed_mut::<ListElem>() {
        for item in &mut list.children {
            item.body = apply_fill_inside(&item.body, fill);
        }
        return content;
    }

    if let Some(enm) = content.to_packed_mut::<EnumElem>() {
        for item in &mut enm.children {
            item.body = apply_fill_inside(&item.body, fill);
        }
        return content;
    }

    content.styled(TextElem::fill.set(fill.into()))
}

fn apply_delete_inside(content: &Content) -> Content {
    let mut content = content.clone();

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq.children.iter().map(apply_delete_inside).collect();
        return content;
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = apply_delete_inside(&styled.child);
        return content;
    }

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = apply_delete_inside(&par.body);
        return content;
    }

    if let Some(heading) = content.to_packed_mut::<HeadingElem>() {
        heading.body = apply_delete_inside(&heading.body);
        return content;
    }

    if let Some(block) = content.to_packed_mut::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        block
            .body
            .set(Some(BlockBody::Content(apply_delete_inside(&body))));
        return content;
    }

    if let Some(list) = content.to_packed_mut::<ListElem>() {
        for item in &mut list.children {
            item.body = apply_delete_inside(&item.body);
        }
        return content;
    }

    if let Some(enm) = content.to_packed_mut::<EnumElem>() {
        for item in &mut enm.children {
            item.body = apply_delete_inside(&item.body);
        }
        return content;
    }

    if content.plain_text().is_empty() {
        return content;
    }

    let colored = content.styled(TextElem::fill.set(red().into()));
    Content::new(StrikeElem::new(colored))
}

/// Build annotated content from the new tree-shaped [`crate::diff::DiffResult`].
pub fn build_annotated_content_from_tree(
    result: &crate::diff::DiffResult,
    compact_substitutions: bool,
) -> Content {
    let mut groups: Vec<Content> = Vec::new();
    let mut current_blocks: Vec<Content> = Vec::new();
    let mut current_page_styles: Option<typst::foundations::Styles> = None;

    for mut annotated_block in result
        .blocks
        .iter()
        .map(|block| annotate_block_edit(block, compact_substitutions))
    {
        apply_region_edits_to_page_styles(
            &mut annotated_block.page_styles,
            &result.regions,
            compact_substitutions,
        );
        if current_page_styles
            .as_ref()
            .is_some_and(|s| s != &annotated_block.page_styles)
        {
            flush_group(&mut groups, &mut current_blocks, current_page_styles.take());
        }
        current_page_styles.get_or_insert_with(|| annotated_block.page_styles.clone());
        current_blocks.push(annotated_block.content);
    }
    flush_group(&mut groups, &mut current_blocks, current_page_styles);
    let mut root_styles = result.root_styles.clone();
    apply_region_edits_to_root_styles(&mut root_styles, &result.regions, compact_substitutions);
    Content::sequence(groups).styled_with_map(root_styles)
}

fn annotate_block_edit(block: &DiffBlockEdit, compact: bool) -> DiffBlock {
    DiffBlock {
        content: apply_edits_to_base(&block.base, &block.edits, compact),
        page_styles: block.page_styles.clone(),
    }
}

pub(crate) fn apply_edits_to_base(
    base: &crate::annotated::AnnotatedContent,
    edits: &[RealizedEdit],
    compact: bool,
) -> Content {
    let mut node = base.clone();
    let mut index = 0;
    while index < edits.len() {
        if let Some((before, anchor)) = insertion_anchor(&edits[index]) {
            let mut end = index + 1;
            while end < edits.len()
                && insertion_anchor(&edits[end]).is_some_and(|(next_before, next_anchor)| {
                    next_before == before && next_anchor == anchor
                })
            {
                end += 1;
            }
            for edit in edits[index..end].iter().rev() {
                apply_realized_edit(&mut node, edit, compact);
            }
            index = end;
        } else {
            apply_realized_edit(&mut node, &edits[index], compact);
            index += 1;
        }
    }
    effective_realized_content(&node)
}

fn apply_region_edits_to_root_styles(
    root_styles: &mut Styles,
    regions: &[DiffRegionEdit],
    compact: bool,
) {
    for region in regions {
        let content = apply_edits_to_base(&region.base, &region.edits, compact);
        match region.path {
            RegionPath::RootPage(kind) => set_root_page_region(root_styles, kind, content),
        }
    }
}

fn apply_region_edits_to_page_styles(
    page_styles: &mut Styles,
    regions: &[DiffRegionEdit],
    compact: bool,
) {
    if page_styles.is_empty() {
        return;
    }
    for region in regions {
        let RegionPath::RootPage(kind) = region.path;
        if !page_styles_has_region(page_styles, kind) {
            continue;
        }
        let content = apply_edits_to_base(&region.base, &region.edits, compact);
        set_root_page_region(page_styles, kind, content);
    }
}

fn page_styles_has_region(page_styles: &Styles, kind: PageRegionKind) -> bool {
    match kind {
        PageRegionKind::Header => page_styles.has(PageElem::header),
        PageRegionKind::Footer => page_styles.has(PageElem::footer),
        PageRegionKind::Background => page_styles.has(PageElem::background),
        PageRegionKind::Foreground => page_styles.has(PageElem::foreground),
    }
}

fn set_root_page_region(root_styles: &mut Styles, kind: PageRegionKind, content: Content) {
    match kind {
        PageRegionKind::Header => {
            root_styles.push(PageElem::header.set(Smart::Custom(Some(marginal_content(content)))))
        }
        PageRegionKind::Footer => {
            root_styles.push(PageElem::footer.set(Smart::Custom(Some(marginal_content(content)))))
        }
        PageRegionKind::Background => root_styles.push(PageElem::background.set(Some(content))),
        PageRegionKind::Foreground => root_styles.push(PageElem::foreground.set(Some(content))),
    }
}

fn marginal_content(content: Content) -> Content {
    Content::new(
        BlockElem::new()
            .with_width(Smart::Custom(Rel::one()))
            .with_body(Some(BlockBody::Content(
                content.styled(ParElem::justify.set(false)),
            ))),
    )
}

fn insertion_anchor(edit: &RealizedEdit) -> Option<(bool, &[usize])> {
    match edit {
        RealizedEdit::InsertBefore { anchor, .. } => Some((true, anchor)),
        RealizedEdit::InsertAfter { anchor, .. } => Some((false, anchor)),
        _ => None,
    }
}

fn effective_realized_content(node: &crate::annotated::AnnotatedContent) -> Content {
    let surface = node
        .annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized);
    if !surface.is_empty() || node.children.is_empty() {
        return surface.clone();
    }
    Content::sequence(node.children.iter().map(effective_realized_content))
}

fn apply_realized_edit(
    node: &mut crate::annotated::AnnotatedContent,
    edit: &RealizedEdit,
    compact: bool,
) {
    match edit {
        RealizedEdit::ReplaceAt { path, content } => {
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = replace_annotated_path_content(node, path, rendered.clone()) {
                node.annotation.patch_surface = Some(if modified_is_pure_insert(content) {
                    patched
                } else {
                    strip_leading_parbreak(patched)
                });
            }
            if let Some(child) = node.get_path_mut(path) {
                child.realized = rendered;
                child.annotation.patch_surface = None;
            }
        }
        RealizedEdit::InsertBefore { anchor, content } => {
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = insert_annotated_path_content(node, anchor, rendered, true) {
                node.annotation.patch_surface =
                    Some(insertion_patch_surface(node, anchor, patched));
            }
        }
        RealizedEdit::InsertAfter { anchor, content } => {
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = insert_annotated_path_content(node, anchor, rendered, false) {
                node.annotation.patch_surface =
                    Some(insertion_patch_surface(node, anchor, patched));
            }
        }
        RealizedEdit::Append { content } => {
            let rendered = render_edit_content(content, compact);
            let base = effective_realized_content(node);
            node.annotation.patch_surface = Some(Content::sequence([base, rendered]));
        }
        RealizedEdit::WholeBlock(content) => {
            node.realized = render_edit_content(content, compact);
            node.annotation.patch_surface = None;
            node.children.clear();
        }
    }
}

fn modified_is_pure_insert(content: &EditContent) -> bool {
    matches!(content, EditContent::Modified { word_ops, .. } if word_ops.iter().all(|op| !matches!(op, WordOp::Delete(_))) && word_ops.iter().any(|op| matches!(op, WordOp::Insert(_))))
}

fn strip_leading_parbreak(content: Content) -> Content {
    let mut content = content;
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        if seq
            .children
            .first()
            .is_some_and(|child| child.is::<ParbreakElem>())
        {
            seq.children.remove(0);
        }
        seq.children = seq
            .children
            .iter()
            .cloned()
            .map(strip_leading_parbreak)
            .collect();
        return content;
    }
    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = strip_leading_parbreak(styled.child.clone());
    }
    content
}

fn insertion_patch_surface(
    node: &crate::annotated::AnnotatedContent,
    anchor: &[usize],
    patched: Content,
) -> Content {
    let nested_list_insert = anchor.len() > 1
        && !has_table_container(&patched)
        && (matches!(
            node.annotation.semantic_kind,
            Some(crate::annotated::SemanticKind::List) | Some(crate::annotated::SemanticKind::Enum)
        ) || has_any_list_container(&patched)
            || node.annotation.semantic_kind.is_none());
    if nested_list_insert || has_nested_list_container(&patched) {
        Content::sequence([Content::new(ParbreakElem::new()), patched])
    } else {
        patched
    }
}

fn has_any_list_container(content: &Content) -> bool {
    let mut found = content.is::<ListElem>() || content.is::<EnumElem>();
    let _ = content.traverse::<_, ()>(&mut |child| {
        if child.is::<ListElem>() || child.is::<EnumElem>() {
            found = true;
            return std::ops::ControlFlow::Break(());
        }
        std::ops::ControlFlow::Continue(())
    });
    found
}

fn has_table_container(content: &Content) -> bool {
    use typst::layout::GridElem;
    use typst::model::TableElem;

    let mut found = content.is::<TableElem>() || content.is::<GridElem>();
    let _ = content.traverse::<_, ()>(&mut |child| {
        if child.is::<TableElem>() || child.is::<GridElem>() {
            found = true;
            return std::ops::ControlFlow::Break(());
        }
        std::ops::ControlFlow::Continue(())
    });
    found
}

fn has_nested_list_container(content: &Content) -> bool {
    let mut seen = false;
    let mut nested = false;
    let _ = content.traverse::<_, ()>(&mut |child| {
        if child.is::<ListElem>() || child.is::<EnumElem>() {
            if seen {
                nested = true;
                return std::ops::ControlFlow::Break(());
            }
            seen = true;
        }
        std::ops::ControlFlow::Continue(())
    });
    nested
}

fn render_edit_content(content: &EditContent, compact: bool) -> Content {
    match content {
        EditContent::Inserted(content) => {
            if content.plain_text().is_empty() {
                content.clone()
            } else {
                apply_fill_inside(content, green())
            }
        }
        EditContent::Deleted(content) => apply_delete_inside(content),
        EditContent::Modified { base, word_ops } => {
            let inline = annotated_inline_content(word_ops, compact);
            replace_text_container(base, &inline).unwrap_or(inline)
        }
        EditContent::Nested { base, edits } => apply_edits_to_base(base, edits, compact),
    }
}

fn replace_annotated_path_content(
    node: &crate::annotated::AnnotatedContent,
    path: &[usize],
    replacement: Content,
) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return Some(replacement);
    };
    let surface = patchable_surface_for_index(node, *index)?;
    let surface_child = container_ops::realized_child_contents(&surface)
        .get(*index)
        .cloned()?;
    let replaced_child = replace_content_path(&surface_child, rest, replacement)?;
    container_ops::replace_realized_child(&surface, *index, replaced_child)
}

fn replace_content_path(
    surface: &Content,
    path: &[usize],
    replacement: Content,
) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return Some(replacement);
    };
    let surface_child = container_ops::realized_child_contents(surface)
        .get(*index)
        .cloned()?;
    let replaced_child = replace_content_path(&surface_child, rest, replacement)?;
    container_ops::replace_realized_child(surface, *index, replaced_child)
}

fn insert_annotated_path_content(
    node: &crate::annotated::AnnotatedContent,
    path: &[usize],
    insertion: Content,
    before: bool,
) -> Option<Content> {
    let (index, rest) = path.split_first()?;
    if rest.is_empty() {
        return container_ops::insert_realized_child(
            render_surface(node),
            *index,
            insertion,
            before,
        );
    }
    let surface = render_surface(node);
    let surface_child = container_ops::realized_child_contents(surface)
        .get(*index)
        .cloned()?;
    let patched_child = insert_content_path(&surface_child, rest, insertion, before)?;
    container_ops::replace_realized_child(surface, *index, patched_child)
}

fn insert_content_path(
    surface: &Content,
    path: &[usize],
    insertion: Content,
    before: bool,
) -> Option<Content> {
    let (index, rest) = path.split_first()?;
    if rest.is_empty() {
        return container_ops::insert_realized_child(surface, *index, insertion, before);
    }
    let surface_child = container_ops::realized_child_contents(surface)
        .get(*index)
        .cloned()?;
    let patched_child = insert_content_path(&surface_child, rest, insertion, before)?;
    container_ops::replace_realized_child(surface, *index, patched_child)
}

fn render_surface(node: &crate::annotated::AnnotatedContent) -> &Content {
    node.annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized)
}

fn patchable_surface_for_index(
    node: &crate::annotated::AnnotatedContent,
    index: usize,
) -> Option<Content> {
    let surface = render_surface(node);
    if container_ops::realized_child_contents(surface)
        .get(index)
        .is_some()
    {
        return Some(surface.clone());
    }
    (index < node.children.len())
        .then(|| Content::sequence(node.children.iter().map(effective_realized_content)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotated::{AnnotatedContent, Annotation, annotate_realized};
    use crate::diff::{DiffBlockEdit, DiffResult, EditContent, RealizedEdit, Token, WordOp};
    use typst::foundations::{NativeElement, Packed};
    use typst::layout::BlockElem;
    use typst::model::{HeadingElem, ListElem, ListItem};
    use typst::text::TextElem;

    fn word_token(s: &str) -> Token {
        Token {
            text: s.to_string(),
            content: TextElem::packed(s),
        }
    }

    fn annotated(content: Content) -> AnnotatedContent {
        AnnotatedContent {
            realized: content,
            annotation: Annotation::default(),
            children: vec![],
        }
    }

    fn render(base: Content, edits: Vec<RealizedEdit>, compact: bool) -> Content {
        let result = DiffResult {
            blocks: vec![DiffBlockEdit {
                base: annotated(base),
                edits,
                page_styles: Default::default(),
            }],
            root_styles: Default::default(),
            regions: vec![],
        };
        build_annotated_content_from_tree(&result, compact)
    }

    fn modified(base: Content, word_ops: Vec<WordOp>) -> EditContent {
        EditContent::Modified { base, word_ops }
    }

    fn whole(content: EditContent) -> Vec<RealizedEdit> {
        vec![RealizedEdit::WholeBlock(content)]
    }

    fn replace_at(path: Vec<usize>, content: EditContent) -> Vec<RealizedEdit> {
        vec![RealizedEdit::ReplaceAt { path, content }]
    }

    fn count_elem<T: NativeElement>(content: &Content) -> usize {
        let mut count = 0;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if c.is::<T>() {
                count += 1;
            }
            std::ops::ControlFlow::Continue(())
        });
        count
    }

    #[test]
    fn inserted_block_wrapped_green() {
        let content = render(
            TextElem::packed("New paragraph"),
            whole(EditContent::Inserted(TextElem::packed("New paragraph"))),
            false,
        );
        assert!(!content.is_empty());
    }

    #[test]
    fn modified_block_contains_strike_for_deletion() {
        let content = render(
            TextElem::packed("The new text."),
            whole(modified(
                TextElem::packed("The new text."),
                vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ],
            )),
            false,
        );
        assert!(!content.is_empty());
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
    }

    #[test]
    fn deleted_heading_keeps_heading_formatting() {
        let heading = Content::new(HeadingElem::new(TextElem::packed("Old heading")));
        let content = render(heading.clone(), whole(EditContent::Deleted(heading)), false);

        assert_eq!(count_elem::<HeadingElem>(&content), 1);
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
        assert!(content.plain_text().contains("Old heading"));
    }

    #[test]
    fn deleted_semantic_heading_block_keeps_block_formatting() {
        let heading_block = Content::new(typst::layout::BlockElem::new().with_body(Some(
            typst::layout::BlockBody::Content(TextElem::packed("Old heading")),
        )));
        let content = render(
            heading_block.clone(),
            whole(EditContent::Deleted(heading_block)),
            false,
        );

        assert_eq!(count_elem::<BlockElem>(&content), 1);
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
        assert!(content.plain_text().contains("Old heading"));
    }

    #[test]
    fn compact_substitutions_drop_deleted_text_and_color_inserted_text() {
        let compact = render(
            TextElem::packed("The new text."),
            whole(modified(
                TextElem::packed("The new text."),
                vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ],
            )),
            true,
        );

        assert_eq!(count_elem::<StrikeElem>(&compact), 0);
        assert!(compact.plain_text().contains("new"));
        assert!(!compact.plain_text().contains("old"));
    }

    #[test]
    fn modified_heading_preserves_heading_element() {
        let heading = Content::new(HeadingElem::new(TextElem::packed("New heading")));
        let annotated = render(
            heading.clone(),
            whole(modified(
                heading,
                vec![
                    WordOp::Delete(vec![word_token("Old")]),
                    WordOp::Insert(vec![word_token("New")]),
                    WordOp::Equal(vec![word_token(" heading")]),
                ],
            )),
            false,
        );

        assert_eq!(count_elem::<HeadingElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn modified_list_child_preserves_list_and_unchanged_items() {
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Better"))),
            Packed::new(ListItem::new(TextElem::packed("Gamma"))),
        ]));
        let annotated = render(
            list,
            replace_at(
                vec![1],
                modified(
                    TextElem::packed("Better"),
                    vec![
                        WordOp::Delete(vec![word_token("Beta")]),
                        WordOp::Insert(vec![word_token("Better")]),
                    ],
                ),
            ),
            false,
        );
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<ListElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Alpha"), "{plain}");
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Gamma"), "{plain}");
    }

    #[test]
    fn modified_table_child_preserves_table_structure() {
        use typst::model::{TableCell, TableChild, TableElem, TableItem};

        let table = Content::new(TableElem::new(vec![
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("Metric"),
            )))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("New value"),
            )))),
        ]));
        let annotated = render(
            table,
            replace_at(
                vec![1],
                modified(
                    TextElem::packed("New value"),
                    vec![
                        WordOp::Delete(vec![word_token("Old")]),
                        WordOp::Insert(vec![word_token("New")]),
                        WordOp::Equal(vec![word_token(" value")]),
                    ],
                ),
            ),
            false,
        );

        assert_eq!(count_elem::<TableElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("Metric"));
        assert!(annotated.plain_text().contains("Old"));
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn nested_list_child_preserves_both_list_levels_and_annotates_leaf() {
        let inner = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            TextElem::packed("Better"),
        ))]));
        let outer_list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(inner)),
            Packed::new(ListItem::new(TextElem::packed("Stable"))),
        ]));

        let annotated = render(
            outer_list,
            replace_at(
                vec![0, 0],
                modified(
                    TextElem::packed("Better"),
                    vec![
                        WordOp::Delete(vec![word_token("Beta")]),
                        WordOp::Insert(vec![word_token("Better")]),
                    ],
                ),
            ),
            false,
        );
        let plain = annotated.plain_text();

        assert_eq!(
            count_elem::<ListElem>(&annotated),
            2,
            "both list levels preserved"
        );
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Stable"), "{plain}");
    }

    #[test]
    fn mixed_body_inline_change_detected_and_nested_structure_preserved() {
        use crate::diff::diff_annotated;
        use typst::model::ParbreakElem;

        let inner = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            TextElem::packed("Leaf"),
        ))]));
        let body_old = Content::sequence([
            TextElem::packed("old title"),
            Content::new(ParbreakElem::new()),
            inner.clone(),
        ]);
        let body_new = Content::sequence([
            TextElem::packed("new title"),
            Content::new(ParbreakElem::new()),
            inner.clone(),
        ]);
        let old = Content::new(ListElem::new(vec![Packed::new(ListItem::new(body_old))]));
        let new = Content::new(ListElem::new(vec![Packed::new(ListItem::new(body_new))]));

        let result = diff_annotated(
            &annotate_realized(&old, &old),
            &annotate_realized(&new, &new),
        );
        let annotated = build_annotated_content_from_tree(&result, false);
        let plain = annotated.plain_text();

        assert!(
            count_elem::<ListElem>(&annotated) >= 1,
            "nested list preserved"
        );
        assert!(plain.contains("old"), "{plain}");
        assert!(plain.contains("new"), "{plain}");
        assert!(plain.contains("Leaf"), "{plain}");
    }

    #[test]
    fn annotated_inline_content_inserts_separator_between_adjacent_changes() {
        let inline = annotated_inline_content(
            &[
                WordOp::Delete(vec![word_token("old")]),
                WordOp::Insert(vec![word_token("new")]),
            ],
            false,
        );

        assert_eq!(inline.plain_text(), "old new");
        assert_eq!(count_elem::<StrikeElem>(&inline), 1);
    }

    #[test]
    fn inserted_parbreak_is_not_wrapped_in_styled_elem() {
        use typst::model::ParbreakElem;

        let parbreak = Content::new(ParbreakElem::new());
        assert!(
            parbreak.plain_text().is_empty(),
            "sanity: ParbreakElem has no text"
        );

        let content = render(
            parbreak.clone(),
            whole(EditContent::Inserted(parbreak)),
            false,
        );
        assert!(
            content.is::<ParbreakElem>(),
            "inserted ParbreakElem must remain bare, got: {:?}",
            content.func().name()
        );
    }

    #[test]
    fn annotate_applies_whole_block_insert_edit() {
        let content = render(
            TextElem::packed("new text"),
            whole(EditContent::Inserted(TextElem::packed("new text"))),
            false,
        );
        assert!(!content.is_empty());
    }

    #[test]
    fn inserted_visible_block_still_gets_green_fill() {
        let text = TextElem::packed("Visible text");
        assert!(!text.plain_text().is_empty(), "sanity: TextElem has text");

        let content = render(text.clone(), whole(EditContent::Inserted(text)), false);
        assert!(
            !content.is::<typst::text::TextElem>(),
            "inserted visible block should be styled/colored, not a bare TextElem"
        );
    }
}
