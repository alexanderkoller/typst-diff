//! Shared content-tree traversal and path editing helpers.
//!
//! This module owns generic path mechanics. Element-specific child extraction
//! and child replacement still live in `container_ops` during this phase; those
//! APIs are the current source of truth for what typst-diff considers a
//! realized child.

use typst::foundations::Content;
use typst::foundations::{SequenceElem, StyleChain, StyledElem};
use typst::layout::{
    AlignElem, BlockBody, BlockElem, BoxElem, ColumnsElem, HideElem, PadElem, PlaceElem,
};
use typst::model::{
    EmphElem, EnumElem, EnumItem, FigureCaption, FigureElem, FootnoteBody, FootnoteElem,
    HeadingElem, LinkElem, ListElem, ListItem, ParElem, StrongElem, TermItem, TermsElem,
};
use typst::text::{HighlightElem, StrikeElem};
use typst::visualize::{CircleElem, EllipseElem, RectElem};

use crate::annotated::AnnotatedContent;
use crate::container_ops;

pub(crate) fn realized_content_at_path(content: &Content, path: &[usize]) -> Option<Content> {
    let mut current = content.clone();
    for index in path {
        current = container_ops::realized_child_contents(&current)
            .get(*index)?
            .clone();
    }
    Some(current)
}

pub(crate) fn replace_realized_content_at_path(
    content: &Content,
    path: &[usize],
    replacement: Content,
) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return Some(replacement);
    };
    let child = container_ops::realized_child_contents(content)
        .get(*index)?
        .clone();
    let patched_child = replace_realized_content_at_path(&child, rest, replacement)?;
    container_ops::replace_realized_child(content, *index, patched_child)
}

pub(crate) fn insert_realized_content_at_path(
    content: &Content,
    path: &[usize],
    insertion: Content,
    before: bool,
) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return None;
    };
    if rest.is_empty() {
        return container_ops::insert_realized_child(content, *index, insertion, before);
    }
    let child = container_ops::realized_child_contents(content)
        .get(*index)?
        .clone();
    let patched_child = insert_realized_content_at_path(&child, rest, insertion, before)?;
    container_ops::replace_realized_child(content, *index, patched_child)
}

pub(crate) fn map_realized_children(
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

pub(crate) fn replace_annotated_at_path(
    node: &mut AnnotatedContent,
    path: &[usize],
    replacement: AnnotatedContent,
) -> bool {
    let Some((first, rest)) = path.split_first() else {
        *node = replacement;
        return true;
    };
    let Some(child) = node.children.get_mut(*first) else {
        return false;
    };
    replace_annotated_at_path(child, rest, replacement)
}

pub(crate) fn map_transparent_children(
    content: &Content,
    mut map_child: impl FnMut(&Content) -> Content,
) -> Option<Content> {
    let mut content = content.clone();

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq.children.iter().map(map_child).collect();
        return Some(content);
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = map_child(&styled.child);
        return Some(content);
    }

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = map_child(&par.body);
        return Some(content);
    }

    if let Some(heading) = content.to_packed_mut::<HeadingElem>() {
        heading.body = map_child(&heading.body);
        return Some(content);
    }

    if let Some(link) = content.to_packed_mut::<LinkElem>() {
        link.body = map_child(&link.body);
        return Some(content);
    }

    if let Some(strong) = content.to_packed_mut::<StrongElem>() {
        strong.body = map_child(&strong.body);
        return Some(content);
    }

    if let Some(emph) = content.to_packed_mut::<EmphElem>() {
        emph.body = map_child(&emph.body);
        return Some(content);
    }

    if let Some(highlight) = content.to_packed_mut::<HighlightElem>() {
        highlight.body = map_child(&highlight.body);
        return Some(content);
    }

    if let Some(hidden) = content.to_packed_mut::<HideElem>() {
        hidden.body = map_child(&hidden.body);
        return Some(content);
    }

    if let Some(figure) = content.to_packed_mut::<FigureElem>() {
        figure.body = map_child(&figure.body);
        if let Some(caption) = figure.caption.as_option_mut().as_mut()
            && let Some(caption) = caption.as_mut()
        {
            caption.body = map_child(&caption.body);
        }
        return Some(content);
    }

    if let Some(caption) = content.to_packed_mut::<FigureCaption>() {
        caption.body = map_child(&caption.body);
        return Some(content);
    }

    if let Some(footnote) = content.to_packed_mut::<FootnoteElem>()
        && let FootnoteBody::Content(body) = &footnote.body
    {
        footnote.body = FootnoteBody::Content(map_child(body));
        return Some(content);
    }

    if let Some(block) = content.to_packed_mut::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        block.body.set(Some(BlockBody::Content(map_child(&body))));
        return Some(content);
    }

    if let Some(align) = content.to_packed_mut::<AlignElem>() {
        align.body = map_child(&align.body);
        return Some(content);
    }

    if let Some(pad) = content.to_packed_mut::<PadElem>() {
        pad.body = map_child(&pad.body);
        return Some(content);
    }

    if let Some(place) = content.to_packed_mut::<PlaceElem>() {
        place.body = map_child(&place.body);
        return Some(content);
    }

    if let Some(columns) = content.to_packed_mut::<ColumnsElem>() {
        columns.body = map_child(&columns.body);
        return Some(content);
    }

    if let Some(box_elem) = content.to_packed_mut::<BoxElem>()
        && let Some(body) = box_elem.body.get_cloned(StyleChain::default())
    {
        box_elem.body.set(Some(map_child(&body)));
        return Some(content);
    }

    if let Some(rect) = content.to_packed_mut::<RectElem>()
        && let Some(body) = rect.body.get_cloned(StyleChain::default())
    {
        rect.body.set(Some(map_child(&body)));
        return Some(content);
    }

    if let Some(circle) = content.to_packed_mut::<CircleElem>()
        && let Some(body) = circle.body.get_cloned(StyleChain::default())
    {
        circle.body.set(Some(map_child(&body)));
        return Some(content);
    }

    if let Some(ellipse) = content.to_packed_mut::<EllipseElem>()
        && let Some(body) = ellipse.body.get_cloned(StyleChain::default())
    {
        ellipse.body.set(Some(map_child(&body)));
        return Some(content);
    }

    if let Some(strike) = content.to_packed_mut::<StrikeElem>() {
        strike.body = map_child(&strike.body);
        return Some(content);
    }

    if let Some(item) = content.to_packed_mut::<ListItem>() {
        item.body = map_child(&item.body);
        return Some(content);
    }

    if let Some(item) = content.to_packed_mut::<EnumItem>() {
        item.body = map_child(&item.body);
        return Some(content);
    }

    if let Some(item) = content.to_packed_mut::<TermItem>() {
        item.term = map_child(&item.term);
        item.description = map_child(&item.description);
        return Some(content);
    }

    if let Some(list) = content.to_packed_mut::<ListElem>() {
        for item in &mut list.children {
            item.body = map_child(&item.body);
        }
        return Some(content);
    }

    if let Some(enm) = content.to_packed_mut::<EnumElem>() {
        for item in &mut enm.children {
            item.body = map_child(&item.body);
        }
        return Some(content);
    }

    if let Some(terms) = content.to_packed_mut::<TermsElem>() {
        for item in &mut terms.children {
            item.term = map_child(&item.term);
            item.description = map_child(&item.description);
        }
        return Some(content);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::Packed;
    use typst::model::{ListElem, ListItem};
    use typst::text::TextElem;

    #[test]
    fn path_edit_replaces_and_inserts_list_children() {
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Gamma"))),
        ]));

        let replaced =
            replace_realized_content_at_path(&list, &[1], TextElem::packed("Beta")).unwrap();
        let replaced_list = replaced.to_packed::<ListElem>().unwrap();
        assert_eq!(replaced_list.children[1].body.plain_text(), "Beta");

        let inserted =
            insert_realized_content_at_path(&list, &[1], TextElem::packed("Beta"), true).unwrap();
        let inserted_list = inserted.to_packed::<ListElem>().unwrap();
        assert_eq!(inserted_list.children.len(), 3);
        assert_eq!(inserted_list.children[1].body.plain_text(), "Beta");
        assert_eq!(inserted_list.children[2].body.plain_text(), "Gamma");
    }

    #[test]
    fn path_edit_replaces_list_item_body() {
        let item = Content::new(ListItem::new(TextElem::packed("Alpha")));

        let replaced =
            replace_realized_content_at_path(&item, &[0], TextElem::packed("Beta")).unwrap();
        let replaced_item = replaced.to_packed::<ListItem>().unwrap();

        assert_eq!(replaced_item.body.plain_text(), "Beta");
    }
}
