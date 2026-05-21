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

#[derive(Clone, Debug)]
pub struct ContentSlot {
    pub path: Vec<SlotStep>,
    pub content: Content,
}

pub fn extract_slots(content: &Content) -> Vec<ContentSlot> {
    let mut slots = Vec::new();
    collect_slots(content, &mut Vec::new(), &mut slots);
    slots
}

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
