use typst::foundations::Content;
use typst::foundations::{SequenceElem, StyleChain, StyledElem};
use typst::layout::Abs;
use typst::math::{CancelElem, EquationElem};
use typst::model::{ParElem, TableChild, TableElem, TableItem};
use typst::text::{StrikeElem, TextElem};
use typst::visualize::{Color, Stroke};

use crate::diff::{DiffBlock, DiffResult, DiffResultOp, WordOp};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}
fn blue() -> Color {
    Color::from_u8(0, 100, 220, 255)
}

pub fn build_annotated_content(result: &DiffResult, compact_substitutions: bool) -> Content {
    let mut groups: Vec<Content> = Vec::new();
    let mut current_blocks: Vec<Content> = Vec::new();
    let mut current_page_styles = None;

    for op in &result.block_ops {
        let block = match op {
            DiffResultOp::Equal(c) => c.clone(),

            DiffResultOp::Inserted(c) => DiffBlock {
                content: c.content.clone().styled(TextElem::fill.set(green().into())),
                page_styles: c.page_styles.clone(),
            },

            DiffResultOp::Deleted(c) => {
                let colored = plain_content(&c.content).styled(TextElem::fill.set(red().into()));
                let struck = Content::new(StrikeElem::new(colored));
                DiffBlock {
                    content: replace_text_container(&c.content, &struck).unwrap_or(struck),
                    page_styles: c.page_styles.clone(),
                }
            }

            DiffResultOp::Modified(new_block, word_ops) => {
                let inline = annotated_inline_content(word_ops, compact_substitutions);
                DiffBlock {
                    content: replace_text_container(&new_block.content, &inline).unwrap_or(inline),
                    page_styles: new_block.page_styles.clone(),
                }
            }

            DiffResultOp::ModifiedTable(new_block, cell_diffs) => DiffBlock {
                content: replace_table_cells(&new_block.content, cell_diffs, compact_substitutions)
                    .unwrap_or_else(|| new_block.content.clone()),
                page_styles: new_block.page_styles.clone(),
            },
        };

        if current_page_styles
            .as_ref()
            .is_some_and(|styles| styles != &block.page_styles)
        {
            flush_group(&mut groups, &mut current_blocks, current_page_styles.take());
        }

        current_page_styles.get_or_insert_with(|| block.page_styles.clone());
        current_blocks.push(block.content);
    }

    flush_group(&mut groups, &mut current_blocks, current_page_styles);
    Content::sequence(groups).styled_with_map(result.root_styles.clone())
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

fn plain_content(content: &Content) -> Content {
    let text = content.plain_text();
    TextElem::packed(text.as_str())
}

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
                let color = if compact_substitutions && adjacent_delete { blue() } else { green() };
                let joined = Content::sequence(tokens.iter().map(|t| t.content.clone()));
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

fn replace_text_container(template: &Content, replacement: &Content) -> Option<Content> {
    let mut content = template.clone();

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = replacement.clone();
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

fn replace_table_cells(
    template: &Content,
    cell_diffs: &[crate::diff::TableCellDiff],
    compact_substitutions: bool,
) -> Option<Content> {
    let mut content = template.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>()
        && let Some(child) = replace_table_cells(&styled.child, cell_diffs, compact_substitutions)
    {
        styled.child = child;
        return Some(content);
    }

    let table = content.to_packed_mut::<TableElem>()?;
    let mut next_diff = 0;
    let mut cell_index = 0;
    replace_table_child_cells(
        &mut table.children,
        cell_diffs,
        &mut next_diff,
        &mut cell_index,
        compact_substitutions,
    );
    Some(content)
}

fn replace_table_child_cells(
    children: &mut [TableChild],
    cell_diffs: &[crate::diff::TableCellDiff],
    next_diff: &mut usize,
    cell_index: &mut usize,
    compact_substitutions: bool,
) {
    for child in children {
        match child {
            TableChild::Header(header) => {
                replace_table_item_cells(&mut header.children, cell_diffs, next_diff, cell_index, compact_substitutions)
            }
            TableChild::Footer(footer) => {
                replace_table_item_cells(&mut footer.children, cell_diffs, next_diff, cell_index, compact_substitutions)
            }
            TableChild::Item(item) => {
                replace_table_item_cell(item, cell_diffs, next_diff, cell_index, compact_substitutions)
            }
        }
    }
}

fn replace_table_item_cells(
    items: &mut [TableItem],
    cell_diffs: &[crate::diff::TableCellDiff],
    next_diff: &mut usize,
    cell_index: &mut usize,
    compact_substitutions: bool,
) {
    for item in items {
        replace_table_item_cell(item, cell_diffs, next_diff, cell_index, compact_substitutions);
    }
}

fn replace_table_item_cell(
    item: &mut TableItem,
    cell_diffs: &[crate::diff::TableCellDiff],
    next_diff: &mut usize,
    cell_index: &mut usize,
    compact_substitutions: bool,
) {
    let TableItem::Cell(cell) = item else {
        return;
    };

    if let Some(cell_diff) = cell_diffs.get(*next_diff)
        && cell_diff.index == *cell_index
    {
        let inline = annotated_inline_content(&cell_diff.word_ops, compact_substitutions);
        cell.body = replace_text_container(&cell.body, &inline).unwrap_or(inline);
        *next_diff += 1;
    }

    *cell_index += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffResult, DiffResultOp, Token, WordOp};
    use typst::text::TextElem;

    fn word_token(s: &str) -> Token {
        Token {
            text: s.to_string(),
            content: TextElem::packed(s),
        }
    }

    fn block(content: Content) -> DiffBlock {
        DiffBlock {
            content,
            page_styles: Default::default(),
        }
    }

    #[test]
    fn inserted_block_wrapped_green() {
        let result = DiffResult {
            block_ops: vec![DiffResultOp::Inserted(block(TextElem::packed(
                "New paragraph",
            )))],
            root_styles: Default::default(),
        };
        let content = build_annotated_content(&result, false);
        assert!(!content.is_empty());
    }

    #[test]
    fn modified_block_contains_strike_for_deletion() {
        let result = DiffResult {
            block_ops: vec![DiffResultOp::Modified(
                block(TextElem::packed("The new text.")),
                vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ],
            )],
            root_styles: Default::default(),
        };
        let content = build_annotated_content(&result, false);
        assert!(!content.is_empty());
        let mut found_strike = false;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if c.is::<StrikeElem>() {
                found_strike = true;
            }
            std::ops::ControlFlow::Continue(())
        });
        assert!(found_strike, "expected StrikeElem for deleted word");
    }

    #[test]
    fn deleted_block_is_rendered_as_plain_text() {
        use typst::model::HeadingElem;

        let result = DiffResult {
            block_ops: vec![DiffResultOp::Deleted(block(Content::new(
                HeadingElem::new(TextElem::packed("Old heading")),
            )))],
            root_styles: Default::default(),
        };
        let content = build_annotated_content(&result, false);

        let mut found_heading = false;
        let mut found_old_text = false;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if c.is::<HeadingElem>() {
                found_heading = true;
            }
            if let Some(text) = c.to_packed::<TextElem>()
                && text.text.as_str().contains("Old heading")
            {
                found_old_text = true;
            }
            std::ops::ControlFlow::Continue(())
        });

        assert!(
            !found_heading,
            "deleted block should not keep structural side effects"
        );
        assert!(found_old_text, "deleted block text should remain visible");
    }
}
