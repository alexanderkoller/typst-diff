//! Named content "slots" — addressable text-bearing sub-positions inside structured elements.
//!
//! A **slot** is a leaf text-bearing position identified by a [`Vec<SlotStep>`] path from
//! the element root. [`replace_slot`] takes a path and a replacement `Content` and writes
//! it back into a clone of the original tree.
//!
//! The old `extract_slots` / `collect_slots` machinery has been removed; slot paths are
//! now supplied by the annotated tree (`annotation.slots`) rather than extracted from
//! realized content.

use typst::foundations::{Content, SequenceElem, StyledElem};
use typst::model::{
    EnumElem, EnumItem, ListElem, ListItem, ParElem, ParbreakElem, TermItem, TermsElem,
};
use typst::text::SpaceElem;

/// One step in a path from a container element to a leaf slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotStep {
    SequenceChild(usize),
    StyledChild,
    ParBody,
    ItemBody,
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

/// Wrap consecutive bare `ListItem` / `EnumItem` / `TermItem` nodes into their
/// container elements (`ListElem`, `EnumElem`, `TermsElem`).
///
/// Typst's evaluator sometimes emits list items as siblings in a `SequenceElem`
/// rather than inside a `ListElem`. This normalization step ensures the content
/// tree always uses the container form that the annotated tree understands.
pub fn normalize_list_item_runs(content: Content) -> Content {
    let mut content = content;

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        let children: Vec<Content> = seq
            .children
            .iter()
            .cloned()
            .map(normalize_list_item_runs)
            .collect();
        seq.children = group_list_item_runs(children);
        return content;
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = normalize_list_item_runs(styled.child.clone());
        return content;
    }

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = normalize_list_item_runs(par.body.clone());
        return content;
    }

    content
}

fn group_list_item_runs(children: Vec<Content>) -> Vec<Content> {
    let mut grouped = Vec::new();
    let mut index = 0;

    while index < children.len() {
        if children[index].is::<ListItem>() {
            let span = children[index].span();
            let mut items = Vec::new();
            while index < children.len() {
                if children[index].is::<ListItem>() {
                    let child = children[index].clone();
                    items.push(child.into_packed::<ListItem>().unwrap());
                    index += 1;
                    continue;
                }
                if children[index].is::<SpaceElem>() || children[index].is::<ParbreakElem>() {
                    index += 1;
                    continue;
                }
                break;
            }
            grouped.push(Content::new(ListElem::new(items)).spanned(span));
        } else if children[index].is::<EnumItem>() {
            let span = children[index].span();
            let mut items = Vec::new();
            while index < children.len() {
                if children[index].is::<EnumItem>() {
                    let child = children[index].clone();
                    items.push(child.into_packed::<EnumItem>().unwrap());
                    index += 1;
                    continue;
                }
                if children[index].is::<SpaceElem>() || children[index].is::<ParbreakElem>() {
                    index += 1;
                    continue;
                }
                break;
            }
            grouped.push(Content::new(EnumElem::new(items)).spanned(span));
        } else if children[index].is::<TermItem>() {
            let span = children[index].span();
            let mut items = Vec::new();
            while index < children.len() {
                if children[index].is::<TermItem>() {
                    let child = children[index].clone();
                    items.push(child.into_packed::<TermItem>().unwrap());
                    index += 1;
                    continue;
                }
                if children[index].is::<SpaceElem>() || children[index].is::<ParbreakElem>() {
                    index += 1;
                    continue;
                }
                break;
            }
            grouped.push(Content::new(TermsElem::new(items)).spanned(span));
        } else {
            grouped.push(children[index].clone());
            index += 1;
        }
    }

    grouped
}

pub fn rebuild_realized_grid_with_cells(
    container: &Content,
    cell_bodies: Vec<Content>,
) -> Option<Content> {
    crate::container_ops::rebuild_realized_grid_with_cells(container, cell_bodies)
}

/// Write `replacement` into the slot identified by `path` inside a clone of `template`.
///
/// Returns `None` if the path is empty, the expected element type isn't found at any
/// step, or the index is out of range. On success returns a new `Content` tree with
/// only the target slot mutated.
pub fn replace_slot(
    template: &Content,
    path: &[SlotStep],
    replacement: Content,
) -> Option<Content> {
    crate::container_ops::replace_slot(template, path, replacement)
}

/// Graft `replacement` into the innermost inline-content position of `template`.
///
/// Like [`crate::annotate`]'s `replace_text_container` but operates on slot content
/// (which may already be a `ParElem`). Returns `None` if no injection site is found.
pub fn replace_inline_content(template: &Content, replacement: &Content) -> Option<Content> {
    let mut content = template.clone();

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = replacement.clone();
        return Some(content);
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>()
        && let Some(child) = replace_inline_content(&styled.child, replacement)
    {
        styled.child = child;
        return Some(content);
    }

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        if seq.children.iter().all(is_inlineish) {
            seq.children = replacement
                .to_packed::<SequenceElem>()
                .map(|seq| seq.children.clone())
                .unwrap_or_else(|| vec![replacement.clone()]);
            return Some(content);
        }
    }

    None
}

fn is_inlineish(content: &Content) -> bool {
    !content.is::<ParElem>() && content.to_packed::<SequenceElem>().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::Packed;
    use typst::model::QuoteElem;
    use typst::text::TextElem;

    fn text(s: &str) -> Content {
        TextElem::packed(s)
    }

    #[test]
    fn replace_slot_list_item_body() {
        let content = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
            Packed::new(ListItem::new(text("Gamma"))),
        ]));

        let replaced = replace_slot(&content, &[SlotStep::ListItem(1)], text("Better")).unwrap();
        assert_eq!(
            replaced.plain_text(),
            Content::new(ListElem::new(vec![
                Packed::new(ListItem::new(text("Alpha"))),
                Packed::new(ListItem::new(text("Better"))),
                Packed::new(ListItem::new(text("Gamma"))),
            ]))
            .plain_text()
        );
        // Original is unchanged
        assert!(content.plain_text().contains("Beta"));
    }

    #[test]
    fn replace_slot_enum_item_body() {
        let content = Content::new(EnumElem::new(vec![
            Packed::new(EnumItem::new(text("One"))),
            Packed::new(EnumItem::new(text("Two"))),
        ]));

        let replaced = replace_slot(&content, &[SlotStep::EnumItem(0)], text("First")).unwrap();
        assert!(replaced.plain_text().contains("First"));
        assert!(!replaced.plain_text().contains("One"));
    }

    #[test]
    fn replace_slot_term_and_description() {
        let content = Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
            text("API"),
            text("Old description"),
        ))]));

        let replaced = replace_slot(&content, &[SlotStep::Term(0)], text("SDK")).unwrap();
        let replaced = replace_slot(
            &replaced,
            &[SlotStep::TermDescription(0)],
            text("New description"),
        )
        .unwrap();
        assert!(replaced.plain_text().contains("SDK"));
        assert!(replaced.plain_text().contains("New description"));
        assert!(!replaced.plain_text().contains("API"));
        assert!(!replaced.plain_text().contains("Old description"));
    }

    #[test]
    fn replace_slot_quote_body() {
        let content = Content::new(QuoteElem::new(text("Old quote")));
        let replaced = replace_slot(&content, &[SlotStep::QuoteBody], text("New quote")).unwrap();
        assert_eq!(replaced.plain_text(), "New quote");
    }

    #[test]
    fn replace_slot_returns_none_for_invalid_paths() {
        let content = Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
            "Only",
        )))]));

        assert!(replace_slot(&content, &[], text("Nope")).is_none());
        assert!(replace_slot(&content, &[SlotStep::ListItem(3)], text("Nope")).is_none());
        assert!(replace_slot(&content, &[SlotStep::FigureCaption], text("Nope")).is_none());
    }

    #[test]
    fn normalize_list_item_runs_groups_items_and_preserves_siblings() {
        let content = Content::sequence([
            text("Before"),
            Content::new(ListItem::new(text("Alpha"))),
            Content::new(ListItem::new(text("Beta"))),
            text("After"),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        assert!(seq.children[0].plain_text().contains("Before"));
        assert!(seq.children[1].is::<ListElem>());
        assert!(seq.children[2].plain_text().contains("After"));
        // The grouped ListElem contains both list items
        assert!(seq.children[1].plain_text().contains("Alpha"));
        assert!(seq.children[1].plain_text().contains("Beta"));
    }

    #[test]
    fn replace_inline_content_targets_paragraph_styled_and_inline_sequence() {
        let replacement = Content::sequence([text("New"), text(" text")]);

        let par = Content::new(ParElem::new(text("Old")));
        assert_eq!(
            replace_inline_content(&par, &replacement)
                .unwrap()
                .plain_text(),
            "New text"
        );

        let styled = Content::sequence([text("Old"), text(" text")]).styled(
            typst::text::TextElem::fill.set(typst::visualize::Color::from_u8(1, 2, 3, 255).into()),
        );
        assert_eq!(
            replace_inline_content(&styled, &replacement)
                .unwrap()
                .plain_text(),
            "New text"
        );

        let inline_seq = Content::sequence([text("Old"), text(" text")]);
        assert_eq!(
            replace_inline_content(&inline_seq, &replacement)
                .unwrap()
                .plain_text(),
            "New text"
        );
    }

    #[test]
    fn rebuild_realized_grid_replaces_each_cell_body_in_order() {
        use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};

        let cells = vec![
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("A"))))),
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("B"))))),
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("C"))))),
        ];
        let grid = Content::new(GridElem::new(cells));
        let container = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(grid))));

        let rebuilt =
            rebuild_realized_grid_with_cells(&container, vec![text("X"), text("Y"), text("Z")])
                .unwrap();
        let plain = rebuilt.plain_text();
        assert!(plain.contains('X'));
        assert!(plain.contains('Y'));
        assert!(plain.contains('Z'));
        assert!(!plain.contains('A'));
        assert!(!plain.contains('B'));
        assert!(!plain.contains('C'));
    }

    #[test]
    fn rebuild_realized_grid_appends_extra_cells_at_end() {
        use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};

        let cells = vec![
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("A"))))),
            GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(text("B"))))),
        ];
        let grid = Content::new(GridElem::new(cells));
        let container = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(grid))));

        let rebuilt = rebuild_realized_grid_with_cells(
            &container,
            vec![text("X"), text("Y"), text("Z"), text("W")],
        )
        .unwrap();
        let plain = rebuilt.plain_text();
        assert!(plain.contains('X'));
        assert!(plain.contains('Y'));
        assert!(plain.contains('Z'));
        assert!(plain.contains('W'));
    }

    #[test]
    fn rebuild_realized_grid_descends_through_styled_block_wrappers() {
        use typst::layout::{BlockBody, BlockElem, GridCell, GridChild, GridElem, GridItem};
        use typst::visualize::Color;

        let cells = vec![GridChild::Item(GridItem::Cell(Packed::new(GridCell::new(
            text("A"),
        ))))];
        let grid = Content::new(GridElem::new(cells));
        let block = Content::new(BlockElem::new().with_body(Some(BlockBody::Content(grid))));
        let container = block.styled(TextElem::fill.set(Color::from_u8(0, 0, 0, 255).into()));

        let rebuilt = rebuild_realized_grid_with_cells(&container, vec![text("Z")]).unwrap();
        assert!(rebuilt.plain_text().contains('Z'));
    }

    #[test]
    fn rebuild_realized_grid_returns_none_when_no_grid_present() {
        let container = text("just text");
        assert!(rebuild_realized_grid_with_cells(&container, vec![text("X")]).is_none());
    }
}
