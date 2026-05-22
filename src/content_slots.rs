//! Named content "slots" — addressable text-bearing sub-positions inside structured elements.
//!
//! Many Typst elements (lists, tables, figures, …) contain multiple independent
//! text regions. The diff needs to identify *which* sub-region changed, not just that
//! the whole block changed.
//!
//! A **slot** is a leaf text-bearing position identified by a [`Vec<SlotStep>`] path from
//! the element root. [`extract_slots`] walks a `Content` tree and collects every slot.
//! [`replace_slot`] takes a path and a replacement `Content` and writes it back into a
//! clone of the original tree.
//!
//! # Element coverage
//!
//! Slots are extracted from: `ListElem` / `ListItem`, `EnumElem` / `EnumItem`,
//! `TermsElem` / `TermItem`, `FigureElem` (body + caption), `FootnoteElem`,
//! `QuoteElem`, `TableElem`, `GridElem`, `StackElem`, and single-body wrappers
//! (`AlignElem`, `PadElem`, `PlaceElem`, `ColumnsElem`, `BoxElem`, `BlockElem`,
//! `RectElem`, `CircleElem`, `EllipseElem`).

use typst::foundations::{Content, SequenceElem, StyleChain, StyledElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, GridChild, GridElem, GridItem, PadElem,
    PlaceElem, StackChild, StackElem,
};
use typst::model::{
    EnumElem, EnumItem, FigureElem, FootnoteBody, FootnoteElem, ListElem, ListItem, ParElem,
    QuoteElem, TableChild, TableElem, TableItem, TermItem, TermsElem,
};
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

/// A named text-bearing position inside a structured element.
///
/// `path` uniquely addresses the slot from the element root; `content` is the
/// current text payload at that address.
#[derive(Clone, Debug)]
pub struct ContentSlot {
    pub path: Vec<SlotStep>,
    pub content: Content,
}

/// Collect every named slot from `content` in document order.
///
/// Returns an empty vec for elements that have no addressable slots (e.g. plain
/// `TextElem`, `HeadingElem`, `RawElem`). Non-empty results are used by the diff
/// to attempt slot-level diffing before falling back to whole-block word-diffing.
pub fn extract_slots(content: &Content) -> Vec<ContentSlot> {
    let mut slots = Vec::new();
    collect_slots(content, &mut Vec::new(), &mut slots);
    slots
}

/// Returns `true` if `content` is a structured element that [`extract_slots`] knows
/// how to descend into.
///
/// Used in `eval.rs` to identify nodes that must be preserved before realization
/// (realization turns them into opaque layout output).
pub fn is_slot_container(content: &Content) -> bool {
    content.is::<ListElem>()
        || content.is::<ListItem>()
        || content.is::<EnumElem>()
        || content.is::<EnumItem>()
        || content.is::<TermsElem>()
        || content.is::<TermItem>()
        || content.is::<FigureElem>()
        || content.is::<FootnoteElem>()
        || content.is::<QuoteElem>()
        || content.is::<TableElem>()
        || content.is::<GridElem>()
        || content.is::<StackElem>()
        || content.is::<AlignElem>()
        || content.is::<PadElem>()
        || content.is::<PlaceElem>()
        || content.is::<ColumnsElem>()
        || content.is::<BoxElem>()
        || content.is::<BlockElem>()
        || content.is::<RectElem>()
        || content.is::<CircleElem>()
        || content.is::<EllipseElem>()
}

/// Wrap consecutive bare `ListItem` / `EnumItem` / `TermItem` nodes into their
/// container elements (`ListElem`, `EnumElem`, `TermsElem`).
///
/// Typst's evaluator sometimes emits list items as siblings in a `SequenceElem`
/// rather than inside a `ListElem`. This normalization step ensures the content
/// tree always uses the container form, which [`extract_slots`] understands.
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

    for slot in extract_slots(&content) {
        let normalized = normalize_list_item_runs(slot.content);
        if let Some(next) = replace_slot(&content, &slot.path, normalized) {
            content = next;
        }
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
            while index < children.len() && children[index].is::<ListItem>() {
                let child = children[index].clone();
                items.push(child.into_packed::<ListItem>().unwrap());
                index += 1;
            }
            grouped.push(Content::new(ListElem::new(items)).spanned(span));
        } else if children[index].is::<EnumItem>() {
            let span = children[index].span();
            let mut items = Vec::new();
            while index < children.len() && children[index].is::<EnumItem>() {
                let child = children[index].clone();
                items.push(child.into_packed::<EnumItem>().unwrap());
                index += 1;
            }
            grouped.push(Content::new(EnumElem::new(items)).spanned(span));
        } else if children[index].is::<TermItem>() {
            let span = children[index].span();
            let mut items = Vec::new();
            while index < children.len() && children[index].is::<TermItem>() {
                let child = children[index].clone();
                items.push(child.into_packed::<TermItem>().unwrap());
                index += 1;
            }
            grouped.push(Content::new(TermsElem::new(items)).spanned(span));
        } else {
            grouped.push(children[index].clone());
            index += 1;
        }
    }

    grouped
}

fn collect_slots(content: &Content, prefix: &mut Vec<SlotStep>, slots: &mut Vec<ContentSlot>) {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        for (index, child) in seq.children.iter().enumerate() {
            prefix.push(SlotStep::SequenceChild(index));
            collect_slots(child, prefix, slots);
            prefix.pop();
        }
        return;
    }

    if let Some(styled) = content.to_packed::<StyledElem>() {
        prefix.push(SlotStep::StyledChild);
        collect_slots(&styled.child, prefix, slots);
        prefix.pop();
        return;
    }

    if let Some(par) = content.to_packed::<ParElem>() {
        let before = slots.len();
        prefix.push(SlotStep::ParBody);
        collect_slots(&par.body, prefix, slots);
        prefix.pop();
        if slots.len() > before {
            return;
        }
        push_slot(prefix, SlotStep::ParBody, par.body.clone(), slots);
        return;
    }

    if let Some(item) = content.to_packed::<ListItem>() {
        push_slot(prefix, SlotStep::ItemBody, item.body.clone(), slots);
        return;
    }

    if let Some(item) = content.to_packed::<EnumItem>() {
        push_slot(prefix, SlotStep::ItemBody, item.body.clone(), slots);
        return;
    }

    if let Some(item) = content.to_packed::<TermItem>() {
        push_slot(prefix, SlotStep::Term(0), item.term.clone(), slots);
        push_slot(
            prefix,
            SlotStep::TermDescription(0),
            item.description.clone(),
            slots,
        );
        return;
    }

    if let Some(list) = content.to_packed::<ListElem>() {
        for (index, item) in list.children.iter().enumerate() {
            push_slot(prefix, SlotStep::ListItem(index), item.body.clone(), slots);
        }
        return;
    }

    if let Some(enm) = content.to_packed::<EnumElem>() {
        for (index, item) in enm.children.iter().enumerate() {
            push_slot(prefix, SlotStep::EnumItem(index), item.body.clone(), slots);
        }
        return;
    }

    if let Some(terms) = content.to_packed::<TermsElem>() {
        for (index, item) in terms.children.iter().enumerate() {
            push_slot(prefix, SlotStep::Term(index), item.term.clone(), slots);
            push_slot(
                prefix,
                SlotStep::TermDescription(index),
                item.description.clone(),
                slots,
            );
        }
        return;
    }

    if let Some(figure) = content.to_packed::<FigureElem>() {
        push_slot(prefix, SlotStep::FigureBody, figure.body.clone(), slots);
        if let Some(caption) = figure.caption.get_cloned(StyleChain::default()) {
            push_slot(prefix, SlotStep::FigureCaption, caption.body.clone(), slots);
        }
        return;
    }

    if let Some(footnote) = content.to_packed::<FootnoteElem>() {
        if let FootnoteBody::Content(body) = &footnote.body {
            push_slot(prefix, SlotStep::FootnoteBody, body.clone(), slots);
        }
        return;
    }

    if let Some(quote) = content.to_packed::<QuoteElem>() {
        push_slot(prefix, SlotStep::QuoteBody, quote.body.clone(), slots);
        return;
    }

    if let Some(table) = content.to_packed::<TableElem>() {
        let mut index = 0;
        collect_table_slots(&table.children, prefix, slots, &mut index);
        return;
    }

    if let Some(grid) = content.to_packed::<GridElem>() {
        let mut index = 0;
        collect_grid_slots(&grid.children, prefix, slots, &mut index);
        return;
    }

    if let Some(stack) = content.to_packed::<StackElem>() {
        for (index, child) in stack.children.iter().enumerate() {
            if let StackChild::Block(body) = child {
                push_slot(prefix, SlotStep::StackChild(index), body.clone(), slots);
            }
        }
        return;
    }

    if let Some(body) = wrapper_body(content) {
        push_slot(prefix, SlotStep::WrapperBody, body, slots);
    }
}

fn push_slot(prefix: &[SlotStep], step: SlotStep, content: Content, slots: &mut Vec<ContentSlot>) {
    let mut path = prefix.to_vec();
    path.push(step);
    slots.push(ContentSlot { path, content });
}

fn wrapper_body(content: &Content) -> Option<Content> {
    if let Some(elem) = content.to_packed::<AlignElem>() {
        return Some(elem.body.clone());
    }
    if let Some(elem) = content.to_packed::<PadElem>() {
        return Some(elem.body.clone());
    }
    if let Some(elem) = content.to_packed::<PlaceElem>() {
        return Some(elem.body.clone());
    }
    if let Some(elem) = content.to_packed::<ColumnsElem>() {
        return Some(elem.body.clone());
    }
    if let Some(elem) = content.to_packed::<BoxElem>() {
        return elem.body.get_cloned(StyleChain::default());
    }
    if let Some(elem) = content.to_packed::<BlockElem>() {
        return match elem.body.get_cloned(StyleChain::default()) {
            Some(BlockBody::Content(body)) => Some(body.clone()),
            _ => None,
        };
    }
    if let Some(elem) = content.to_packed::<RectElem>() {
        return elem.body.get_cloned(StyleChain::default());
    }
    if let Some(elem) = content.to_packed::<CircleElem>() {
        return elem.body.get_cloned(StyleChain::default());
    }
    if let Some(elem) = content.to_packed::<EllipseElem>() {
        return elem.body.get_cloned(StyleChain::default());
    }
    None
}

fn collect_table_slots(
    children: &[TableChild],
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    for child in children {
        match child {
            TableChild::Header(header) => {
                collect_table_item_slots(&header.children, prefix, slots, index)
            }
            TableChild::Footer(footer) => {
                collect_table_item_slots(&footer.children, prefix, slots, index)
            }
            TableChild::Item(item) => collect_table_item_slot(item, prefix, slots, index),
        }
    }
}

fn collect_table_item_slots(
    items: &[TableItem],
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    for item in items {
        collect_table_item_slot(item, prefix, slots, index);
    }
}

fn collect_table_item_slot(
    item: &TableItem,
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    if let TableItem::Cell(cell) = item {
        push_slot(
            prefix,
            SlotStep::TableCell(*index),
            cell.body.clone(),
            slots,
        );
        *index += 1;
    }
}

fn collect_grid_slots(
    children: &[GridChild],
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    for child in children {
        match child {
            GridChild::Header(header) => {
                collect_grid_item_slots(&header.children, prefix, slots, index)
            }
            GridChild::Footer(footer) => {
                collect_grid_item_slots(&footer.children, prefix, slots, index)
            }
            GridChild::Item(item) => collect_grid_item_slot(item, prefix, slots, index),
        }
    }
}

fn collect_grid_item_slots(
    items: &[GridItem],
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    for item in items {
        collect_grid_item_slot(item, prefix, slots, index);
    }
}

fn collect_grid_item_slot(
    item: &GridItem,
    prefix: &[SlotStep],
    slots: &mut Vec<ContentSlot>,
    index: &mut usize,
) {
    if let GridItem::Cell(cell) = item {
        push_slot(prefix, SlotStep::GridCell(*index), cell.body.clone(), slots);
        *index += 1;
    }
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
    use crate::eval::eval_to_content;
    use crate::world::SystemWorld;
    use std::fs;
    use tempfile::TempDir;
    use typst::foundations::Packed;
    use typst::text::TextElem;

    fn eval(source: &str) -> (TempDir, Content) {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), source).unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = eval_to_content(&world).unwrap();
        (dir, normalize_list_item_runs(content))
    }

    fn text(s: &str) -> Content {
        TextElem::packed(s)
    }

    fn slot_texts(content: &Content) -> Vec<String> {
        extract_slots(content)
            .into_iter()
            .map(|slot| slot.content.plain_text().to_string())
            .collect()
    }

    fn replace_nth_slot(content: &Content, index: usize, replacement: &str) -> Content {
        let slots = extract_slots(content);
        replace_slot(content, &slots[index].path, text(replacement)).unwrap()
    }

    #[test]
    fn list_slots_extract_and_replace_each_item_body() {
        let content = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(text("Alpha"))),
            Packed::new(ListItem::new(text("Beta"))),
            Packed::new(ListItem::new(text("Gamma"))),
        ]));

        let slots = extract_slots(&content);
        assert_eq!(
            slots
                .iter()
                .map(|slot| slot.path.clone())
                .collect::<Vec<_>>(),
            vec![
                vec![SlotStep::ListItem(0)],
                vec![SlotStep::ListItem(1)],
                vec![SlotStep::ListItem(2)]
            ]
        );

        let replaced = replace_slot(&content, &slots[1].path, text("Better")).unwrap();
        assert_eq!(slot_texts(&replaced), ["Alpha", "Better", "Gamma"]);
        assert_eq!(slot_texts(&content), ["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn enum_slots_extract_and_replace_each_item_body() {
        let content = Content::new(EnumElem::new(vec![
            Packed::new(EnumItem::new(text("One"))),
            Packed::new(EnumItem::new(text("Two"))),
        ]));

        let slots = extract_slots(&content);
        assert_eq!(
            slots
                .iter()
                .map(|slot| slot.path.clone())
                .collect::<Vec<_>>(),
            vec![vec![SlotStep::EnumItem(0)], vec![SlotStep::EnumItem(1)]]
        );

        let replaced = replace_slot(&content, &slots[0].path, text("First")).unwrap();
        assert_eq!(slot_texts(&replaced), ["First", "Two"]);
    }

    #[test]
    fn term_slots_extract_and_replace_term_and_description() {
        let content = Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
            text("API"),
            text("Old description"),
        ))]));

        let slots = extract_slots(&content);
        assert_eq!(
            slots
                .iter()
                .map(|slot| slot.path.clone())
                .collect::<Vec<_>>(),
            vec![vec![SlotStep::Term(0)], vec![SlotStep::TermDescription(0)]]
        );

        let replaced = replace_slot(&content, &slots[0].path, text("SDK")).unwrap();
        let replaced = replace_slot(&replaced, &slots[1].path, text("New description")).unwrap();
        assert_eq!(slot_texts(&replaced), ["SDK", "New description"]);
    }

    #[test]
    fn figure_slots_extract_body_and_caption() {
        let (_dir, content) = eval("#figure(rect(width: 10pt, height: 4pt), caption: [Old cap])");
        let slots = extract_slots(&content);

        assert!(
            slots
                .iter()
                .any(|slot| slot.path.ends_with(&[SlotStep::FigureBody]))
        );
        assert!(
            slots
                .iter()
                .any(|slot| slot.path.ends_with(&[SlotStep::FigureCaption]))
        );

        let caption = slots
            .iter()
            .find(|slot| slot.path.ends_with(&[SlotStep::FigureCaption]))
            .unwrap();
        let replaced = replace_slot(&content, &caption.path, text("New cap")).unwrap();
        assert!(replaced.plain_text().contains("New cap"));
        assert!(!replaced.plain_text().contains("Old cap"));
    }

    #[test]
    fn footnote_slots_extract_and_replace_body() {
        let (_dir, content) = eval("Text#footnote[Old note].");
        let slots = extract_slots(&content);
        let footnote = slots
            .iter()
            .find(|slot| slot.path.ends_with(&[SlotStep::FootnoteBody]))
            .unwrap();

        let replaced = replace_slot(&content, &footnote.path, text("New note")).unwrap();
        assert!(replaced.plain_text().contains("New note"));
        assert!(!replaced.plain_text().contains("Old note"));
    }

    #[test]
    fn quote_slots_extract_and_replace_body() {
        let (_dir, content) = eval("#quote[Old quote]");
        let slots = extract_slots(&content);
        assert!(
            slots
                .iter()
                .any(|slot| slot.path == vec![SlotStep::QuoteBody])
        );

        let replaced = replace_nth_slot(&content, 0, "New quote");
        assert_eq!(replaced.plain_text(), "New quote");
    }

    #[test]
    fn table_slots_extract_and_replace_cells_in_document_order() {
        let (_dir, content) = eval("#table(columns: 2, [A], [B], [C], [D])");
        assert_eq!(slot_texts(&content), ["A", "B", "C", "D"]);

        let replaced = replace_nth_slot(&content, 2, "Changed");
        assert_eq!(slot_texts(&replaced), ["A", "B", "Changed", "D"]);
    }

    #[test]
    fn grid_slots_extract_and_replace_cells_in_document_order() {
        let (_dir, content) = eval("#grid(columns: 2, [A], [B], [C], [D])");
        assert_eq!(slot_texts(&content), ["A", "B", "C", "D"]);

        let replaced = replace_nth_slot(&content, 3, "Changed");
        assert_eq!(slot_texts(&replaced), ["A", "B", "C", "Changed"]);
    }

    #[test]
    fn stack_slots_extract_and_replace_block_children() {
        let (_dir, content) = eval("#stack(dir: ttb, [Top], [Bottom])");
        assert_eq!(slot_texts(&content), ["Top", "Bottom"]);

        let replaced = replace_nth_slot(&content, 1, "Lower");
        assert_eq!(slot_texts(&replaced), ["Top", "Lower"]);
    }

    #[test]
    fn wrapper_slots_extract_and_replace_body() {
        let cases = [
            "#align(center)[Old]",
            "#pad(5pt)[Old]",
            "#place(top)[Old]",
            "#columns(2)[Old]",
            "#box[Old]",
            "#block[Old]",
            "#rect[Old]",
            "#circle[Old]",
            "#ellipse[Old]",
        ];

        for source in cases {
            let (_dir, content) = eval(source);
            let slots = extract_slots(&content);
            assert_eq!(slots.len(), 1, "{source}: {slots:?}");
            assert!(
                slots[0].path.ends_with(&[SlotStep::WrapperBody]),
                "{source}"
            );

            let replaced = replace_slot(&content, &slots[0].path, text("New")).unwrap();
            assert!(replaced.plain_text().contains("New"), "{source}");
            assert!(!replaced.plain_text().contains("Old"), "{source}");
        }
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
        assert_eq!(slot_texts(&seq.children[1]), ["Alpha", "Beta"]);
    }

    #[test]
    fn normalize_list_item_runs_recurses_into_slot_content() {
        let quote = Content::new(QuoteElem::new(Content::sequence([
            Content::new(ListItem::new(text("Alpha"))),
            Content::new(ListItem::new(text("Beta"))),
        ])));

        let normalized = normalize_list_item_runs(quote);
        let slots = extract_slots(&normalized);
        assert_eq!(slots.len(), 1);
        assert_eq!(slot_texts(&slots[0].content), ["Alpha", "Beta"]);
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
    fn is_slot_container_matches_representative_extractable_elements() {
        let containers = [
            Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
                "Item",
            )))])),
            Content::new(EnumElem::new(vec![Packed::new(EnumItem::new(text(
                "Item",
            )))])),
            Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
                text("Term"),
                text("Description"),
            ))])),
            Content::new(QuoteElem::new(text("Quote"))),
        ];

        for container in containers {
            assert!(is_slot_container(&container));
            assert!(!extract_slots(&container).is_empty());
        }
        assert!(!is_slot_container(&text("Plain")));
        assert!(extract_slots(&text("Plain")).is_empty());
    }
}
