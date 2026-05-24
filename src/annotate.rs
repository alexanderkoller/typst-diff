//! Convert a [`DiffResultFlat`] into an annotated Typst [`Content`] tree ready for rendering.
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

use typst::foundations::Content;
use typst::foundations::{SequenceElem, StyleChain, StyledElem};
use typst::layout::Abs;
use typst::math::{CancelElem, EquationElem};
use typst::model::{HeadingElem, ParElem};
use typst::text::{SpaceElem, StrikeElem, TextElem};
use typst::visualize::{Color, Stroke};

use crate::content_slots::{extract_slots, replace_slot};
use crate::diff::{DiffBlock, DiffResultFlat, DiffResultOp, WordOp};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}
fn blue() -> Color {
    Color::from_u8(0, 100, 220, 255)
}

/// Build the annotated document from a [`DiffResultFlat`].
///
/// `compact_substitutions`: when `true`, substitution pairs (adjacent delete+insert)
/// are shown as blue insertions only — the red strikethrough is suppressed. Useful
/// for authors who want to see "what it says now" without visual noise from deleted text.
pub fn build_annotated_content(result: &DiffResultFlat, compact_substitutions: bool) -> Content {
    let mut groups: Vec<Content> = Vec::new();
    let mut current_blocks: Vec<Content> = Vec::new();
    let mut current_page_styles = None;

    for block in build_annotated_blocks(&result.block_ops, compact_substitutions) {
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

/// Annotate a sequence of block ops, returning one [`DiffBlock`] per input op.
///
/// This is the recursable kernel used both at the top level (by
/// [`build_annotated_content`]) and when annotating slot sub-documents.
/// Unlike the top-level function it does NOT apply page-style grouping — that
/// only happens at the document root.
pub(crate) fn build_annotated_blocks(
    ops: &[DiffResultOp],
    compact_substitutions: bool,
) -> Vec<DiffBlock> {
    ops.iter()
        .map(|op| match op {
            DiffResultOp::Equal(c) => c.clone(),

            DiffResultOp::Inserted(c) => DiffBlock {
                content: if c.content.plain_text().is_empty() {
                    c.content.clone()
                } else {
                    apply_fill_inside(&c.content, green())
                },
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

            DiffResultOp::ModifiedSlots(new_block, slot_diffs) => DiffBlock {
                content: replace_modified_slots(
                    &new_block.content,
                    slot_diffs,
                    compact_substitutions,
                )
                .unwrap_or_else(|| new_block.content.clone()),
                page_styles: new_block.page_styles.clone(),
            },
        })
        .collect()
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

/// Strip all structure from `content` and return a single `TextElem` with its plain text.
///
/// Deleted blocks are flattened to avoid re-triggering heading counters, TOC entries,
/// and other document-level side effects that block elements carry.
fn plain_content(content: &Content) -> Content {
    let text = content.plain_text();
    TextElem::packed(text.as_str())
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

/// Apply `fill` to the text content of `content` without wrapping the outermost
/// structural element in a `StyledElem`.
///
/// For slot-bearing elements (lists, tables, figures, …) the fill is applied to
/// each slot's content individually, so the outer element (e.g. `ListElem`) stays
/// as a bare block — preserving Typst's tight list/table spacing between consecutive
/// elements of the same type.  For elements with no slots (plain text, headings,
/// raw blocks, …) the method falls back to wrapping in `StyledElem`, which is fine
/// because those types don't rely on consecutive-block spacing heuristics.
fn apply_fill_inside(content: &Content, fill: Color) -> Content {
    let slots = extract_slots(content);
    if slots.is_empty() {
        return content.clone().styled(TextElem::fill.set(fill.into()));
    }
    let mut result = content.clone();
    for slot in slots {
        let colored = slot.content.styled(TextElem::fill.set(fill.into()));
        if let Some(next) = replace_slot(&result, &slot.path, colored) {
            result = next;
        }
    }
    result
}

fn replace_modified_slots(
    template: &Content,
    slot_diffs: &[crate::diff::SlotDiff],
    compact_substitutions: bool,
) -> Option<Content> {
    let mut content = template.clone();
    for slot_diff in slot_diffs {
        let sub_blocks = build_annotated_blocks(&slot_diff.ops, compact_substitutions);
        let replacement = Content::sequence(sub_blocks.into_iter().map(|b| b.content));
        content = replace_slot(&content, &slot_diff.path, replacement)?;
    }
    Some(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_slots::SlotStep;
    use crate::diff::{DiffResultFlat, DiffResultOp, SlotDiff, Token, WordOp};
    use typst::foundations::{NativeElement, Packed};
    use typst::model::{HeadingElem, ListElem, ListItem};
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
        let result = DiffResultFlat {
            block_ops: vec![DiffResultOp::Inserted(block(TextElem::packed(
                "New paragraph",
            )))],
            root_styles: Default::default(),
        };
        let content = build_annotated_content(&result, false);
        assert!(!content.is_empty());
    }

    #[test]
    fn inserted_list_block_stays_bare_list_not_styled_wrapper() {
        // An inserted ListElem must NOT be wrapped in a top-level StyledElem.
        // If it were, Typst's tight spacing between consecutive ListElem blocks
        // would break, producing paragraph-sized gaps (the #19/#69 spacing bug).
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("New item"))),
        ]));
        let blocks = build_annotated_blocks(
            &[DiffResultOp::Inserted(block(list))],
            false,
        );
        assert_eq!(blocks.len(), 1);
        // The block content must be a ListElem, not a StyledElem wrapping a ListElem.
        assert!(
            blocks[0].content.is::<ListElem>(),
            "inserted ListElem should remain a bare ListElem, got: {:?}",
            blocks[0].content.func().name()
        );
        // Text must still be present.
        assert!(blocks[0].content.plain_text().contains("New item"));
    }

    #[test]
    fn modified_block_contains_strike_for_deletion() {
        let result = DiffResultFlat {
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
    fn deleted_heading_keeps_heading_formatting() {
        let result = DiffResultFlat {
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
            found_heading,
            "deleted heading should keep heading formatting"
        );
        assert!(found_old_text, "deleted block text should remain visible");
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
    }

    #[test]
    fn compact_substitutions_drop_deleted_text_and_color_inserted_text() {
        let result = DiffResultFlat {
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

        let compact = build_annotated_content(&result, true);

        assert_eq!(count_elem::<StrikeElem>(&compact), 0);
        assert!(compact.plain_text().contains("new"));
        assert!(!compact.plain_text().contains("old"));
    }

    #[test]
    fn modified_heading_preserves_heading_element() {
        let heading = Content::new(HeadingElem::new(TextElem::packed("New heading")));
        let result = DiffResultFlat {
            block_ops: vec![DiffResultOp::Modified(
                block(heading),
                vec![
                    WordOp::Delete(vec![word_token("Old")]),
                    WordOp::Insert(vec![word_token("New")]),
                    WordOp::Equal(vec![word_token(" heading")]),
                ],
            )],
            root_styles: Default::default(),
        };

        let annotated = build_annotated_content(&result, false);

        assert_eq!(count_elem::<HeadingElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn modified_list_slot_preserves_list_and_unchanged_items() {
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Better"))),
            Packed::new(ListItem::new(TextElem::packed("Gamma"))),
        ]));
        let result = DiffResultFlat {
            block_ops: vec![DiffResultOp::ModifiedSlots(
                block(list),
                vec![SlotDiff {
                    path: vec![SlotStep::ListItem(1)],
                    ops: vec![DiffResultOp::Modified(
                        block(TextElem::packed("Better")),
                        vec![
                            WordOp::Delete(vec![word_token("Beta")]),
                            WordOp::Insert(vec![word_token("Better")]),
                        ],
                    )],
                }],
            )],
            root_styles: Default::default(),
        };

        let annotated = build_annotated_content(&result, false);
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<ListElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Alpha"), "{plain}");
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Gamma"), "{plain}");
    }

    #[test]
    fn modified_table_slot_preserves_table_structure() {
        use typst::model::{TableCell, TableChild, TableElem, TableItem};

        let table = Content::new(TableElem::new(vec![
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("Metric"),
            )))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("New value"),
            )))),
        ]));
        let result = DiffResultFlat {
            block_ops: vec![DiffResultOp::ModifiedSlots(
                block(table),
                vec![SlotDiff {
                    path: vec![SlotStep::TableCell(1)],
                    ops: vec![DiffResultOp::Modified(
                        block(TextElem::packed("New value")),
                        vec![
                            WordOp::Delete(vec![word_token("Old")]),
                            WordOp::Insert(vec![word_token("New")]),
                            WordOp::Equal(vec![word_token(" value")]),
                        ],
                    )],
                }],
            )],
            root_styles: Default::default(),
        };

        let annotated = build_annotated_content(&result, false);

        assert_eq!(count_elem::<TableElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("Metric"));
        assert!(annotated.plain_text().contains("Old"));
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn nested_list_slot_preserves_both_list_levels_and_annotates_leaf() {
        // Two-level list: outer item 0 contains an inner list with "Beta" → "Better".
        let inner = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Better"))),
        ]));
        let outer_list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(inner.clone())),
            Packed::new(ListItem::new(TextElem::packed("Stable"))),
        ]));
        let result = DiffResultFlat {
            block_ops: vec![DiffResultOp::ModifiedSlots(
                block(outer_list),
                vec![SlotDiff {
                    path: vec![SlotStep::ListItem(0)],
                    ops: vec![DiffResultOp::ModifiedSlots(
                        block(inner),
                        vec![SlotDiff {
                            path: vec![SlotStep::ListItem(0)],
                            ops: vec![DiffResultOp::Modified(
                                block(TextElem::packed("Better")),
                                vec![
                                    WordOp::Delete(vec![word_token("Beta")]),
                                    WordOp::Insert(vec![word_token("Better")]),
                                ],
                            )],
                        }],
                    )],
                }],
            )],
            root_styles: Default::default(),
        };

        let annotated = build_annotated_content(&result, false);
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<ListElem>(&annotated), 2, "both list levels preserved");
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Stable"), "{plain}");
    }

    #[test]
    fn mixed_body_inline_change_detected_and_nested_structure_preserved() {
        // Item body: inline paragraph "old title" followed by a nested list.
        // Only the inline paragraph changes; nested list stays the same.
        use crate::diff::diff_content;
        use typst::model::ParbreakElem;

        let inner = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Leaf"))),
        ]));
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

        let result = diff_content(&old, &new);
        let annotated = build_annotated_content(&result, false);
        let plain = annotated.plain_text();

        // The nested ListElem must survive in the output.
        assert!(count_elem::<ListElem>(&annotated) >= 1, "nested list preserved");
        // The inline change must appear.
        assert!(plain.contains("old"), "{plain}");
        assert!(plain.contains("new"), "{plain}");
        // The unchanged leaf must still be present.
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
        // ParbreakElem has empty plain_text; wrapping it in StyledElem(green) would
        // make Typst treat it as a generic styled block and insert paragraph-level
        // vertical space instead of tight inter-bullet spacing (corpus #19 bug).
        use typst::model::ParbreakElem;

        let parbreak = Content::new(ParbreakElem::new());
        assert!(parbreak.plain_text().is_empty(), "sanity: ParbreakElem has no text");

        let blocks = build_annotated_blocks(
            &[DiffResultOp::Inserted(block(parbreak))],
            false,
        );
        assert_eq!(blocks.len(), 1);
        assert!(
            blocks[0].content.is::<ParbreakElem>(),
            "inserted ParbreakElem must remain bare, not wrapped in StyledElem, got: {:?}",
            blocks[0].content.func().name()
        );
    }

    #[test]
    fn inserted_visible_block_still_gets_green_fill() {
        // Verify the empty-text guard doesn't suppress coloring on visible content.
        let text = TextElem::packed("Visible text");
        assert!(!text.plain_text().is_empty(), "sanity: TextElem has text");

        let blocks = build_annotated_blocks(
            &[DiffResultOp::Inserted(block(text))],
            false,
        );
        assert_eq!(blocks.len(), 1);
        // The block must NOT be a bare TextElem — it must have been processed by
        // apply_fill_inside (which wraps in StyledElem with green fill).
        assert!(
            !blocks[0].content.is::<typst::text::TextElem>(),
            "inserted visible block should be styled/colored, not a bare TextElem"
        );
    }
}
