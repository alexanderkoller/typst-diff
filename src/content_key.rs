use std::hash::{Hash, Hasher};

use typst::foundations::{Content, NativeElement, Repr, Style, StyleChain, StyledElem, Styles};
use typst::layout::{BlockBody, BlockElem};
use typst::math::EquationElem;
use typst::model::{EmphElem, HeadingElem, LinkElem, ParElem, StrongElem};
use typst::text::{
    HighlightElem, OverlineElem, RawElem, SpaceElem, StrikeElem, SubElem, SuperElem, TextElem,
    UnderlineElem,
};

use crate::annotated::{AnnotatedContent, effective_render_content, effective_text_content};
use crate::container_ops;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ContentKey(String);

impl ContentKey {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for ContentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn presentation_key(content: &Content) -> ContentKey {
    let mut out = String::new();
    write_presentation_key(content, &mut out);
    ContentKey(out)
}

pub(crate) fn context_presentation_key(content: &Content) -> ContentKey {
    let mut out = String::new();
    write_context_presentation_key(content, &mut out);
    ContentKey(out)
}

pub(crate) fn structural_child_key(child: &AnnotatedContent) -> ContentKey {
    ContentKey(format!(
        "{:?}:{}:{}",
        child.annotation.semantic_kind,
        effective_text_content(child).plain_text(),
        presentation_key(&effective_render_content(child))
    ))
}

pub(crate) fn slot_child_match_key(child: &AnnotatedContent) -> ContentKey {
    ContentKey(format!(
        "{}:{}",
        effective_text_content(child).plain_text(),
        presentation_key(&effective_render_content(child))
    ))
}

pub(crate) fn visible_unit_key(text: &str, content: &Content) -> ContentKey {
    ContentKey(format!("visible:{}:{}", text, presentation_key(content)))
}

pub(crate) fn normalized_visible_text_matches(left: &Content, right: &Content) -> bool {
    let left = normalized_visible_text(left);
    let right = normalized_visible_text(right);
    !left.is_empty() && left == right
}

pub(crate) fn normalized_visible_text(content: &Content) -> ContentKey {
    ContentKey(
        content
            .plain_text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Newtype that adds `Eq + Ord` to `Content` for Myers LCS block equality.
///
/// Equality and hashing remain Typst structural equality. Ordering uses visible
/// text plus structural hash only to satisfy `similar::capture_diff_slices`.
#[derive(Clone, Debug)]
pub(crate) struct BlockEqualityKey(Content);

impl BlockEqualityKey {
    pub(crate) fn new(content: Content) -> Self {
        Self(content)
    }
}

impl PartialEq for BlockEqualityKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for BlockEqualityKey {}

impl PartialOrd for BlockEqualityKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BlockEqualityKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let text_cmp = self.0.plain_text().cmp(&other.0.plain_text());
        if text_cmp != std::cmp::Ordering::Equal {
            return text_cmp;
        }
        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut h1);
        other.0.hash(&mut h2);
        h1.finish().cmp(&h2.finish())
    }
}

impl Hash for BlockEqualityKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

pub(crate) fn is_metadata_tag(content: &Content) -> bool {
    content.func().name() == "tag"
}

fn write_presentation_key(content: &Content, out: &mut String) {
    if let Some(seq) = content.to_packed::<typst::foundations::SequenceElem>() {
        out.push_str("seq[");
        for child in &seq.children {
            if is_metadata_tag(child) {
                continue;
            }
            write_presentation_key(child, out);
            out.push(';');
        }
        out.push(']');
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        let styles_key = styles_key(&styled.styles);
        if styles_key.is_empty() {
            write_presentation_key(&styled.child, out);
            return;
        }
        out.push_str("styled(");
        out.push_str(&styles_key);
        out.push_str(")[");
        write_presentation_key(&styled.child, out);
        out.push(']');
    } else if let Some(par) = content.to_packed::<ParElem>() {
        out.push_str("par[");
        write_presentation_key(&par.body, out);
        out.push(']');
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        out.push_str("heading[");
        write_presentation_key(&heading.body, out);
        out.push(']');
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        out.push_str("block(");
        if block_has_visual_decoration(block) {
            out.push_str("visual:");
            out.push_str(content.repr().as_str());
            out.push(')');
            return;
        }
        out.push_str(match block.body.get_cloned(StyleChain::default()) {
            Some(BlockBody::Content(_)) => "content",
            Some(_) => "other",
            None => "auto",
        });
        out.push_str(")[");
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            write_presentation_key(&body, out);
        }
        out.push(']');
    } else if let Some(equation) = content.to_packed::<EquationElem>() {
        out.push_str("equation(");
        out.push_str(if equation.block.get(StyleChain::default()) {
            "block"
        } else {
            "inline"
        });
        out.push_str("):");
        out.push_str(equation.body.repr().as_str());
    } else if let Some(link) = content.to_packed::<LinkElem>() {
        write_presentation_key(&link.body, out);
    } else if let Some(strong) = content.to_packed::<StrongElem>() {
        out.push_str("strong[");
        write_presentation_key(&strong.body, out);
        out.push(']');
    } else if let Some(emph) = content.to_packed::<EmphElem>() {
        out.push_str("emph[");
        write_presentation_key(&emph.body, out);
        out.push(']');
    } else if let Some(highlight) = content.to_packed::<HighlightElem>() {
        out.push_str("highlight(");
        out.push_str(&format!("{highlight:?}"));
        out.push_str(")[");
        write_presentation_key(&highlight.body, out);
        out.push(']');
    } else if let Some(sub) = content.to_packed::<SubElem>() {
        out.push_str("sub[");
        write_presentation_key(&sub.body, out);
        out.push(']');
    } else if let Some(sup) = content.to_packed::<SuperElem>() {
        out.push_str("super[");
        write_presentation_key(&sup.body, out);
        out.push(']');
    } else if let Some(underline) = content.to_packed::<UnderlineElem>() {
        out.push_str("underline[");
        write_presentation_key(&underline.body, out);
        out.push(']');
    } else if let Some(overline) = content.to_packed::<OverlineElem>() {
        out.push_str("overline[");
        write_presentation_key(&overline.body, out);
        out.push(']');
    } else if let Some(strike) = content.to_packed::<StrikeElem>() {
        out.push_str("strike[");
        write_presentation_key(&strike.body, out);
        out.push(']');
    } else if content.is::<TextElem>() {
        out.push_str("text");
    } else if content.is::<SpaceElem>() {
        out.push_str("space");
    } else if is_opaque_visual_element_name(content.func().name()) {
        out.push_str(content.func().name());
        out.push(':');
        out.push_str(content.repr().as_str());
    } else if is_metadata_tag(content) {
    } else {
        let children = container_ops::semantic_diff_child_contents(content);
        if !children.is_empty() {
            out.push_str(content.func().name());
            out.push('[');
            for child in children {
                if is_metadata_tag(&child) {
                    continue;
                }
                write_presentation_key(&child, out);
                out.push(';');
            }
            out.push(']');
        } else {
            out.push_str(content.func().name());
            out.push(':');
            out.push_str(content.plain_text().as_str());
        }
    }
}

fn write_context_presentation_key(content: &Content, out: &mut String) {
    if let Some(seq) = content.to_packed::<typst::foundations::SequenceElem>() {
        out.push_str("seq[");
        for child in &seq.children {
            write_context_presentation_key(child, out);
            out.push(';');
        }
        out.push(']');
    } else if let Some(styled) = content.to_packed::<StyledElem>() {
        out.push_str("styled(");
        write_styles_key(&styled.styles, out);
        out.push_str(")[");
        write_context_presentation_key(&styled.child, out);
        out.push(']');
    } else if let Some(par) = content.to_packed::<ParElem>() {
        out.push_str("par[");
        write_context_presentation_key(&par.body, out);
        out.push(']');
    } else if let Some(heading) = content.to_packed::<HeadingElem>() {
        out.push_str("heading(");
        out.push_str(&format!(
            "level={:?}:depth={}:offset={}",
            heading.level.get(StyleChain::default()),
            heading.depth.get(StyleChain::default()).get(),
            heading.offset.get(StyleChain::default())
        ));
        out.push_str(")[");
        write_context_presentation_key(&heading.body, out);
        out.push(']');
    } else if let Some(block) = content.to_packed::<BlockElem>() {
        out.push_str("block[");
        if let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default()) {
            write_context_presentation_key(&body, out);
        }
        out.push(']');
    } else if is_metadata_tag(content) || content.is::<TextElem>() || content.is::<SpaceElem>() {
    } else {
        let children = container_ops::semantic_diff_child_contents(content);
        if !children.is_empty() {
            out.push_str(content.func().name());
            out.push('[');
            for child in children {
                write_context_presentation_key(&child, out);
                out.push(';');
            }
            out.push(']');
        } else {
            out.push_str(content.func().name());
        }
    }
}

fn write_styles_key(styles: &Styles, out: &mut String) {
    out.push_str(&styles_key(styles));
}

fn styles_key(styles: &Styles) -> String {
    let mut out = String::new();
    for style in styles.iter() {
        if !is_presentation_style(style) {
            continue;
        }
        out.push_str(&format!("{style:?}"));
        out.push(';');
    }
    out
}

fn is_presentation_style(style: &Style) -> bool {
    style.property().is_some()
        && style.element().is_some_and(|element| {
            element == TextElem::ELEM
                || element == ParElem::ELEM
                || element == HeadingElem::ELEM
                || element == EquationElem::ELEM
                || element == RawElem::ELEM
        })
}

fn block_has_visual_decoration(block: &typst::foundations::Packed<BlockElem>) -> bool {
    let styles = StyleChain::default();
    if block.fill.get_cloned(styles).is_some() {
        return true;
    }
    let stroke = block.stroke.resolve(styles);
    stroke.left.is_some()
        || stroke.top.is_some()
        || stroke.right.is_some()
        || stroke.bottom.is_some()
}

fn is_opaque_visual_element_name(name: &str) -> bool {
    matches!(
        name,
        "rect" | "circle" | "ellipse" | "line" | "polygon" | "path" | "image"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::model::EmphElem;
    use typst::text::{SpaceElem, TextElem};

    #[test]
    fn content_key_presentation_distinguishes_formatting_from_visible_text() {
        let plain = TextElem::packed("same");
        let emph = EmphElem::new(TextElem::packed("same")).pack();

        assert_eq!(plain.plain_text(), emph.plain_text());
        assert_ne!(presentation_key(&plain), presentation_key(&emph));
    }

    #[test]
    fn content_key_normalized_visible_text_matches_whitespace_variants() {
        let left = Content::sequence([
            TextElem::packed("a"),
            SpaceElem::shared().clone(),
            TextElem::packed("b"),
        ]);
        let right = TextElem::packed("a b");

        assert!(normalized_visible_text_matches(&left, &right));
    }

    #[test]
    fn content_key_block_equality_remains_structural() {
        let plain = TextElem::packed("same");
        let emph = EmphElem::new(TextElem::packed("same")).pack();

        assert_ne!(BlockEqualityKey::new(plain), BlockEqualityKey::new(emph));
    }
}
