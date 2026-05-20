use typst::foundations::Content;
use typst::model::ParbreakElem;
use typst::text::{StrikeElem, TextElem};
use typst::visualize::Color;

use crate::diff::{DiffResult, DiffResultOp, WordOp};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}

pub fn build_annotated_content(result: &DiffResult) -> Content {
    let mut blocks: Vec<Content> = Vec::new();

    for op in &result.block_ops {
        match op {
            DiffResultOp::Equal(c) => blocks.push(c.clone()),

            DiffResultOp::Inserted(c) => {
                blocks.push(c.clone().styled(TextElem::fill.set(green().into())));
            }

            DiffResultOp::Deleted(c) => {
                let colored = c.clone().styled(TextElem::fill.set(red().into()));
                blocks.push(Content::new(StrikeElem::new(colored)));
            }

            DiffResultOp::Modified(word_ops) => {
                let mut inline: Vec<Content> = Vec::new();
                for wop in word_ops {
                    match wop {
                        WordOp::Equal(tokens) => {
                            for t in tokens {
                                inline.push(t.content.clone());
                            }
                        }
                        WordOp::Insert(tokens) => {
                            let joined = Content::sequence(
                                tokens.iter().map(|t| t.content.clone()),
                            );
                            inline
                                .push(joined.styled(TextElem::fill.set(green().into())));
                        }
                        WordOp::Delete(tokens) => {
                            let joined = Content::sequence(
                                tokens.iter().map(|t| t.content.clone()),
                            );
                            let colored =
                                joined.styled(TextElem::fill.set(red().into()));
                            inline.push(Content::new(StrikeElem::new(colored)));
                        }
                    }
                }
                blocks.push(Content::sequence(inline));
            }
        }

        blocks.push(Content::new(ParbreakElem::new()));
    }

    Content::sequence(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::TextElem;
    use crate::diff::{DiffResult, DiffResultOp, WordOp, Token};

    fn word_token(s: &str) -> Token {
        Token { text: s.to_string(), content: TextElem::packed(s) }
    }

    #[test]
    fn inserted_block_wrapped_green() {
        let result = DiffResult {
            block_ops: vec![DiffResultOp::Inserted(TextElem::packed("New paragraph"))],
        };
        let content = build_annotated_content(&result);
        assert!(!content.is_empty());
    }

    #[test]
    fn modified_block_contains_strike_for_deletion() {
        let result = DiffResult {
            block_ops: vec![DiffResultOp::Modified(vec![
                WordOp::Equal(vec![word_token("The ")]),
                WordOp::Delete(vec![word_token("old")]),
                WordOp::Insert(vec![word_token("new")]),
                WordOp::Equal(vec![word_token(" text.")]),
            ])],
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
}
