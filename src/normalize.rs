use typst::foundations::{Content, SequenceElem, StyledElem};
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

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::Packed;
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
}
