//! Named content "slots" — addressable text-bearing sub-positions inside structured elements.
//!
//! A **slot** is a leaf text-bearing position identified by a [`Vec<SlotStep>`] path from
//! the element root. [`replace_slot`] takes a path and a replacement `Content` and writes
//! it back into a clone of the original tree.
//!
//! The old `extract_slots` / `collect_slots` machinery has been removed; slot paths are
//! now supplied by the annotated tree (`annotation.slots`) rather than extracted from
//! realized content.

use typst::foundations::{Content, SequenceElem, StyleChain, StyledElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridChild, GridElem, GridItem, PadElem,
    PlaceElem, StackChild, StackElem,
};
use typst::model::{
    EnumElem, EnumItem, FigureElem, FootnoteBody, FootnoteElem, ListElem, ListItem, ParElem,
    ParbreakElem, QuoteElem, TableChild, TableElem, TableItem, TermItem, TermsElem,
};
use typst::text::SpaceElem;
use typst::visualize::{CircleElem, EllipseElem, RectElem};

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
    let mut content = container.clone();
    rebuild_grid_in_realized(&mut content, &cell_bodies)?;
    Some(content)
}

fn rebuild_grid_in_realized(content: &mut Content, cell_bodies: &[Content]) -> Option<()> {
    if content.to_packed::<GridElem>().is_some() {
        return rebuild_grid_body_cells(content, cell_bodies);
    }
    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        return rebuild_grid_in_realized(&mut styled.child, cell_bodies);
    }
    if let Some(block) = content.to_packed_mut::<BlockElem>() {
        let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) else {
            return None;
        };
        let mut body = body;
        rebuild_grid_in_realized(&mut body, cell_bodies)?;
        block.body.set(Some(BlockBody::Content(body)));
        return Some(());
    }
    None
}

fn rebuild_grid_body_cells(content: &mut Content, cell_bodies: &[Content]) -> Option<()> {
    use typst::foundations::Packed;
    use typst::layout::GridCell;

    let grid = content.to_packed_mut::<GridElem>()?;
    let mut new_children: Vec<GridChild> = Vec::with_capacity(grid.children.len());
    let mut idx = 0usize;

    for child in &grid.children {
        match child {
            GridChild::Item(GridItem::Cell(cell)) => {
                if idx < cell_bodies.len() {
                    let mut new_cell = (**cell).clone();
                    new_cell.body = cell_bodies[idx].clone();
                    new_children.push(GridChild::Item(GridItem::Cell(Packed::new(new_cell))));
                    idx += 1;
                } else {
                    new_children.push(child.clone());
                }
            }
            other => new_children.push(other.clone()),
        }
    }

    while idx < cell_bodies.len() {
        let cell = GridCell::new(cell_bodies[idx].clone());
        new_children.push(GridChild::Item(GridItem::Cell(Packed::new(cell))));
        idx += 1;
    }

    grid.children = new_children;
    Some(())
}

pub fn replace_subtree(
    haystack: &Content,
    needle: &Content,
    replacement: Content,
) -> Option<Content> {
    if haystack == needle {
        return Some(replacement);
    }

    let mut content = haystack.clone();
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        for child in &mut seq.children {
            if let Some(patched) = replace_subtree(child, needle, replacement.clone()) {
                *child = patched;
                return Some(content);
            }
        }
        return None;
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        if let Some(patched) = replace_subtree(&styled.child, needle, replacement) {
            styled.child = patched;
            return Some(content);
        }
        return None;
    }

    None
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
    let (step, rest) = path.split_first()?;
    let mut content = template.clone();

    match step {
        SlotStep::SequenceChild(index) => {
            let seq = content.to_packed_mut::<SequenceElem>()?;
            let child = seq.children.get(*index)?;
            seq.children[*index] = replace_slot(child, rest, replacement)?;
            Some(content)
        }
        SlotStep::StyledChild => {
            let styled = content.to_packed_mut::<StyledElem>()?;
            styled.child = replace_slot(&styled.child, rest, replacement)?;
            Some(content)
        }
        SlotStep::ParBody => {
            let par = content.to_packed_mut::<ParElem>()?;
            if rest.is_empty() {
                par.body = replacement;
            } else {
                par.body = replace_slot(&par.body, rest, replacement)?;
            }
            Some(content)
        }
        SlotStep::ItemBody if rest.is_empty() => {
            if let Some(item) = content.to_packed_mut::<ListItem>() {
                item.body = replacement;
                return Some(content);
            }
            if let Some(item) = content.to_packed_mut::<EnumItem>() {
                item.body = replacement;
                return Some(content);
            }
            None
        }
        SlotStep::ListItem(index) if rest.is_empty() => {
            let list = content.to_packed_mut::<ListElem>()?;
            list.children.get_mut(*index)?.body = replacement;
            Some(content)
        }
        SlotStep::EnumItem(index) if rest.is_empty() => {
            let enm = content.to_packed_mut::<EnumElem>()?;
            enm.children.get_mut(*index)?.body = replacement;
            Some(content)
        }
        SlotStep::Term(index) if rest.is_empty() => {
            if *index == 0
                && let Some(item) = content.to_packed_mut::<TermItem>()
            {
                item.term = replacement;
                return Some(content);
            }
            let terms = content.to_packed_mut::<TermsElem>()?;
            terms.children.get_mut(*index)?.term = replacement;
            Some(content)
        }
        SlotStep::TermDescription(index) if rest.is_empty() => {
            if *index == 0
                && let Some(item) = content.to_packed_mut::<TermItem>()
            {
                item.description = replacement;
                return Some(content);
            }
            let terms = content.to_packed_mut::<TermsElem>()?;
            terms.children.get_mut(*index)?.description = replacement;
            Some(content)
        }
        SlotStep::FigureBody if rest.is_empty() => {
            let figure = content.to_packed_mut::<FigureElem>()?;
            figure.body = replacement;
            Some(content)
        }
        SlotStep::FigureCaption if rest.is_empty() => {
            let figure = content.to_packed_mut::<FigureElem>()?;
            figure.caption.as_option_mut().as_mut()?.as_mut()?.body = replacement;
            Some(content)
        }
        SlotStep::FootnoteBody if rest.is_empty() => {
            let footnote = content.to_packed_mut::<FootnoteElem>()?;
            footnote.body = FootnoteBody::Content(replacement);
            Some(content)
        }
        SlotStep::QuoteBody if rest.is_empty() => {
            let quote = content.to_packed_mut::<QuoteElem>()?;
            quote.body = replacement;
            Some(content)
        }
        SlotStep::WrapperBody if rest.is_empty() => replace_wrapper_body(content, replacement),
        SlotStep::TableCell(index) if rest.is_empty() => {
            replace_table_cell(&mut content, *index, replacement)?;
            Some(content)
        }
        SlotStep::GridCell(index) if rest.is_empty() => {
            replace_grid_cell(&mut content, *index, replacement)?;
            Some(content)
        }
        SlotStep::StackChild(index) if rest.is_empty() => {
            let stack = content.to_packed_mut::<StackElem>()?;
            let child = stack.children.get_mut(*index)?;
            if let StackChild::Block(body) = child {
                *body = replacement;
                Some(content)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn replace_wrapper_body(mut content: Content, replacement: Content) -> Option<Content> {
    if let Some(elem) = content.to_packed_mut::<AlignElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<PadElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<PlaceElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<ColumnsElem>() {
        elem.body = replacement;
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<BoxElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<BlockElem>() {
        elem.body.set(Some(BlockBody::Content(replacement)));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<RectElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<CircleElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    if let Some(elem) = content.to_packed_mut::<EllipseElem>() {
        elem.body.set(Some(replacement));
        return Some(content);
    }
    None
}

fn replace_table_cell(content: &mut Content, target: usize, replacement: Content) -> Option<()> {
    let table = content.to_packed_mut::<TableElem>()?;
    let mut index = 0;
    replace_table_child_cell(&mut table.children, target, replacement, &mut index)
}

fn replace_table_child_cell(
    children: &mut [TableChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for child in children {
        let found = match child {
            TableChild::Header(header) => {
                replace_table_item_cell(&mut header.children, target, replacement.clone(), index)
            }
            TableChild::Footer(footer) => {
                replace_table_item_cell(&mut footer.children, target, replacement.clone(), index)
            }
            TableChild::Item(item) => {
                replace_one_table_item_cell(item, target, replacement.clone(), index)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn replace_table_item_cell(
    items: &mut [TableItem],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for item in items {
        if replace_one_table_item_cell(item, target, replacement.clone(), index).is_some() {
            return Some(());
        }
    }
    None
}

fn replace_one_table_item_cell(
    item: &mut TableItem,
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    if let TableItem::Cell(cell) = item {
        if *index == target {
            cell.body = replacement;
            return Some(());
        }
        *index += 1;
    }
    None
}

fn replace_grid_cell(content: &mut Content, target: usize, replacement: Content) -> Option<()> {
    let grid = content.to_packed_mut::<GridElem>()?;
    let mut index = 0;
    replace_grid_child_cell(&mut grid.children, target, replacement, &mut index)
}

fn replace_grid_child_cell(
    children: &mut [GridChild],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for child in children {
        let found = match child {
            GridChild::Header(header) => {
                replace_grid_item_cell(&mut header.children, target, replacement.clone(), index)
            }
            GridChild::Footer(footer) => {
                replace_grid_item_cell(&mut footer.children, target, replacement.clone(), index)
            }
            GridChild::Item(item) => {
                replace_one_grid_item_cell(item, target, replacement.clone(), index)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn replace_grid_item_cell(
    items: &mut [GridItem],
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    for item in items {
        if replace_one_grid_item_cell(item, target, replacement.clone(), index).is_some() {
            return Some(());
        }
    }
    None
}

fn replace_one_grid_item_cell(
    item: &mut GridItem,
    target: usize,
    replacement: Content,
    index: &mut usize,
) -> Option<()> {
    if let GridItem::Cell(cell) = item {
        if *index == target {
            cell.body = replacement;
            return Some(());
        }
        *index += 1;
    }
    None
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
        let replaced =
            replace_slot(&replaced, &[SlotStep::TermDescription(0)], text("New description"))
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
        use typst::layout::{GridCell, GridChild, GridElem, GridItem};

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
        use typst::layout::{GridCell, GridChild, GridElem, GridItem};

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
        use typst::layout::{GridCell, GridChild, GridElem, GridItem};
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

    #[test]
    fn replace_subtree_swaps_matching_node_inside_sequence() {
        let needle = text("inner");
        let haystack = Content::sequence([text("before"), needle.clone(), text("after")]);
        let patched = replace_subtree(&haystack, &needle, text("REPLACED")).unwrap();

        assert!(patched.plain_text().contains("REPLACED"));
        assert!(!patched.plain_text().contains("inner"));
        assert!(patched.plain_text().contains("before"));
        assert!(patched.plain_text().contains("after"));
    }

    #[test]
    fn replace_subtree_returns_none_when_needle_not_found() {
        let haystack = Content::sequence([text("a"), text("b")]);
        let needle = text("missing");
        assert!(replace_subtree(&haystack, &needle, text("Z")).is_none());
    }

    #[test]
    fn replace_subtree_walks_through_styled_wrapper() {
        use typst::visualize::Color;

        let needle = text("inner");
        let haystack = Content::sequence([text("a"), needle.clone()])
            .styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));
        let patched = replace_subtree(&haystack, &needle, text("Z")).unwrap();
        assert!(patched.plain_text().contains('Z'));
        assert!(!patched.plain_text().contains("inner"));
    }

    #[test]
    fn replace_subtree_at_root_matches_haystack_directly() {
        let needle = text("whole");
        let patched = replace_subtree(&needle, &needle, text("Z")).unwrap();
        assert_eq!(patched.plain_text(), "Z");
    }
}
