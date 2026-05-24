//! Typst document evaluation: from source text to a fully-realized [`Content`] tree.
//!
//! Two public entry points are exposed:
//! - [`eval_to_content`] — shallow eval only (Typst AST → unevaluated `Content`).
//! - [`eval_to_realized_content`] — full pipeline used by the diff: eval → layout →
//!   realization, producing a stable content tree where show rules and counters have
//!   been expanded and footnote markers and equations are in their original form.
//!
//! # Why realization is needed
//!
//! The diff operates on the *semantic* content tree, not the raw layout output.
//! Typst's realization step expands show rules (e.g. `show heading: …`) so that the
//! resulting tree reflects the document's logical structure. Without it, two headings
//! with identical text but different show rules would look identical in the diff.
//!
//! # Preserved elements
//!
//! `EquationElem` and "slot container" nodes (lists, figures, tables, …) would become
//! opaque layout output after realization. They are captured by span before realization
//! and spliced back in afterwards, keeping them diffable as structured content. A span
//! can appear more than once when a function body expands repeatedly, so preserved
//! nodes are stored as per-span queues and restored in document order.

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use typst::ROUTINES;
use typst::World;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{
    Content, NativeElement, SequenceElem, Style, StyleChain, StyledElem, Styles, Target, TargetElem,
};
use typst::introspection::TagElem;
use typst::introspection::{Introspector, Locator};
use typst::layout::{HElem, PageElem, PagebreakElem};
use typst::math::EquationElem;
use typst::model::{DocumentInfo, FootnoteElem, HeadingElem, ParElem};
use typst::routines::{Arenas, RealizationKind};
use typst::syntax::Span;
use typst::text::TextElem;

use crate::content_slots::{
    extract_slots, is_slot_container, normalize_list_item_runs, replace_slot,
};
use crate::diag::format_diagnostics;

/// Evaluate the entry file and return the raw, unrealized [`Content`] tree.
///
/// This is a thin wrapper around `typst_eval::eval`. The result still contains
/// unevaluated show rules and unexpanded counters. Useful for tests but not for
/// diffing; prefer [`eval_to_realized_content`] for production use.
pub fn eval_to_content(world: &dyn World) -> Result<Content> {
    let source = world
        .source(world.main())
        .map_err(|e| anyhow::anyhow!("cannot read main source: {e}"))?;
    let mut sink = Sink::new();
    let traced = Traced::default();
    typst_eval::eval(
        &ROUTINES,
        world.track(),
        traced.track(),
        sink.track_mut(),
        Route::default().track(),
        &source,
    )
    .map(|module| module.content())
    .map_err(|errs| anyhow::anyhow!("eval failed:\n{}", format_diagnostics(world, &errs)))
}

/// Evaluate, layout, and realize the document into a stable [`Content`] tree.
///
/// Steps:
/// 1. Evaluate source → raw content (via [`eval_to_content`]).
/// 2. Normalize bare list/enum/term items into their container elements.
/// 3. Run up to 5 layout iterations to build a stable [`Introspector`]
///    (needed so that counters, references, and footnotes converge).
/// 4. Call `ROUTINES.realize` to expand show rules and finalize the tree.
/// 5. Restore pre-realization `EquationElem` and slot-container nodes by span.
/// 6. Restore original `FootnoteElem` nodes (realization replaces them with markers).
///
/// The returned `AnnotatedContent` wraps the realized `Content` with semantic
/// annotations built by walking the pre- and post-realization trees together.
pub fn eval_to_realized_content(world: &dyn World) -> Result<crate::annotated::AnnotatedContent> {
    let pre_content = normalize_list_item_runs(eval_to_content(world)?);
    let introspector = layout_introspector(world, &pre_content)?;
    let realized_content = realize_to_content(world, &pre_content, introspector)?;
    let mut annotated = crate::annotated::annotate_realized(&pre_content, &realized_content);
    let footnotes = collect_footnotes(&pre_content);
    let mut next = 0;
    crate::annotated::annotate_footnote_markers(&mut annotated, &footnotes, &mut next);
    Ok(annotated)
}

/// Run layout up to 5 times until the [`Introspector`] converges.
///
/// An `Introspector` feeds back into the layout engine for features that depend
/// on their own position (page references, footnote numbering, counters). Running
/// layout once produces an initial introspector; repeating with that introspector
/// lets position-dependent values stabilize. 5 iterations is Typst's standard limit.
fn layout_introspector(world: &dyn World, content: &Content) -> Result<Introspector> {
    let library = world.library();
    let base = StyleChain::new(&library.styles);
    let target = TargetElem::target.set(Target::Paged).wrap();
    let styles = base.chain(&target);

    let traced = Traced::default();
    let mut introspector = Introspector::default();
    let mut final_sink = Sink::new();

    for _ in 0..5 {
        let constraint = typst::comemo::Constraint::new();
        let mut sink = Sink::new();
        let mut engine = Engine {
            routines: &ROUTINES,
            world: world.track(),
            introspector: introspector.track_with(&constraint),
            traced: traced.track(),
            sink: sink.track_mut(),
            route: Route::default(),
        };

        let laid_out =
            typst_layout::layout_document(&mut engine, content, styles).map_err(|errs| {
                anyhow::anyhow!("layout failed:\n{}", format_diagnostics(world, &errs))
            })?;
        let next_introspector = laid_out.introspector.clone();
        let converged = constraint.validate(&next_introspector);

        final_sink = sink;
        introspector = next_introspector;

        if converged {
            break;
        }
    }

    let delayed = final_sink.delayed();
    if !delayed.is_empty() {
        return Err(anyhow::anyhow!(
            "layout errors:\n{}",
            format_diagnostics(world, &delayed)
        ));
    }

    Ok(introspector)
}

/// Expand show rules and finalize the content tree using a stable `introspector`.
///
/// After realization each top-level item is wrapped with only its non-page styles
/// (page styles are handled separately in [`build_annotated_content`]), except for
/// `PagebreakElem` nodes which get marginal styles. The whole sequence is then
/// wrapped with the root-level page styles so that margin/header/footer settings
/// from `#set page(…)` survive into annotation and rendering.
fn realize_to_content(
    world: &dyn World,
    content: &Content,
    introspector: Introspector,
) -> Result<Content> {
    let library = world.library();
    let target = TargetElem::target.set(Target::Paged).wrap();
    let base = StyleChain::new(&library.styles);
    let styles = base.chain(&target);
    let style_map = styles.to_map().outside();
    let root_page_styles = page_styles(&style_map);
    let styles = StyleChain::new(&style_map);
    let mut preserved = collect_preserved_by_span(content);
    let footnotes = collect_footnotes(content);

    let traced = Traced::default();
    let mut sink = Sink::new();
    let mut engine = Engine {
        routines: &ROUTINES,
        world: world.track(),
        introspector: introspector.track(),
        traced: traced.track(),
        sink: sink.track_mut(),
        route: Route::default(),
    };

    let arenas = Arenas::default();
    let mut info = DocumentInfo::default();
    let mut locator = Locator::root().split();
    let realized = (ROUTINES.realize)(
        RealizationKind::LayoutDocument { info: &mut info },
        &mut engine,
        &mut locator,
        &arenas,
        content,
        styles,
    )
    .map_err(|errs| anyhow::anyhow!("realize failed:\n{}", format_diagnostics(world, &errs)))?;

    let delayed = sink.delayed();
    if !delayed.is_empty() {
        return Err(anyhow::anyhow!(
            "realize errors:\n{}",
            format_diagnostics(world, &delayed)
        ));
    }

    let realized = Content::sequence(realized.iter().map(|(realized_content, styles)| {
        let content = restore_preserved((*realized_content).clone(), &mut preserved);
        let styles = if realized_content.is::<PagebreakElem>() {
            marginal_styles(&styles.to_map())
        } else {
            non_page_styles(styles.to_map())
        };
        content.styled_with_map(styles)
    }))
    .styled_with_map(root_page_styles);

    Ok(restore_footnote_markers(realized, &footnotes))
}

/// Walk the pre-realization tree and index every `EquationElem`, `HeadingElem`, and
/// slot-container node by span so they can be swapped back in after realization.
///
/// Typst may assign the same source span to multiple realized nodes when a
/// function body is expanded more than once. A plain `HashMap<Span, Content>`
/// would collapse those invocations and make every realized node restore to the
/// last preserved value. The queue keeps all preserved nodes for a span and lets
/// `restore_preserved` consume them in traversal order.
fn collect_preserved_by_span(content: &Content) -> HashMap<Span, VecDeque<Content>> {
    let mut preserved: HashMap<Span, VecDeque<Content>> = HashMap::new();
    let _ = content.traverse::<_, ()>(&mut |content| {
        if content.is::<EquationElem>()
            || content.is::<HeadingElem>()
            || is_slot_container(&content)
        {
            preserved
                .entry(content.span())
                .or_default()
                .push_back(content.clone());
        }
        std::ops::ControlFlow::Continue(())
    });
    preserved
}

/// Collect all `FootnoteElem` nodes in document order from the pre-realization tree.
///
/// Realization replaces `FootnoteElem` nodes with superscript number markers and
/// moves the note body to the page footer. `restore_footnote_markers` uses this
/// list to swap the markers back to the original structured elements so the diff
/// can see footnote text as diffable content rather than bare numbers.
fn collect_footnotes(content: &Content) -> Vec<Content> {
    let mut footnotes = Vec::new();
    let _ = content.traverse::<_, ()>(&mut |content| {
        if content.is::<FootnoteElem>() {
            footnotes.push(content);
        }
        std::ops::ControlFlow::Continue(())
    });
    footnotes
}

fn restore_footnote_markers(content: Content, footnotes: &[Content]) -> Content {
    let mut next = 0;
    restore_footnote_markers_inner(content, footnotes, &mut next)
}

fn restore_footnote_markers_inner(
    content: Content,
    footnotes: &[Content],
    next: &mut usize,
) -> Content {
    if footnotes.is_empty() {
        return content;
    }

    if is_footnote_marker(&content, *next + 1)
        && let Some(footnote) = footnotes.get(*next)
    {
        *next += 1;
        return footnote.clone();
    }

    let mut content = content;
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = restore_footnote_markers_in_sequence(&seq.children, footnotes, next);
    } else if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = restore_footnote_markers_inner(styled.child.clone(), footnotes, next);
    } else if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = restore_footnote_markers_inner(par.body.clone(), footnotes, next);
    }

    content
}

fn restore_footnote_markers_in_sequence(
    children: &[Content],
    footnotes: &[Content],
    next: &mut usize,
) -> Vec<Content> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < children.len() {
        if is_footnote_marker_deep(&children[i], *next + 1)
            && let Some(footnote) = footnotes.get(*next)
        {
            while out.last().is_some_and(is_footnote_scaffold) {
                out.pop();
            }
            out.push(footnote.clone());
            *next += 1;
            i += 1;
            while i < children.len() && is_footnote_scaffold(&children[i]) {
                i += 1;
            }
            continue;
        }

        out.push(restore_footnote_markers_inner(
            children[i].clone(),
            footnotes,
            next,
        ));
        i += 1;
    }

    out
}

fn is_footnote_marker_deep(content: &Content, number: usize) -> bool {
    if is_footnote_marker(content, number) {
        return true;
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return is_footnote_marker_deep(&styled.child, number);
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq
            .children
            .iter()
            .any(|child| is_footnote_marker_deep(child, number));
    }
    false
}

fn is_footnote_scaffold(content: &Content) -> bool {
    if content.is::<TagElem>() || content.is::<HElem>() {
        return true;
    }
    if let Some(styled) = content.to_packed::<StyledElem>() {
        return is_footnote_scaffold(&styled.child);
    }
    if let Some(seq) = content.to_packed::<SequenceElem>() {
        return seq.children.iter().all(is_footnote_scaffold);
    }
    false
}

fn is_footnote_marker(content: &Content, number: usize) -> bool {
    content
        .to_packed::<TextElem>()
        .is_some_and(|text| text.text.as_str() == number.to_string())
}

fn restore_preserved(
    content: Content,
    preserved: &mut HashMap<Span, VecDeque<Content>>,
) -> Content {
    // Preserve repeated function expansions with identical spans by consuming
    // one queued replacement per realized node, in document order.
    if let Some(replacements) = preserved.get_mut(&content.span())
        && let Some(replacement) = replacements.pop_front()
    {
        return replacement;
    }

    let mut content = content;
    if let Some(seq) = content.to_packed_mut::<SequenceElem>() {
        seq.children = seq
            .children
            .iter()
            .cloned()
            .map(|child| restore_preserved(child, preserved))
            .collect();
    } else if let Some(styled) = content.to_packed_mut::<StyledElem>() {
        styled.child = restore_preserved(styled.child.clone(), preserved);
    } else if let Some(par) = content.to_packed_mut::<ParElem>() {
        par.body = restore_preserved(par.body.clone(), preserved);
    } else {
        for slot in extract_slots(&content) {
            let restored = restore_preserved(slot.content, preserved);
            if let Some(next) = replace_slot(&content, &slot.path, restored) {
                content = next;
            }
        }
    }

    content
}

/// Strip `PageElem` styles from a style map, keeping only inline/block styles.
///
/// Page styles must not be re-applied at the block level; they are handled
/// separately per style group in `build_annotated_content`.
fn non_page_styles(styles: Styles) -> Styles {
    styles
        .iter()
        .filter(|style| {
            style
                .element()
                .is_none_or(|element| element != PageElem::ELEM)
        })
        .cloned()
        .map(Style::wrap)
        .collect()
}

fn page_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| {
            style
                .element()
                .is_some_and(|element| element == PageElem::ELEM)
        })
        .cloned()
        .map(Style::wrap)
        .collect()
}

fn marginal_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| {
            style
                .element()
                .is_some_and(|element| element == PageElem::ELEM)
                || (style.outside() && style.liftable())
        })
        .cloned()
        .map(Style::wrap)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SystemWorld;
    use std::fs;
    use tempfile::TempDir;
    use typst::foundations::Packed;
    use typst::layout::{BlockBody, BlockElem};
    use typst::model::{FootnoteBody, ListElem, ListItem};
    use typst::text::TextElem;
    use typst::visualize::Color;

    #[test]
    fn eval_to_realized_content_returns_annotated_content_with_realized_field() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "Hello *world*.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let annotated = eval_to_realized_content(&world).unwrap();
        assert!(annotated.realized.plain_text().contains("Hello"));
        assert!(annotated.realized.plain_text().contains("world"));
    }

    #[test]
    fn eval_extracts_text_nodes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "Hello *world*.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = eval_to_content(&world).unwrap();

        let mut texts = Vec::new();
        let _ = content.traverse::<_, ()>(&mut |c| {
            if let Some(t) = c.to_packed::<TextElem>() {
                texts.push(t.text.to_string());
            }
            std::ops::ControlFlow::Continue(())
        });
        assert!(texts.contains(&"Hello".to_string()));
        assert!(texts.contains(&"world".to_string()));
    }

    #[test]
    fn eval_inlines_includes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "#include \"ch.typ\"").unwrap();
        fs::write(dir.path().join("ch.typ"), "Included text.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = eval_to_content(&world).unwrap();
        let plain = content.plain_text();
        assert!(plain.contains("Included text."));
    }

    fn text(s: &str) -> Content {
        TextElem::packed(s)
    }

    fn block(body: &str) -> Content {
        Content::new(BlockElem::new().with_body(Some(BlockBody::Content(TextElem::packed(body)))))
            .spanned(Span::detached())
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

    #[test]
    fn collect_preserved_by_span_keeps_multiple_values_for_same_span() {
        let first = block("First");
        let second = block("Second");
        let content = Content::sequence([first.clone(), second.clone()]);

        let preserved = collect_preserved_by_span(&content);
        let queue = preserved.get(&Span::detached()).unwrap();

        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].plain_text(), "First");
        assert_eq!(queue[1].plain_text(), "Second");
    }

    #[test]
    fn restore_preserved_consumes_same_span_values_in_order() {
        let preserved_content = Content::sequence([block("First"), block("Second")]);
        let mut preserved = collect_preserved_by_span(&preserved_content);

        let realized_a = text("realized a").spanned(Span::detached());
        let realized_b = text("realized b").spanned(Span::detached());

        assert_eq!(
            restore_preserved(realized_a, &mut preserved).plain_text(),
            "First"
        );
        assert_eq!(
            restore_preserved(realized_b, &mut preserved).plain_text(),
            "Second"
        );
    }

    #[test]
    fn restore_preserved_recurses_into_slot_container_children() {
        let preserved_child = Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
            "Preserved item",
        )))]))
        .spanned(Span::detached());
        let mut preserved = collect_preserved_by_span(&preserved_child);

        let realized = Content::new(ListElem::new(vec![Packed::new(ListItem::new(text(
            "Realized item",
        )))]));
        let restored = restore_preserved(realized, &mut preserved);

        assert_eq!(restored.plain_text(), "Preserved item");
    }

    #[test]
    fn restore_preserved_leaves_unknown_content_unchanged() {
        let mut preserved = HashMap::new();
        let restored = restore_preserved(text("Plain"), &mut preserved);
        assert_eq!(restored.plain_text(), "Plain");
    }

    #[test]
    fn restore_footnote_markers_replaces_markers_in_document_order() {
        let first = Content::new(FootnoteElem::new(FootnoteBody::Content(text("First note"))));
        let second = Content::new(FootnoteElem::new(FootnoteBody::Content(text(
            "Second note",
        ))));
        let content = Content::sequence([text("Text"), text("1"), text(" more"), text("2")]);

        let restored = restore_footnote_markers(content, &[first, second]);

        assert!(restored.plain_text().contains("First note"));
        assert!(restored.plain_text().contains("Second note"));
        assert_eq!(count_elem::<FootnoteElem>(&restored), 2);
    }

    #[test]
    fn restore_footnote_markers_handles_styled_marker() {
        let footnote = Content::new(FootnoteElem::new(FootnoteBody::Content(text(
            "Styled note",
        ))));
        let marker = text("1").styled(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));

        let restored = restore_footnote_markers(marker, &[footnote]);

        assert_eq!(restored.plain_text(), "Styled note");
        assert_eq!(count_elem::<FootnoteElem>(&restored), 1);
    }

    #[test]
    fn restore_footnote_markers_does_not_replace_non_matching_numbers() {
        let footnote = Content::new(FootnoteElem::new(FootnoteBody::Content(text("Note"))));
        let content = Content::sequence([text("2")]);

        let restored = restore_footnote_markers(content, &[footnote]);

        assert_eq!(restored.plain_text(), "2");
        assert_eq!(count_elem::<FootnoteElem>(&restored), 0);
    }

    #[test]
    fn style_partitioning_separates_page_and_non_page_styles() {
        let mut styles = Styles::new();
        styles.push(PageElem::flipped.set(true));
        styles.push(TextElem::fill.set(Color::from_u8(1, 2, 3, 255).into()));

        let pages = page_styles(&styles);
        let non_pages = non_page_styles(styles.clone());
        let marginal = marginal_styles(&styles);

        assert!(!pages.is_empty());
        assert!(pages.iter().all(|style| {
            style
                .element()
                .is_some_and(|element| element == PageElem::ELEM)
        }));
        assert!(non_pages.iter().all(|style| {
            style
                .element()
                .is_none_or(|element| element != PageElem::ELEM)
        }));
        assert!(marginal.iter().any(|style| {
            style
                .element()
                .is_some_and(|element| element == PageElem::ELEM)
        }));
    }
}
