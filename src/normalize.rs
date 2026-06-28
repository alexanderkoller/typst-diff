use typst::foundations::{Content, SequenceElem, StyleChain, StyledElem};
use typst::model::{
    EnumElem, EnumItem, ListElem, ListItem, ParElem, ParbreakElem, TermItem, TermsElem,
};
use typst::text::SpaceElem;

/// Wrap consecutive bare `ListItem` / `EnumItem` / `TermItem` nodes into their
/// container elements (`ListElem`, `EnumElem`, `TermsElem`).
///
/// Typst's evaluator sometimes emits list items as siblings in a `SequenceElem`
/// rather than inside a container. This normalization step gives annotation a
/// stable semantic tree to map onto the realized tree.
pub(crate) fn normalize_list_item_runs(content: Content) -> Content {
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

    if let Some(list) = content.to_packed_mut::<ListElem>() {
        for item in &mut list.children {
            item.body = normalize_list_item_runs(item.body.clone());
        }
        return content;
    }

    if let Some(enm) = content.to_packed_mut::<EnumElem>() {
        for item in &mut enm.children {
            item.body = normalize_list_item_runs(item.body.clone());
        }
        return content;
    }

    if let Some(terms) = content.to_packed_mut::<TermsElem>() {
        for item in &mut terms.children {
            item.term = normalize_list_item_runs(item.term.clone());
            item.description = normalize_list_item_runs(item.description.clone());
        }
        return content;
    }

    content
}

fn group_list_item_runs(children: Vec<Content>) -> Vec<Content> {
    let mut grouped = Vec::new();
    let mut index = 0;

    while index < children.len() {
        if is_list_component(&children[index]) {
            let span = children[index].span();
            let mut items = Vec::new();
            let mut tight = true;
            let mut template = None;
            let mut pending_parbreak = false;
            loop {
                if children[index].is::<ListItem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    let child = children[index].clone();
                    items.push(child.into_packed::<ListItem>().unwrap());
                    index += 1;
                } else if let Some(list) = children[index].to_packed::<ListElem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    tight &= list.tight.get(StyleChain::default());
                    if template.is_none() {
                        template = Some(list.clone().unpack());
                    }
                    items.extend(list.children.iter().cloned());
                    index += 1;
                } else {
                    break;
                }

                let Some((next_index, had_parbreak)) =
                    next_component_after_separators(&children, index, is_list_component)
                else {
                    break;
                };
                pending_parbreak = had_parbreak;
                index = next_index;
            }
            let list = if let Some(mut list) = template {
                list.children = items;
                list.with_tight(tight)
            } else {
                ListElem::new(items).with_tight(tight)
            };
            grouped.push(Content::new(list).spanned(span));
        } else if is_enum_component(&children[index]) {
            let span = children[index].span();
            let mut items = Vec::new();
            let mut tight = true;
            let mut template = None;
            let mut pending_parbreak = false;
            loop {
                if children[index].is::<EnumItem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    let child = children[index].clone();
                    items.push(child.into_packed::<EnumItem>().unwrap());
                    index += 1;
                } else if let Some(enm) = children[index].to_packed::<EnumElem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    tight &= enm.tight.get(StyleChain::default());
                    if template.is_none() {
                        template = Some(enm.clone().unpack());
                    }
                    items.extend(enm.children.iter().cloned());
                    index += 1;
                } else {
                    break;
                }

                let Some((next_index, had_parbreak)) =
                    next_component_after_separators(&children, index, is_enum_component)
                else {
                    break;
                };
                pending_parbreak = had_parbreak;
                index = next_index;
            }
            let enm = if let Some(mut enm) = template {
                enm.children = items;
                enm.with_tight(tight)
            } else {
                EnumElem::new(items).with_tight(tight)
            };
            grouped.push(Content::new(enm).spanned(span));
        } else if is_terms_component(&children[index]) {
            let span = children[index].span();
            let mut items = Vec::new();
            let mut tight = true;
            let mut template = None;
            let mut pending_parbreak = false;
            loop {
                if children[index].is::<TermItem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    let child = children[index].clone();
                    items.push(child.into_packed::<TermItem>().unwrap());
                    index += 1;
                } else if let Some(terms) = children[index].to_packed::<TermsElem>() {
                    if !items.is_empty() && pending_parbreak {
                        tight = false;
                    }
                    tight &= terms.tight.get(StyleChain::default());
                    if template.is_none() {
                        template = Some(terms.clone().unpack());
                    }
                    items.extend(terms.children.iter().cloned());
                    index += 1;
                } else {
                    break;
                }

                let Some((next_index, had_parbreak)) =
                    next_component_after_separators(&children, index, is_terms_component)
                else {
                    break;
                };
                pending_parbreak = had_parbreak;
                index = next_index;
            }
            let terms = if let Some(mut terms) = template {
                terms.children = items;
                terms.with_tight(tight)
            } else {
                TermsElem::new(items).with_tight(tight)
            };
            grouped.push(Content::new(terms).spanned(span));
        } else {
            grouped.push(children[index].clone());
            index += 1;
        }
    }

    grouped
}

fn is_list_item_separator(content: &Content) -> bool {
    content.is::<SpaceElem>() || content.is::<ParbreakElem>()
}

fn is_list_component(content: &Content) -> bool {
    content.is::<ListItem>() || content.is::<ListElem>()
}

fn is_enum_component(content: &Content) -> bool {
    content.is::<EnumItem>() || content.is::<EnumElem>()
}

fn is_terms_component(content: &Content) -> bool {
    content.is::<TermItem>() || content.is::<TermsElem>()
}

fn next_component_after_separators(
    children: &[Content],
    index: usize,
    is_component: fn(&Content) -> bool,
) -> Option<(usize, bool)> {
    let mut next = index;
    let mut had_parbreak = false;
    while next < children.len() && is_list_item_separator(&children[next]) {
        had_parbreak |= children[next].is::<ParbreakElem>();
        next += 1;
    }
    (next < children.len() && is_component(&children[next])).then_some((next, had_parbreak))
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Packed, StyleChain};
    use typst::text::TextElem;

    #[test]
    fn normalize_list_item_runs_groups_items_and_preserves_siblings() {
        let content = Content::sequence([
            TextElem::packed("before"),
            Content::new(ListItem::new(TextElem::packed("A"))),
            Content::new(SpaceElem::new()),
            Content::new(ListItem::new(TextElem::packed("B"))),
            TextElem::packed("after"),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        assert_eq!(seq.children.len(), 3);
        assert!(seq.children[0].plain_text().contains("before"));
        assert!(seq.children[1].is::<ListElem>());
        assert_eq!(
            seq.children[1]
                .to_packed::<ListElem>()
                .unwrap()
                .children
                .len(),
            2
        );
        assert!(seq.children[2].plain_text().contains("after"));
    }

    #[test]
    fn normalize_list_item_runs_marks_parbreak_separated_items_as_loose() {
        let content = Content::sequence([
            Content::new(ListItem::new(TextElem::packed("A"))),
            Content::new(ParbreakElem::new()),
            Content::new(ListItem::new(TextElem::packed("B"))),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        let list = seq.children[0].to_packed::<ListElem>().unwrap();

        assert!(!list.tight.get(StyleChain::default()));
    }

    #[test]
    fn normalize_list_item_runs_ignores_trailing_parbreak_for_tightness() {
        let content = Content::sequence([
            Content::new(ListItem::new(TextElem::packed("A"))),
            Content::new(ParbreakElem::new()),
            TextElem::packed("after"),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        let list = seq.children[0].to_packed::<ListElem>().unwrap();

        assert!(list.tight.get(StyleChain::default()));
    }

    #[test]
    fn normalize_term_item_runs_groups_terms() {
        let content = Content::sequence([
            Content::new(TermItem::new(
                TextElem::packed("API"),
                TextElem::packed("Description"),
            )),
            Content::new(TermItem::new(
                TextElem::packed("SDK"),
                TextElem::packed("Toolkit"),
            )),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        assert_eq!(seq.children.len(), 1);
        let terms = seq.children[0].to_packed::<TermsElem>().unwrap();
        assert_eq!(terms.children.len(), 2);
    }

    #[test]
    fn normalize_enum_item_runs_groups_enums() {
        let content = Content::sequence([
            Content::new(EnumItem::new(TextElem::packed("One"))),
            Content::new(EnumItem::new(TextElem::packed("Two"))),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        assert_eq!(seq.children.len(), 1);
        let enm = seq.children[0].to_packed::<EnumElem>().unwrap();
        assert_eq!(enm.children.len(), 2);
    }

    #[test]
    fn normalize_enum_and_term_item_runs_preserve_loose_tightness() {
        let enum_content = Content::sequence([
            Content::new(EnumItem::new(TextElem::packed("One"))),
            Content::new(ParbreakElem::new()),
            Content::new(EnumItem::new(TextElem::packed("Two"))),
        ]);
        let terms_content = Content::sequence([
            Content::new(TermItem::new(
                TextElem::packed("API"),
                TextElem::packed("Description"),
            )),
            Content::new(ParbreakElem::new()),
            Content::new(TermItem::new(
                TextElem::packed("SDK"),
                TextElem::packed("Toolkit"),
            )),
        ]);

        let enum_normalized = normalize_list_item_runs(enum_content);
        let enum_seq = enum_normalized.to_packed::<SequenceElem>().unwrap();
        let enm = enum_seq.children[0].to_packed::<EnumElem>().unwrap();

        let terms_normalized = normalize_list_item_runs(terms_content);
        let terms_seq = terms_normalized.to_packed::<SequenceElem>().unwrap();
        let terms = terms_seq.children[0].to_packed::<TermsElem>().unwrap();

        assert!(!enm.tight.get(StyleChain::default()));
        assert!(!terms.tight.get(StyleChain::default()));
    }

    #[test]
    fn normalize_merges_existing_list_with_following_bare_item() {
        let existing = Content::new(
            ListElem::new(vec![Packed::new(ListItem::new(TextElem::packed("A")))])
                .with_tight(false),
        );
        let content = Content::sequence([
            existing,
            Content::new(SpaceElem::new()),
            Content::new(ListItem::new(TextElem::packed("B"))),
        ]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        let list = seq.children[0].to_packed::<ListElem>().unwrap();

        assert_eq!(seq.children.len(), 1);
        assert_eq!(list.children.len(), 2);
        assert_eq!(list.children[0].body.plain_text(), "A");
        assert_eq!(list.children[1].body.plain_text(), "B");
        assert!(!list.tight.get(StyleChain::default()));
    }

    #[test]
    fn normalize_merges_existing_enum_and_terms_with_following_bare_items() {
        let enum_existing = Content::new(EnumElem::new(vec![Packed::new(EnumItem::new(
            TextElem::packed("One"),
        ))]));
        let enum_content = Content::sequence([
            enum_existing,
            Content::new(SpaceElem::new()),
            Content::new(EnumItem::new(TextElem::packed("Two"))),
        ]);

        let terms_existing = Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
            TextElem::packed("API"),
            TextElem::packed("Description"),
        ))]));
        let terms_content = Content::sequence([
            terms_existing,
            Content::new(SpaceElem::new()),
            Content::new(TermItem::new(
                TextElem::packed("SDK"),
                TextElem::packed("Toolkit"),
            )),
        ]);

        let enum_normalized = normalize_list_item_runs(enum_content);
        let enum_seq = enum_normalized.to_packed::<SequenceElem>().unwrap();
        let enm = enum_seq.children[0].to_packed::<EnumElem>().unwrap();

        let terms_normalized = normalize_list_item_runs(terms_content);
        let terms_seq = terms_normalized.to_packed::<SequenceElem>().unwrap();
        let terms = terms_seq.children[0].to_packed::<TermsElem>().unwrap();

        assert_eq!(enum_seq.children.len(), 1);
        assert_eq!(enm.children.len(), 2);
        assert!(enm.tight.get(StyleChain::default()));
        assert_eq!(terms_seq.children.len(), 1);
        assert_eq!(terms.children.len(), 2);
        assert!(terms.tight.get(StyleChain::default()));
    }

    #[test]
    fn normalize_nested_sequence() {
        let inner = Content::sequence([
            Content::new(ListItem::new(TextElem::packed("A"))),
            TextElem::packed("tail"),
        ]);
        let content = Content::sequence([inner, TextElem::packed("outer")]);

        let normalized = normalize_list_item_runs(content);
        let seq = normalized.to_packed::<SequenceElem>().unwrap();
        let inner = seq.children[0].to_packed::<SequenceElem>().unwrap();
        assert!(inner.children[0].is::<ListElem>());
    }

    #[test]
    fn normalize_preserves_existing_container() {
        let list = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            TextElem::packed("A"),
        ))]));

        let normalized = normalize_list_item_runs(list);
        assert!(normalized.is::<ListElem>());
    }

    #[test]
    fn normalize_recurses_into_existing_list_item_bodies() {
        let body = Content::sequence([
            TextElem::packed("Parent"),
            Content::new(ListItem::new(TextElem::packed("Nested A"))),
            Content::new(ListItem::new(TextElem::packed("Nested B"))),
        ]);
        let list = Content::new(ListElem::new(vec![Packed::new(ListItem::new(body))]));

        let normalized = normalize_list_item_runs(list);
        let list = normalized.to_packed::<ListElem>().unwrap();
        let body = &list.children[0].body;
        let body = body.to_packed::<SequenceElem>().unwrap();

        assert!(body.children[1].is::<ListElem>());
        assert_eq!(
            body.children[1]
                .to_packed::<ListElem>()
                .unwrap()
                .children
                .len(),
            2
        );
    }
}
