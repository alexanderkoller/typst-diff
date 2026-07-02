//! Convert a tree-shaped diff into annotated Typst [`Content`] ready for rendering.
//!
//! # Colour conventions
//!
//! | Change type       | Colour                  | Technique                    |
//! |-------------------|-------------------------|------------------------------|
//! | Inserted block    | Green text fill         | `TextElem::fill`             |
//! | Deleted block     | Red strikethrough       | `StrikeElem` on plain text   |
//! | Deleted equation  | Red cancel              | `CancelElem` inside equation |
//! | Modified word ins | Green (or blue if compact) | `TextElem::fill`           |
//! | Modified word del | Red strikethrough       | `StrikeElem`                 |
//!
//! # Page-style grouping
//!
//! Blocks that share the same `page_styles` are accumulated into a group and then
//! wrapped together in a single `styled_with_map(page_styles)` call. A new group is
//! started whenever the page styles change. This preserves `#set page(…)` boundaries
//! (margins, headers, footers) across section breaks in the diff output.

use typst::foundations::{Content, Smart, Style, Styles};
use typst::foundations::{SequenceElem, StyleChain, StyledElem};
use typst::layout::{Abs, BlockBody, BlockElem, PageElem, Rel, Sides};
use typst::math::{CancelElem, EquationElem};
use typst::model::{
    EmphElem, FigureCaption, FootnoteBody, FootnoteElem, HeadingElem, ParElem, ParbreakElem,
};
use typst::text::{LinebreakElem, RawLine, SpaceElem, StrikeElem, TextElem};
use typst::visualize::{Color, Stroke};

use crate::annotated::effective_render_content;
use crate::container_ops;
use crate::content_tree;
#[cfg(test)]
use crate::diff::BlockBaseProvenance;
use crate::diff::{
    DiffBlock, DiffBlockEdit, DiffRegionEdit, EditContent, PageRegionKind, RealizedEdit,
    RegionPath, RenderedRegionAlignment, RenderedRegionEdit, RenderedRegionWrapper, Token, WordOp,
};
use crate::style_context;
use crate::trace::{DebugEventSink, PipelineTraceEvent, emit_pipeline_trace_event};

fn green() -> Color {
    Color::from_u8(0, 180, 0, 255)
}
fn red() -> Color {
    Color::from_u8(220, 0, 0, 255)
}
fn blue() -> Color {
    Color::from_u8(0, 100, 220, 255)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChangeColor {
    Green,
    Blue,
}

impl ChangeColor {
    fn color(self) -> Color {
        match self {
            Self::Green => green(),
            Self::Blue => blue(),
        }
    }

    fn typst_hex(self) -> &'static str {
        match self {
            Self::Green => "#00b400",
            Self::Blue => "#0064dc",
        }
    }
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
    if let Some(styles) = page_styles
        && !styles.is_empty()
    {
        groups.push(group.styled_with_map(styles));
        return;
    }
    groups.push(group);
}

/// Turn a [`WordOp`] sequence into a flat inline `Content` sequence with colour annotations.
///
/// Deleted tokens become red `StrikeElem` (equations use `CancelElem`). Inserted tokens
/// become green. In compact mode, inserted tokens that are adjacent to a delete are
/// coloured blue and deleted modified-word runs are dropped entirely. A separator is inserted
/// before an inserted run when the surrounding tokens would otherwise be glued into
/// one word-like token.
fn annotated_inline_content(word_ops: &[WordOp], compact_substitutions: bool) -> Content {
    let mut inline: Vec<Content> = Vec::new();
    for run in word_render_runs(word_ops, compact_substitutions) {
        match run {
            WordRenderRun::Equal(tokens) => {
                for t in tokens {
                    inline.push(token_render_content(t));
                }
            }
            WordRenderRun::Insert {
                tokens,
                color,
                leading_separator,
                trailing_separator,
            } => {
                let joined = changed_token_sequence(tokens, leading_separator, trailing_separator);
                inline.push(joined.styled(TextElem::fill.set(color.color().into())));
            }
            WordRenderRun::Delete { tokens, visible } => {
                if visible {
                    inline.push(Content::sequence(tokens.iter().map(deleted_token_content)));
                }
            }
        }
    }
    Content::sequence(inline)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum WordRenderRun<'a> {
    Equal(&'a [Token]),
    Insert {
        tokens: &'a [Token],
        color: ChangeColor,
        leading_separator: bool,
        trailing_separator: bool,
    },
    Delete {
        tokens: &'a [Token],
        visible: bool,
    },
}

fn word_render_runs(word_ops: &[WordOp], compact: bool) -> Vec<WordRenderRun<'_>> {
    word_ops
        .iter()
        .enumerate()
        .map(|(index, op)| {
            let prev = index.checked_sub(1).and_then(|i| word_ops.get(i));
            let next = word_ops.get(index + 1);
            match op {
                WordOp::Equal(tokens) => WordRenderRun::Equal(tokens),
                WordOp::Insert(tokens) => {
                    let adjacent_delete = prev.is_some_and(|op| matches!(op, WordOp::Delete(_)))
                        || next.is_some_and(|op| matches!(op, WordOp::Delete(_)));
                    WordRenderRun::Insert {
                        tokens,
                        color: if compact && adjacent_delete {
                            ChangeColor::Blue
                        } else {
                            ChangeColor::Green
                        },
                        leading_separator: !compact
                            && prev
                                .and_then(tokens_before_insert)
                                .is_some_and(|prev| needs_separator(prev, tokens)),
                        trailing_separator: !compact
                            && next
                                .and_then(tokens_after_insert)
                                .is_some_and(|next| needs_separator(tokens, next)),
                    }
                }
                WordOp::Delete(tokens) => WordRenderRun::Delete {
                    tokens,
                    visible: !compact,
                },
            }
        })
        .collect()
}

fn tokens_before_insert(op: &WordOp) -> Option<&[crate::diff::Token]> {
    match op {
        WordOp::Equal(tokens) | WordOp::Delete(tokens) => Some(tokens),
        _ => None,
    }
}

fn tokens_after_insert(op: &WordOp) -> Option<&[crate::diff::Token]> {
    match op {
        WordOp::Equal(tokens) | WordOp::Delete(tokens) => Some(tokens),
        _ => None,
    }
}

fn changed_token_sequence(
    tokens: &[Token],
    leading_separator: bool,
    trailing_separator: bool,
) -> Content {
    let mut content: Vec<Content> = Vec::new();
    if leading_separator {
        content.push(SpaceElem::shared().clone());
    }
    content.extend(tokens.iter().map(token_render_content));
    if trailing_separator {
        content.push(SpaceElem::shared().clone());
    }
    Content::sequence(content)
}

fn needs_separator(left: &[crate::diff::Token], right: &[crate::diff::Token]) -> bool {
    let Some(left_text) = left.last().map(|token| token.text.as_str()) else {
        return false;
    };
    let Some(right_text) = right.first().map(|token| token.text.as_str()) else {
        return false;
    };
    let Some(left_char) = left_text.chars().next_back() else {
        return false;
    };
    let Some(right_char) = right_text.chars().next() else {
        return false;
    };
    !left_char.is_whitespace()
        && !right_char.is_whitespace()
        && !left_char.is_ascii_punctuation()
        && !right_char.is_ascii_punctuation()
}

fn deleted_token_content(token: &crate::diff::Token) -> Content {
    let content = token_render_content(token);
    if let Some(equation) = deleted_equation_token_content(&content) {
        return equation;
    }

    let content = if content.plain_text().is_empty() {
        TextElem::packed(token.text.as_str())
    } else {
        content
    };
    let colored = content.styled(TextElem::fill.set(red().into()));
    Content::new(StrikeElem::new(colored))
}

fn deleted_equation_token_content(content: &Content) -> Option<Content> {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let child = deleted_equation_token_content(&styled.child)?;
        return Some(child.styled_with_map(styled.styles.clone()));
    }

    let equation = content.to_packed::<EquationElem>()?;
    let body = equation
        .body
        .clone()
        .styled(TextElem::fill.set(red().into()));
    let cancelled = Content::new(
        CancelElem::new(body).with_stroke(Stroke::from_pair(red(), Abs::pt(0.6).into())),
    );
    Some(Content::new(
        EquationElem::new(cancelled).with_block(equation.block.get(StyleChain::default())),
    ))
}

fn token_render_content(token: &crate::diff::Token) -> Content {
    inline_token_content(&token.content, token.text.as_str())
}

fn inline_token_content(content: &Content, text: &str) -> Content {
    if let Some(styled) = content.to_packed::<StyledElem>() {
        let child = inline_token_content(&styled.child, text);
        return child.styled_with_map(styled.styles.clone());
    }

    if let Some(par) = content.to_packed::<ParElem>() {
        return inline_token_content(&par.body, text);
    }

    if let Some(heading) = content.to_packed::<HeadingElem>() {
        return inline_token_content(&heading.body, text);
    }

    if let Some(caption) = content.to_packed::<FigureCaption>() {
        return inline_token_content(&caption.body, text);
    }

    if let Some(block) = content.to_packed::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        return inline_token_content(&body, text);
    }

    if content.plain_text().is_empty() && !text.is_empty() {
        TextElem::packed(text)
    } else {
        content.clone()
    }
}

/// Graft `replacement` into the innermost text-bearing position of `template`.
///
/// This preserves the block's outer styling (e.g. heading level, custom paragraph
/// styles) while swapping out only the inline text content. The search recurses through
/// `ParElem`, `StyledElem`, and all-inline `SequenceElem` wrappers.
/// Returns `None` if no suitable injection site is found.
fn replace_text_container(template: &Content, replacement: &Content) -> Option<Content> {
    let mut content = template.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>()
        && let Some(child) = replace_text_container(
            &styled.child,
            &strip_inherited_styles(replacement, &styled.styles),
        )
    {
        styled.child = child;
        return Some(content);
    }

    if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = replacement.clone();
        return Some(content);
    }

    if let Some(heading) = content.to_packed_mut::<HeadingElem>() {
        heading.body = replacement.clone();
        return Some(content);
    }

    if let Some(caption) = content.to_packed_mut::<FigureCaption>() {
        caption.body = replacement.clone();
        return Some(content);
    }

    if let Some(footnote) = content.to_packed_mut::<FootnoteElem>()
        && let FootnoteBody::Content(body) = &footnote.body
    {
        footnote.body = FootnoteBody::Content(
            replace_text_container(body, replacement).unwrap_or_else(|| replacement.clone()),
        );
        return Some(content);
    }

    if let Some(block) = content.to_packed_mut::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
    {
        let body =
            replace_text_container(&body, replacement).unwrap_or_else(|| replacement.clone());
        block.body.set(Some(BlockBody::Content(body)));
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

fn strip_inherited_styles(content: &Content, inherited: &Styles) -> Content {
    if inherited.is_empty() {
        return content.clone();
    }

    let mut content = content.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        let child = strip_inherited_styles(&styled.child, inherited);
        let stripped = styles_without_inherited_sequence(&styled.styles, inherited);
        if let Some(remaining_styles) = stripped {
            if remaining_styles.is_empty() {
                return child;
            }
            styled.styles = remaining_styles;
        }
        styled.child = child;
        return content;
    }

    if let Some(mapped) = content_tree::map_transparent_children(&content, |child| {
        strip_inherited_styles(child, inherited)
    }) {
        return mapped;
    }

    content
}

fn strip_page_styles(content: &Content) -> Content {
    let mut content = content.clone();

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        let child = strip_page_styles(&styled.child);
        styled.styles = style_context::non_page_styles(&styled.styles);
        styled.child = child;
        if styled.styles.is_empty() {
            return styled.child.clone();
        }
        return content;
    }

    if let Some(mapped) = content_tree::map_transparent_children(&content, strip_page_styles) {
        return mapped;
    }

    content
}

fn styles_without_inherited_sequence(styles: &Styles, inherited: &Styles) -> Option<Styles> {
    let styles = styles.as_slice();
    let inherited = inherited.as_slice();
    let style_signatures: Vec<_> = styles
        .iter()
        .map(|style| style_signature_ignoring_provenance(style))
        .collect();
    let inherited_signatures: Vec<_> = inherited
        .iter()
        .map(|style| style_signature_ignoring_provenance(style))
        .collect();
    let start = styles
        .windows(inherited.len())
        .enumerate()
        .find(|(index, _)| {
            style_signatures[*index..*index + inherited.len()] == inherited_signatures
        })
        .map(|(index, _)| index)?;

    let mut remaining = Styles::new();
    for (index, style) in styles.iter().enumerate() {
        if index < start || index >= start + inherited.len() {
            remaining.push((**style).clone());
        }
    }
    Some(remaining)
}

fn style_signature_ignoring_provenance(style: &Style) -> String {
    // LazyHash equality includes realization provenance. For inherited-style
    // removal we only want the visible property or recipe that Debug exposes.
    format!("{style:?}")
}

fn is_inlineish(content: &Content) -> bool {
    !content.is::<ParElem>() && content.to_packed::<SequenceElem>().is_none()
}

/// Apply `fill` to the text content of `content` at the outer block level.
///
/// The realized-edit pipeline uses this for inserted edit payloads so structural
/// whitespace nodes remain bare.
fn apply_fill_inside(content: &Content, fill: Color) -> Content {
    if let Some(mapped) =
        content_tree::map_transparent_children(content, |child| apply_fill_inside(child, fill))
    {
        return mapped;
    }

    content.clone().styled(TextElem::fill.set(fill.into()))
}

fn apply_delete_inside(content: &Content) -> Content {
    if let Some(mapped) = content_tree::map_transparent_children(content, apply_delete_inside) {
        return mapped;
    }

    if content.plain_text().is_empty() {
        return content.clone();
    }

    let colored = content.clone().styled(TextElem::fill.set(red().into()));
    Content::new(StrikeElem::new(colored))
}

fn framed_opaque_visual(content: &Content, color: Color) -> Content {
    // Opaque replacements cannot expose word-level anchors, so render the old
    // and new visual payloads as framed block-level alternatives.
    Content::new(
        BlockElem::new()
            .with_width(Smart::Custom(Rel::one()))
            .with_stroke(Sides::splat(Some(Some(Stroke::from_pair(
                color,
                Abs::pt(0.8).into(),
            )))))
            .with_inset(Sides::splat(Some(Abs::pt(2.0).into())))
            .with_body(Some(BlockBody::Content(content.clone()))),
    )
}

/// Build annotated content from the new tree-shaped [`crate::diff::DiffResult`].
pub fn build_annotated_content_from_tree(
    result: &crate::diff::DiffResult,
    compact_substitutions: bool,
) -> Content {
    let mut no_debug_events = None;
    build_annotated_content_from_tree_inner(result, compact_substitutions, &mut no_debug_events)
        .expect("annotation without trace cannot fail")
}

pub fn build_annotated_content_from_tree_with_debug_events(
    result: &crate::diff::DiffResult,
    compact_substitutions: bool,
    debug_events: &mut dyn DebugEventSink,
) -> anyhow::Result<Content> {
    let mut debug_events = Some(debug_events);
    build_annotated_content_from_tree_inner(result, compact_substitutions, &mut debug_events)
}

fn build_annotated_content_from_tree_inner(
    result: &crate::diff::DiffResult,
    compact_substitutions: bool,
    debug_events: &mut Option<&mut dyn DebugEventSink>,
) -> anyhow::Result<Content> {
    let mut groups: Vec<Content> = Vec::new();
    let mut current_blocks: Vec<Content> = Vec::new();
    let mut current_page_styles: Option<typst::foundations::Styles> = None;
    let page_region_updates = page_region_updates(
        &result.regions,
        &result.rendered_regions,
        compact_substitutions,
    );
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("annotate/page-regions", "updates")
            .reason(format!("count={}", page_region_updates.len())),
    )?;

    for (index, block) in result.blocks.iter().enumerate() {
        let mut annotated_block = annotate_block_edit(block, compact_substitutions);
        apply_page_region_updates(
            &mut annotated_block.page_styles,
            &page_region_updates,
            PageRegionUpdateScope::ExistingOnly,
        );
        if current_page_styles
            .as_ref()
            .is_some_and(|s| s != &annotated_block.page_styles)
        {
            flush_group(&mut groups, &mut current_blocks, current_page_styles.take());
        }
        current_page_styles.get_or_insert_with(|| annotated_block.page_styles.clone());
        current_blocks.push(annotated_block.content);
        emit_pipeline_trace_event(
            debug_events,
            PipelineTraceEvent::new("annotate/block", "annotated")
                .new_block_index(index)
                .reason(format!("current_group_blocks={}", current_blocks.len())),
        )?;
    }
    flush_group(&mut groups, &mut current_blocks, current_page_styles);
    let mut root_styles = result.root_styles.clone();
    apply_page_region_updates(
        &mut root_styles,
        &page_region_updates,
        PageRegionUpdateScope::All,
    );
    emit_pipeline_trace_event(
        debug_events,
        PipelineTraceEvent::new("annotate/groups", "complete")
            .reason(format!("group_count={}", groups.len())),
    )?;
    let content = Content::sequence(groups);
    let content = if root_styles.is_empty() {
        content
    } else {
        content.styled_with_map(root_styles)
    };
    Ok(crate::normalize::normalize_list_item_runs(content))
}

fn annotate_block_edit(block: &DiffBlockEdit, compact: bool) -> DiffBlock {
    DiffBlock {
        content: apply_edits_to_base(&block.base, &block.edits, compact),
        page_styles: block.page_styles.clone(),
    }
}

pub(crate) fn apply_edits_to_base(
    base: &crate::annotated::AnnotatedContent,
    edits: &[RealizedEdit],
    compact: bool,
) -> Content {
    let mut node = base.clone();
    let mut index = 0;
    while index < edits.len() {
        if let Some((before, anchor)) = insertion_anchor(&edits[index]) {
            let mut end = index + 1;
            while end < edits.len()
                && insertion_anchor(&edits[end]).is_some_and(|(next_before, next_anchor)| {
                    next_before == before && next_anchor == anchor
                })
            {
                end += 1;
            }
            for edit in edits[index..end].iter().rev() {
                apply_realized_edit(&mut node, edit, compact);
            }
            index = end;
        } else {
            apply_realized_edit(&mut node, &edits[index], compact);
            index += 1;
        }
    }
    effective_render_content(&node)
}

struct PageRegionUpdate {
    kind: PageRegionKind,
    content: Content,
}

#[derive(Clone, Copy)]
enum PageRegionUpdateScope {
    All,
    ExistingOnly,
}

fn page_region_updates(
    regions: &[DiffRegionEdit],
    rendered_regions: &[RenderedRegionEdit],
    compact: bool,
) -> Vec<PageRegionUpdate> {
    let mut updates = Vec::new();
    for region in regions {
        let content = apply_edits_to_base(&region.base, &region.edits, compact);
        match region.path {
            RegionPath::RootPage(kind) => updates.push(PageRegionUpdate { kind, content }),
        }
    }
    updates.extend(rendered_regions.iter().map(|region| PageRegionUpdate {
        kind: region.kind,
        content: rendered_region_context_content(region, compact),
    }));
    updates
}

fn apply_page_region_updates(
    styles: &mut Styles,
    updates: &[PageRegionUpdate],
    scope: PageRegionUpdateScope,
) {
    if styles.is_empty() && matches!(scope, PageRegionUpdateScope::ExistingOnly) {
        return;
    }
    for update in updates {
        if matches!(scope, PageRegionUpdateScope::ExistingOnly)
            && !page_styles_has_region(styles, update.kind)
        {
            continue;
        }
        set_page_region(styles, update.kind, update.content.clone());
    }
}

fn page_styles_has_region(page_styles: &Styles, kind: PageRegionKind) -> bool {
    match kind {
        PageRegionKind::Header => page_styles.has(PageElem::header),
        PageRegionKind::Footer => page_styles.has(PageElem::footer),
        PageRegionKind::Background => page_styles.has(PageElem::background),
        PageRegionKind::Foreground => page_styles.has(PageElem::foreground),
    }
}

fn set_page_region(styles: &mut Styles, kind: PageRegionKind, content: Content) {
    match kind {
        PageRegionKind::Header => {
            styles.push(PageElem::header.set(Smart::Custom(Some(marginal_content(content)))))
        }
        PageRegionKind::Footer => {
            styles.push(PageElem::footer.set(Smart::Custom(Some(marginal_content(content)))))
        }
        PageRegionKind::Background => styles.push(PageElem::background.set(Some(content))),
        PageRegionKind::Foreground => styles.push(PageElem::foreground.set(Some(content))),
    }
}

fn rendered_region_context_content(region: &RenderedRegionEdit, compact: bool) -> Content {
    let mut source = String::from("#context {\n  let p = counter(page).get().first()\n");
    for page in &region.pages {
        source.push_str(&format!(
            "  {} p == {} {{ ",
            if page.page == 1 { "if" } else { "else if" },
            page.page
        ));
        push_rendered_region_wrapper_start(&mut source, region.wrapper);
        source.push_str(&rendered_region_page_markup(page, compact));
        push_rendered_region_wrapper_end(&mut source, region.wrapper);
        source.push_str(" }\n");
    }
    if let Some(last) = region.pages.last() {
        source.push_str("  else { ");
        push_rendered_region_wrapper_start(&mut source, region.wrapper);
        source.push_str(&typst_escape_content(last.base.plain_text().as_str()));
        push_rendered_region_wrapper_end(&mut source, region.wrapper);
        source.push_str(" }\n");
    }
    source.push_str("}\n");
    crate::eval::eval_snippet_to_content(&source).expect("generated rendered region Typst is valid")
}

fn rendered_region_page_markup(
    page: &crate::diff::RenderedRegionPageEdit,
    compact: bool,
) -> String {
    if page.segments.len() <= 1 {
        return word_ops_typst_markup(&page.word_ops, compact);
    }

    page.segments
        .iter()
        .map(|segment| word_ops_typst_markup(&segment.word_ops, compact))
        .collect::<Vec<_>>()
        .join(" #h(1fr) ")
}

fn push_rendered_region_wrapper_start(source: &mut String, wrapper: RenderedRegionWrapper) {
    match wrapper {
        RenderedRegionWrapper::None => source.push('['),
        RenderedRegionWrapper::Align(alignment) => {
            source.push_str("align(");
            source.push_str(rendered_region_alignment_name(alignment));
            source.push_str(")[");
        }
    }
}

fn push_rendered_region_wrapper_end(source: &mut String, _wrapper: RenderedRegionWrapper) {
    source.push(']');
}

fn rendered_region_alignment_name(alignment: RenderedRegionAlignment) -> &'static str {
    match alignment {
        RenderedRegionAlignment::Left => "left",
        RenderedRegionAlignment::Center => "center",
        RenderedRegionAlignment::Right => "right",
        RenderedRegionAlignment::Start => "start",
        RenderedRegionAlignment::End => "end",
    }
}

fn word_ops_typst_markup(word_ops: &[WordOp], compact: bool) -> String {
    let mut out = String::new();
    for run in word_render_runs(word_ops, compact) {
        match run {
            WordRenderRun::Equal(tokens) => {
                for token in tokens {
                    out.push_str(&content_typst_markup(&token.content));
                }
            }
            WordRenderRun::Insert {
                tokens,
                color,
                leading_separator,
                trailing_separator,
            } => {
                out.push_str(&format!(
                    "#text(fill: rgb(\"{}\"))[{}]",
                    color.typst_hex(),
                    changed_tokens_typst_markup(tokens, leading_separator, trailing_separator)
                ));
            }
            WordRenderRun::Delete { tokens, visible } => {
                if visible {
                    out.push_str(&format!(
                        "#strike[#text(fill: rgb(\"#dc0000\"))[{}]]",
                        tokens_typst_markup(tokens)
                    ));
                }
            }
        }
    }
    out
}

fn changed_tokens_typst_markup(
    tokens: &[Token],
    leading_separator: bool,
    trailing_separator: bool,
) -> String {
    let mut text = String::new();
    if leading_separator {
        text.push(' ');
    }
    text.push_str(&tokens_typst_markup(tokens));
    if trailing_separator {
        text.push(' ');
    }
    text
}

fn tokens_typst_markup(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|token| content_typst_markup(&token_render_content(token)))
        .collect()
}

fn content_typst_markup(content: &Content) -> String {
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .map(content_typst_markup)
            .collect::<String>();
    }
    if let Some(emph) = content.to_packed::<EmphElem>() {
        return format!("#emph[{}]", content_typst_markup(&emph.body));
    }
    typst_escape_content(content.plain_text().as_str())
}

fn typst_escape_content(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\\' | '[' | ']' | '#' => ['\\', c].into_iter().collect::<Vec<_>>(),
            _ => [c].into_iter().collect(),
        })
        .collect()
}

fn marginal_content(content: Content) -> Content {
    Content::new(
        BlockElem::new()
            .with_width(Smart::Custom(Rel::one()))
            .with_body(Some(BlockBody::Content(
                content.styled(ParElem::justify.set(false)),
            ))),
    )
}

fn insertion_anchor(edit: &RealizedEdit) -> Option<(bool, &[usize])> {
    match edit {
        RealizedEdit::InsertBefore { anchor, .. } => Some((true, anchor)),
        RealizedEdit::InsertAfter { anchor, .. } => Some((false, anchor)),
        RealizedEdit::LogOnly(_) | RealizedEdit::MarkBaseInserted(_) => None,
        _ => None,
    }
}

enum PathEdit {
    Replace(Content),
    Insert { content: Content, before: bool },
}

fn apply_realized_edit(
    node: &mut crate::annotated::AnnotatedContent,
    edit: &RealizedEdit,
    compact: bool,
) {
    match edit {
        RealizedEdit::ReplaceAt { path, content } => {
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = replace_annotated_path_content(node, path, rendered.clone()) {
                node.annotation.patch_surface = Some(if modified_is_pure_insert(content) {
                    patched
                } else {
                    strip_leading_parbreak(patched)
                });
            }
            if let Some(child) = node.get_path_mut(path) {
                child.realized = rendered;
                child.annotation.patch_surface = None;
            }
        }
        RealizedEdit::InsertBefore { anchor, content } => {
            if compact && is_deleted_edit_content(content) {
                return;
            }
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = insert_annotated_path_content(node, anchor, rendered, true) {
                node.annotation.patch_surface = Some(patched);
            }
        }
        RealizedEdit::InsertAfter { anchor, content } => {
            if compact && is_deleted_edit_content(content) {
                return;
            }
            let rendered = render_edit_content(content, compact);
            if let Some(patched) = insert_annotated_path_content(node, anchor, rendered, false) {
                node.annotation.patch_surface = Some(patched);
            }
        }
        RealizedEdit::Append { content } => {
            if compact && is_deleted_edit_content(content) {
                return;
            }
            let rendered = render_edit_content(content, compact);
            let base = effective_render_content(node);
            node.annotation.patch_surface = Some(Content::sequence([base, rendered]));
        }
        RealizedEdit::WholeBlock(content) => {
            node.realized = render_edit_content(content, compact);
            node.annotation.patch_surface = None;
            node.children.clear();
        }
        RealizedEdit::LogOnly(_) => {}
        RealizedEdit::MarkBaseInserted(_) => {
            node.realized = apply_fill_inside(&node.realized, green());
            node.annotation.patch_surface = None;
        }
    }
}

fn is_deleted_edit_content(content: &EditContent) -> bool {
    matches!(content, EditContent::Deleted(_))
}

fn modified_is_pure_insert(content: &EditContent) -> bool {
    matches!(content, EditContent::Modified { word_ops, .. } if word_ops.iter().all(|op| !matches!(op, WordOp::Delete(_))) && word_ops.iter().any(|op| matches!(op, WordOp::Insert(_))))
}

fn strip_leading_parbreak(content: Content) -> Content {
    let mut content = content;
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        if seq
            .children
            .first()
            .is_some_and(|child| child.is::<ParbreakElem>())
        {
            seq.children.remove(0);
        }
        seq.children = seq
            .children
            .iter()
            .cloned()
            .map(strip_leading_parbreak)
            .collect();
        return content;
    }
    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = strip_leading_parbreak(styled.child.clone());
    }
    content
}

fn render_edit_content(content: &EditContent, compact: bool) -> Content {
    match content {
        EditContent::Inserted(content) => {
            if content.plain_text().is_empty() {
                strip_page_styles(content)
            } else {
                strip_page_styles(&apply_fill_inside(content, green()))
            }
        }
        EditContent::Deleted(content) => {
            strip_page_styles(&apply_delete_inside(content.as_content()))
        }
        EditContent::OpaqueReplacement { old, new } => Content::sequence([
            framed_opaque_visual(&strip_page_styles(old.as_content()), red()),
            framed_opaque_visual(&strip_page_styles(new), green()),
        ]),
        EditContent::Modified { base, word_ops } => {
            if let Some(raw) = render_raw_block_modified(base, word_ops, compact) {
                strip_page_styles(&raw)
            } else {
                let inline = annotated_inline_content(word_ops, compact);
                let rendered = replace_text_container(base, &inline).unwrap_or(inline);
                strip_page_styles(&rendered)
            }
        }
        EditContent::Nested { base, edits } => {
            strip_page_styles(&apply_edits_to_base(base, edits, compact))
        }
    }
}

fn render_raw_block_modified(
    base: &Content,
    word_ops: &[WordOp],
    compact: bool,
) -> Option<Content> {
    if !contains_raw_line(base) {
        return None;
    }
    replace_raw_line_sequence(base, raw_line_diff_sequence(word_ops, compact))
}

fn contains_raw_line(content: &Content) -> bool {
    let mut found = content.is::<RawLine>();
    let _ = content.traverse::<_, ()>(&mut |child| {
        if child.is::<RawLine>() {
            found = true;
            return std::ops::ControlFlow::Break(());
        }
        std::ops::ControlFlow::Continue(())
    });
    found
}

fn replace_raw_line_sequence(content: &Content, replacement: Content) -> Option<Content> {
    let mut content = content.clone();

    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        if seq.children.iter().any(|child| child.is::<RawLine>()) {
            seq.children = replacement
                .to_packed::<SequenceElem>()
                .map(|seq| seq.children.clone())
                .unwrap_or_else(|| vec![replacement].into_iter().collect());
            return Some(content);
        }
        for child in &mut seq.children {
            if let Some(replaced) = replace_raw_line_sequence(child, replacement.clone()) {
                *child = replaced;
                return Some(content);
            }
        }
        return None;
    }

    if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        if let Some(child) = replace_raw_line_sequence(&styled.child, replacement) {
            styled.child = child;
            return Some(content);
        }
        return None;
    }

    if let Some(block) = content.to_packed_mut::<BlockElem>()
        && let Some(BlockBody::Content(body)) = block.body.get_cloned(StyleChain::default())
        && let Some(body) = replace_raw_line_sequence(&body, replacement)
    {
        block.body.set(Some(BlockBody::Content(body)));
        return Some(content);
    }

    None
}

fn raw_line_diff_sequence(word_ops: &[WordOp], compact: bool) -> Content {
    let mut lines = Vec::new();
    for run in word_render_runs(word_ops, compact) {
        match run {
            WordRenderRun::Equal(tokens) => {
                lines.extend(
                    tokens
                        .iter()
                        .map(|token| raw_line_body(token.text.as_str())),
                );
            }
            WordRenderRun::Insert { tokens, color, .. } => {
                lines.extend(tokens.iter().map(|token| {
                    raw_line_body(token.text.as_str())
                        .styled(TextElem::fill.set(color.color().into()))
                }));
            }
            WordRenderRun::Delete { tokens, visible } => {
                if visible {
                    lines.extend(tokens.iter().map(|token| {
                        Content::new(StrikeElem::new(
                            raw_line_body(token.text.as_str())
                                .styled(TextElem::fill.set(red().into())),
                        ))
                    }));
                }
            }
        }
    }
    raw_lines_sequence(lines)
}

fn raw_line_body(text: &str) -> Content {
    TextElem::packed(text)
}

fn raw_lines_sequence(lines: Vec<Content>) -> Content {
    let count = lines.len() as i64;
    let mut children = Vec::new();
    for (index, body) in lines.into_iter().enumerate() {
        if index > 0 {
            children.push(Content::new(LinebreakElem::new()));
        }
        let text = body.plain_text();
        children.push(Content::new(RawLine::new(
            index as i64 + 1,
            count,
            text,
            body,
        )));
    }
    Content::sequence(children)
}

fn replace_annotated_path_content(
    node: &crate::annotated::AnnotatedContent,
    path: &[usize],
    replacement: Content,
) -> Option<Content> {
    let path = patch_path_for_logical_path(node, path).unwrap_or_else(|| path.to_vec());
    let path = path.as_slice();
    let Some((index, _)) = path.split_first() else {
        return apply_path_edit(render_surface(node), path, PathEdit::Replace(replacement));
    };
    let surface = patchable_surface_for_index(node, *index)?;
    apply_path_edit(&surface, path, PathEdit::Replace(replacement))
}

fn patch_path_for_logical_path(
    node: &crate::annotated::AnnotatedContent,
    path: &[usize],
) -> Option<Vec<usize>> {
    node.annotation
        .slots
        .iter()
        .find(|slot| slot.path == path)
        .and_then(|slot| slot.patch_path.clone())
}

fn insert_annotated_path_content(
    node: &crate::annotated::AnnotatedContent,
    path: &[usize],
    insertion: Content,
    before: bool,
) -> Option<Content> {
    apply_path_edit(
        render_surface(node),
        path,
        PathEdit::Insert {
            content: insertion,
            before,
        },
    )
}

fn apply_path_edit(surface: &Content, path: &[usize], edit: PathEdit) -> Option<Content> {
    let Some((index, rest)) = path.split_first() else {
        return match edit {
            PathEdit::Replace(content) => Some(content),
            PathEdit::Insert { .. } => None,
        };
    };
    if rest.is_empty() {
        return match edit {
            PathEdit::Replace(content) => {
                container_ops::replace_realized_child(surface, *index, content)
            }
            PathEdit::Insert { content, before } => {
                container_ops::insert_realized_child(surface, *index, content, before)
            }
        };
    }
    let surface_child = container_ops::realized_child_contents(surface)
        .get(*index)
        .cloned()?;
    let patched_child = apply_path_edit(&surface_child, rest, edit)?;
    container_ops::replace_realized_child(surface, *index, patched_child)
}

fn render_surface(node: &crate::annotated::AnnotatedContent) -> &Content {
    node.annotation
        .patch_surface
        .as_ref()
        .unwrap_or(&node.realized)
}

fn patchable_surface_for_index(
    node: &crate::annotated::AnnotatedContent,
    index: usize,
) -> Option<Content> {
    let surface = render_surface(node);
    if container_ops::realized_child_contents(surface)
        .get(index)
        .is_some()
    {
        return Some(surface.clone());
    }
    (index < node.children.len())
        .then(|| Content::sequence(node.children.iter().map(effective_render_content)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotated::{AnnotatedContent, Annotation, annotate_realized};
    use crate::diff::{
        DiffBlockEdit, DiffResult, EditContent, OldDisplaySurface, RealizedEdit, Token, WordOp,
    };
    use typst::foundations::{NativeElement, Packed};
    use typst::layout::BlockElem;
    use typst::model::{HeadingElem, ListElem, ListItem};
    use typst::text::TextElem;

    fn word_token(s: &str) -> Token {
        Token {
            text: s.to_string(),
            content: TextElem::packed(s),
        }
    }

    fn annotated(content: Content) -> AnnotatedContent {
        AnnotatedContent {
            realized: content,
            annotation: Annotation::default(),
            children: vec![],
        }
    }

    fn render(base: Content, edits: Vec<RealizedEdit>) -> Content {
        render_with_compact(base, edits, false)
    }

    fn render_with_compact(base: Content, edits: Vec<RealizedEdit>, compact: bool) -> Content {
        let result = DiffResult {
            blocks: vec![DiffBlockEdit {
                base: annotated(base),
                base_provenance: BlockBaseProvenance::LiveNew,
                edits,
                page_styles: Default::default(),
            }],
            root_styles: Default::default(),
            regions: vec![],
            rendered_regions: vec![],
        };
        build_annotated_content_from_tree(&result, compact)
    }

    fn modified(base: Content, word_ops: Vec<WordOp>) -> EditContent {
        EditContent::Modified { base, word_ops }
    }

    fn deleted(content: Content) -> EditContent {
        EditContent::Deleted(OldDisplaySurface::new(content))
    }

    fn whole(content: EditContent) -> Vec<RealizedEdit> {
        vec![RealizedEdit::WholeBlock(content)]
    }

    fn replace_at(path: Vec<usize>, content: EditContent) -> Vec<RealizedEdit> {
        vec![RealizedEdit::ReplaceAt { path, content }]
    }

    fn count_elem<T: NativeElement>(content: &Content) -> usize {
        let mut count = 0;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if c.is::<T>() {
                count += 1;
            }
            std::ops::ControlFlow::Continue(())
        });
        count
    }

    fn count_page_style_wrappers(content: &Content) -> usize {
        let mut count = 0;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if let Some(styled) = c.to_packed::<StyledElem>()
                && styled.styles.iter().any(|style| {
                    style
                        .element()
                        .is_some_and(|element| element == PageElem::ELEM)
                })
            {
                count += 1;
            }
            std::ops::ControlFlow::Continue(())
        });
        count
    }

    #[test]
    fn word_render_runs_share_compact_substitution_policy() {
        let old = word_token("old");
        let new = word_token("new");
        let ops = vec![
            WordOp::Delete(vec![old.clone()]),
            WordOp::Insert(vec![new.clone()]),
        ];
        let runs = word_render_runs(&ops, true);

        assert!(matches!(
            runs.as_slice(),
            [
                WordRenderRun::Delete { visible: false, .. },
                WordRenderRun::Insert {
                    color: ChangeColor::Blue,
                    leading_separator: false,
                    trailing_separator: false,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn word_render_runs_share_separator_policy() {
        let ops = vec![
            WordOp::Equal(vec![word_token("matter")]),
            WordOp::Insert(vec![word_token("entirely")]),
        ];
        let runs = word_render_runs(&ops, false);

        assert!(matches!(
            runs.as_slice(),
            [
                WordRenderRun::Equal(_),
                WordRenderRun::Insert {
                    color: ChangeColor::Green,
                    leading_separator: true,
                    trailing_separator: false,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn apply_path_edit_replaces_and_inserts_list_children() {
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Gamma"))),
        ]));

        let replaced =
            apply_path_edit(&list, &[1], PathEdit::Replace(TextElem::packed("Beta"))).unwrap();
        let replaced_list = replaced.to_packed::<ListElem>().unwrap();
        assert_eq!(replaced_list.children[1].body.plain_text(), "Beta");

        let inserted = apply_path_edit(
            &list,
            &[1],
            PathEdit::Insert {
                content: TextElem::packed("Beta"),
                before: true,
            },
        )
        .unwrap();
        let inserted_list = inserted.to_packed::<ListElem>().unwrap();
        assert_eq!(inserted_list.children.len(), 3);
        assert_eq!(inserted_list.children[1].body.plain_text(), "Beta");
        assert_eq!(inserted_list.children[2].body.plain_text(), "Gamma");
    }

    #[test]
    fn page_region_updates_apply_to_root_and_existing_page_styles() {
        let updates = vec![
            PageRegionUpdate {
                kind: PageRegionKind::Header,
                content: TextElem::packed("New header"),
            },
            PageRegionUpdate {
                kind: PageRegionKind::Footer,
                content: TextElem::packed("New footer"),
            },
        ];

        let mut page_styles = Styles::new();
        page_styles.push(PageElem::header.set(Smart::Custom(Some(TextElem::packed("Old")))));
        apply_page_region_updates(
            &mut page_styles,
            &updates,
            PageRegionUpdateScope::ExistingOnly,
        );
        let chain = StyleChain::new(&page_styles);
        assert_eq!(
            chain
                .get_cloned(PageElem::header)
                .custom()
                .flatten()
                .unwrap()
                .plain_text(),
            "New header"
        );
        assert!(
            chain
                .get_cloned(PageElem::footer)
                .custom()
                .flatten()
                .is_none()
        );

        let mut root_styles = Styles::new();
        apply_page_region_updates(&mut root_styles, &updates, PageRegionUpdateScope::All);
        let chain = StyleChain::new(&root_styles);
        assert_eq!(
            chain
                .get_cloned(PageElem::header)
                .custom()
                .flatten()
                .unwrap()
                .plain_text(),
            "New header"
        );
        assert_eq!(
            chain
                .get_cloned(PageElem::footer)
                .custom()
                .flatten()
                .unwrap()
                .plain_text(),
            "New footer"
        );
    }

    #[test]
    fn inserted_block_wrapped_green() {
        let content = render(
            TextElem::packed("New paragraph"),
            whole(EditContent::Inserted(TextElem::packed("New paragraph"))),
        );
        assert!(!content.is_empty());
    }

    #[test]
    fn modified_block_contains_strike_for_deletion() {
        let content = render(
            TextElem::packed("The new text."),
            whole(modified(
                TextElem::packed("The new text."),
                vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ],
            )),
        );
        assert!(!content.is_empty());
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
    }

    #[test]
    fn deleted_heading_keeps_heading_formatting() {
        let heading = Content::new(HeadingElem::new(TextElem::packed("Old heading")));
        let content = render(heading.clone(), whole(deleted(heading)));

        assert_eq!(count_elem::<HeadingElem>(&content), 1);
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
        assert!(content.plain_text().contains("Old heading"));
    }

    #[test]
    fn deleted_semantic_heading_block_keeps_block_formatting() {
        let heading_block = Content::new(typst::layout::BlockElem::new().with_body(Some(
            typst::layout::BlockBody::Content(TextElem::packed("Old heading")),
        )));
        let content = render(heading_block.clone(), whole(deleted(heading_block)));

        assert_eq!(count_elem::<BlockElem>(&content), 1);
        assert_eq!(count_elem::<StrikeElem>(&content), 1);
        assert!(content.plain_text().contains("Old heading"));
    }

    #[test]
    fn compact_substitutions_drop_deleted_text_and_color_inserted_text() {
        let compact = render_with_compact(
            TextElem::packed("The new text."),
            whole(modified(
                TextElem::packed("The new text."),
                vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ],
            )),
            true,
        );

        assert_eq!(count_elem::<StrikeElem>(&compact), 0);
        assert!(compact.plain_text().contains("new"));
        assert!(!compact.plain_text().contains("old"));
    }

    #[test]
    fn modified_heading_preserves_heading_element() {
        let heading = Content::new(HeadingElem::new(TextElem::packed("New heading")));
        let annotated = render(
            heading.clone(),
            whole(modified(
                heading,
                vec![
                    WordOp::Delete(vec![word_token("Old")]),
                    WordOp::Insert(vec![word_token("New")]),
                    WordOp::Equal(vec![word_token(" heading")]),
                ],
            )),
        );

        assert_eq!(count_elem::<HeadingElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn modified_list_child_preserves_list_and_unchanged_items() {
        let list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(TextElem::packed("Alpha"))),
            Packed::new(ListItem::new(TextElem::packed("Better"))),
            Packed::new(ListItem::new(TextElem::packed("Gamma"))),
        ]));
        let annotated = render(
            list,
            replace_at(
                vec![1],
                modified(
                    TextElem::packed("Better"),
                    vec![
                        WordOp::Delete(vec![word_token("Beta")]),
                        WordOp::Insert(vec![word_token("Better")]),
                    ],
                ),
            ),
        );
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<ListElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Alpha"), "{plain}");
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Gamma"), "{plain}");
    }

    #[test]
    fn modified_table_child_preserves_table_structure() {
        use typst::model::{TableCell, TableChild, TableElem, TableItem};

        let table = Content::new(TableElem::new(vec![
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("Metric"),
            )))),
            TableChild::Item(TableItem::Cell(Packed::new(TableCell::new(
                TextElem::packed("New value"),
            )))),
        ]));
        let annotated = render(
            table,
            replace_at(
                vec![1],
                modified(
                    TextElem::packed("New value"),
                    vec![
                        WordOp::Delete(vec![word_token("Old")]),
                        WordOp::Insert(vec![word_token("New")]),
                        WordOp::Equal(vec![word_token(" value")]),
                    ],
                ),
            ),
        );

        assert_eq!(count_elem::<TableElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(annotated.plain_text().contains("Metric"));
        assert!(annotated.plain_text().contains("Old"));
        assert!(annotated.plain_text().contains("New"));
    }

    #[test]
    fn nested_list_child_preserves_both_list_levels_and_annotates_leaf() {
        let inner = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            TextElem::packed("Better"),
        ))]));
        let outer_list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(inner)),
            Packed::new(ListItem::new(TextElem::packed("Stable"))),
        ]));

        let annotated = render(
            outer_list,
            replace_at(
                vec![0, 0],
                modified(
                    TextElem::packed("Better"),
                    vec![
                        WordOp::Delete(vec![word_token("Beta")]),
                        WordOp::Insert(vec![word_token("Better")]),
                    ],
                ),
            ),
        );
        let plain = annotated.plain_text();

        assert_eq!(
            count_elem::<ListElem>(&annotated),
            2,
            "both list levels preserved"
        );
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Stable"), "{plain}");
    }

    #[test]
    fn nested_enum_child_preserves_enum_container_and_annotates_leaf() {
        use typst::model::{EnumElem, EnumItem};

        let enm = Content::new(EnumElem::new(vec![Packed::new(EnumItem::new(
            TextElem::packed("Better"),
        ))]));
        let outer_list = Content::new(ListElem::new(vec![
            Packed::new(ListItem::new(enm.clone())),
            Packed::new(ListItem::new(TextElem::packed("Stable"))),
        ]));

        let annotated = render(
            outer_list,
            replace_at(
                vec![0],
                EditContent::Nested {
                    base: annotated(enm),
                    edits: replace_at(
                        vec![0],
                        modified(
                            TextElem::packed("Better"),
                            vec![
                                WordOp::Delete(vec![word_token("Beta")]),
                                WordOp::Insert(vec![word_token("Better")]),
                            ],
                        ),
                    ),
                },
            ),
        );
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<ListElem>(&annotated), 1);
        assert_eq!(count_elem::<EnumElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Beta"), "{plain}");
        assert!(plain.contains("Better"), "{plain}");
        assert!(plain.contains("Stable"), "{plain}");
    }

    #[test]
    fn nested_terms_child_preserves_terms_container_and_annotates_description() {
        use typst::model::{TableCell, TableChild, TableElem, TableItem, TermItem, TermsElem};

        let terms = Content::new(TermsElem::new(vec![Packed::new(TermItem::new(
            TextElem::packed("Habitat"),
            TextElem::packed("New range"),
        ))]));
        let table = Content::new(TableElem::new(vec![TableChild::Item(TableItem::Cell(
            Packed::new(TableCell::new(terms.clone())),
        ))]));

        let annotated = render(
            table,
            replace_at(
                vec![0],
                EditContent::Nested {
                    base: annotated(terms),
                    edits: replace_at(
                        vec![1],
                        modified(
                            TextElem::packed("New range"),
                            vec![
                                WordOp::Delete(vec![word_token("Old")]),
                                WordOp::Insert(vec![word_token("New")]),
                                WordOp::Equal(vec![word_token(" range")]),
                            ],
                        ),
                    ),
                },
            ),
        );
        let plain = annotated.plain_text();

        assert_eq!(count_elem::<TableElem>(&annotated), 1);
        assert_eq!(count_elem::<TermsElem>(&annotated), 1);
        assert_eq!(count_elem::<StrikeElem>(&annotated), 1);
        assert!(plain.contains("Habitat"), "{plain}");
        assert!(plain.contains("Old"), "{plain}");
        assert!(plain.contains("New"), "{plain}");
    }

    #[test]
    fn mixed_body_inline_change_detected_and_nested_structure_preserved() {
        use crate::diff::diff_annotated;
        use typst::model::ParbreakElem;

        let inner = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            TextElem::packed("Leaf"),
        ))]));
        let body_old = Content::sequence([
            TextElem::packed("old title"),
            Content::new(ParbreakElem::new()),
            inner.clone(),
        ]);
        let body_new = Content::sequence([
            TextElem::packed("new title"),
            Content::new(ParbreakElem::new()),
            inner.clone(),
        ]);
        let old = Content::new(ListElem::new(vec![Packed::new(ListItem::new(body_old))]));
        let new = Content::new(ListElem::new(vec![Packed::new(ListItem::new(body_new))]));

        let result = diff_annotated(
            &annotate_realized(&old, &old),
            &annotate_realized(&new, &new),
        );
        let annotated = build_annotated_content_from_tree(&result, false);
        let plain = annotated.plain_text();

        assert_eq!(
            count_elem::<ListElem>(&annotated),
            2,
            "both outer and nested list levels preserved"
        );
        assert!(plain.contains("old"), "{plain}");
        assert!(plain.contains("new"), "{plain}");
        assert!(plain.contains("Leaf"), "{plain}");
    }

    #[test]
    fn annotated_inline_content_inserts_separator_between_adjacent_changes() {
        let inline = annotated_inline_content(
            &[
                WordOp::Delete(vec![word_token("old")]),
                WordOp::Insert(vec![word_token("new")]),
            ],
            false,
        );

        assert_eq!(inline.plain_text(), "old new");
        assert_eq!(count_elem::<StrikeElem>(&inline), 1);
    }

    #[test]
    fn annotated_inline_content_inserts_separator_after_equal_word() {
        let inline = annotated_inline_content(
            &[
                WordOp::Equal(vec![
                    word_token("subject"),
                    word_token(" "),
                    word_token("matter"),
                ]),
                WordOp::Insert(vec![
                    word_token("entirely"),
                    word_token(" "),
                    word_token("different"),
                ]),
                WordOp::Equal(vec![word_token(".")]),
            ],
            false,
        );
        let plain = inline.plain_text();

        assert!(
            plain.contains("subject matter entirely different"),
            "{plain}"
        );
        assert!(!plain.contains("matterentirely"), "{plain}");
    }

    #[test]
    fn annotated_inline_content_inserts_separator_before_following_equal_word() {
        let inline = annotated_inline_content(
            &[
                WordOp::Equal(vec![word_token("This")]),
                WordOp::Delete(vec![word_token("plain")]),
                WordOp::Insert(vec![
                    word_token("proper"),
                    word_token(" "),
                    word_token("level-one"),
                ]),
                WordOp::Equal(vec![word_token("heading.")]),
            ],
            false,
        );
        let plain = inline.plain_text();

        assert!(plain.contains("proper level-one heading."), "{plain}");
        assert!(!plain.contains("level-oneheading"), "{plain}");
    }

    #[test]
    fn annotated_inline_content_does_not_insert_separator_before_punctuation() {
        let inline = annotated_inline_content(
            &[
                WordOp::Equal(vec![word_token("and")]),
                WordOp::Insert(vec![
                    word_token("."),
                    word_token("It"),
                    word_token(" "),
                    word_token("also"),
                ]),
            ],
            false,
        );

        assert_eq!(inline.plain_text(), "and.It also");
    }

    #[test]
    fn annotated_inline_content_inserts_separator_between_adjacent_numbers() {
        let inline = annotated_inline_content(
            &[
                WordOp::Equal(vec![word_token("Page 1 of ")]),
                WordOp::Delete(vec![word_token("2")]),
                WordOp::Insert(vec![word_token("3")]),
            ],
            false,
        );

        assert_eq!(inline.plain_text(), "Page 1 of 2 3");
    }

    #[test]
    fn annotated_inline_content_does_not_duplicate_deleted_trailing_space() {
        let inline = annotated_inline_content(
            &[
                WordOp::Delete(vec![
                    word_token("subject"),
                    word_token(" "),
                    word_token("matter"),
                    word_token(" "),
                ]),
                WordOp::Insert(vec![
                    word_token("entirely"),
                    word_token(" "),
                    word_token("different"),
                ]),
                WordOp::Equal(vec![word_token(" from Alpha.")]),
            ],
            false,
        );
        let plain = inline.plain_text();

        assert!(
            plain.contains("subject matter entirely different"),
            "{plain}"
        );
        assert!(!plain.contains("matter  entirely"), "{plain}");
        assert!(!plain.contains("matterentirely"), "{plain}");
    }

    #[test]
    fn inherited_style_stripping_ignores_realization_metadata() {
        let mut token_styles = Styles::new();
        token_styles.push(TextElem::fill.set(red().into()));
        let inherited = token_styles.clone().outside();
        let styled = TextElem::packed("same").styled_with_map(token_styles);
        let stripped = strip_inherited_styles(&styled, &inherited);

        assert_eq!(stripped.plain_text(), "same");
        assert!(
            !stripped.is::<StyledElem>(),
            "equivalent inherited styles should be removed even if realization metadata differs: {stripped:#?}"
        );
    }

    #[test]
    fn edit_payloads_do_not_carry_page_styles_into_containers() {
        let page_styled = TextElem::packed("New").styled(PageElem::flipped.set(true));
        let list = Content::new(ListElem::new(vec![Packed::new(ListItem::new(
            page_styled.clone(),
        ))]));

        let content = render(
            list,
            replace_at(vec![0], EditContent::Inserted(page_styled)),
        );

        assert_eq!(count_elem::<ListElem>(&content), 1);
        assert_eq!(count_page_style_wrappers(&content), 0);
    }

    #[test]
    fn inserted_parbreak_is_not_wrapped_in_styled_elem() {
        use typst::model::ParbreakElem;

        let parbreak = Content::new(ParbreakElem::new());
        assert!(
            parbreak.plain_text().is_empty(),
            "sanity: ParbreakElem has no text"
        );

        let content = render(parbreak.clone(), whole(EditContent::Inserted(parbreak)));
        assert!(
            content.is::<ParbreakElem>(),
            "inserted ParbreakElem must remain bare, got: {:?}",
            content.func().name()
        );
    }

    #[test]
    fn annotate_applies_whole_block_insert_edit() {
        let content = render(
            TextElem::packed("new text"),
            whole(EditContent::Inserted(TextElem::packed("new text"))),
        );
        assert!(!content.is_empty());
    }

    #[test]
    fn inserted_visible_block_still_gets_green_fill() {
        let text = TextElem::packed("Visible text");
        assert!(!text.plain_text().is_empty(), "sanity: TextElem has text");

        let content = render(text.clone(), whole(EditContent::Inserted(text)));
        assert!(
            !content.is::<typst::text::TextElem>(),
            "inserted visible block should be styled/colored, not a bare TextElem"
        );
    }
}
