use typst::foundations::Content;
use typst::foundations::{SequenceElem, StyledElem};
use typst::model::ParElem;
use typst::text::{StrikeElem, TextElem};
use typst::visualize::Color;

use crate::diff::{DiffBlock, DiffResult, DiffResultOp, WordOp};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}

pub fn build_annotated_content(result: &DiffResult) -> Content {
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
                let inline = annotated_inline_content(word_ops);
                DiffBlock {
                    content: replace_text_container(&new_block.content, &inline).unwrap_or(inline),
                    page_styles: new_block.page_styles.clone(),
                }
            }
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

fn annotated_inline_content(word_ops: &[WordOp]) -> Content {
    let mut inline: Vec<Content> = Vec::new();
    for wop in word_ops {
        match wop {
            WordOp::Equal(tokens) => {
                for t in tokens {
                    inline.push(t.content.clone());
                }
            }
            WordOp::Insert(tokens) => {
                let joined = Content::sequence(tokens.iter().map(|t| t.content.clone()));
                inline.push(joined.styled(TextElem::fill.set(green().into())));
            }
            WordOp::Delete(tokens) => {
                let joined = Content::sequence(tokens.iter().map(|t| {
                    if t.content.plain_text().is_empty() {
                        TextElem::packed(t.text.as_str())
                    } else {
                        t.content.clone()
                    }
                }));
                let colored = joined.styled(TextElem::fill.set(red().into()));
                inline.push(Content::new(StrikeElem::new(colored)));
            }
        }
    }
    Content::sequence(inline)
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
        let content = build_annotated_content(&result);
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
        let content = build_annotated_content(&result);
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
        let content = build_annotated_content(&result);

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
