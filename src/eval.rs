//! Typst document evaluation: from source text to a fully-realized [`Content`] tree.
//!
//! Two public entry points are exposed:
//! - [`eval_to_content`] — shallow eval only (Typst AST → unevaluated `Content`).
//! - [`eval_to_realized_content`] — full pipeline used by the diff: eval → layout →
//!   realization, producing a stable content tree where show rules and counters have
//!   been expanded.
//!
//! # Why realization is needed
//!
//! The diff operates on the *semantic* content tree, not the raw layout output.
//! Typst's realization step expands show rules (e.g. `show heading: …`) so that the
//! resulting tree reflects the document's logical structure. Without it, two headings
//! with identical text but different show rules would look identical in the diff.

use anyhow::Result;
use typst::ROUTINES;
use typst::World;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{
    Content, NativeElement, Style, StyleChain, Styles, Target, TargetElem,
};
use typst::introspection::{Introspector, Locator};
use typst::layout::{PageElem, PagebreakElem};
use typst::model::{DocumentInfo, FootnoteElem};
use typst::routines::{Arenas, RealizationKind};

use crate::content_slots::normalize_list_item_runs;
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
        let styles = if realized_content.is::<PagebreakElem>() {
            marginal_styles(&styles.to_map())
        } else {
            non_page_styles(styles.to_map())
        };
        (*realized_content).clone().styled_with_map(styles)
    }))
    .styled_with_map(root_page_styles);

    Ok(realized)
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
    fn annotate_realized_handles_repeated_function_expansions_with_distinct_content() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        fs::write(dir_a.path().join("main.typ"),
            "#let f(body) = [#body]\n#f[a]\n#f[b]").unwrap();
        fs::write(dir_b.path().join("main.typ"),
            "#let f(body) = [#body]\n#f[x]\n#f[b]").unwrap();
        let world_old = SystemWorld::new(dir_a.path().join("main.typ")).unwrap();
        let world_new = SystemWorld::new(dir_b.path().join("main.typ")).unwrap();

        let _old = eval_to_realized_content(&world_old).unwrap();
        let new = eval_to_realized_content(&world_new).unwrap();
        assert!(new.realized.plain_text().contains('x'), "{}", new.realized.plain_text());
        assert!(new.realized.plain_text().contains('b'), "{}", new.realized.plain_text());
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
