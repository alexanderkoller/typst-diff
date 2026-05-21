use anyhow::Result;
use typst::ROUTINES;
use typst::World;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{Content, NativeElement, Style, StyleChain, Styles, Target, TargetElem};
use typst::introspection::{Introspector, Locator};
use typst::layout::{PageElem, PagebreakElem};
use typst::model::DocumentInfo;
use typst::routines::{Arenas, RealizationKind};

use crate::diag::format_diagnostics;

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

pub fn eval_to_realized_content(world: &dyn World) -> Result<Content> {
    let content = eval_to_content(world)?;
    let introspector = layout_introspector(world, &content)?;
    realize_to_content(world, &content, introspector)
}

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

    Ok(Content::sequence(realized.iter().map(|(content, styles)| {
        let styles = if content.is::<PagebreakElem>() {
            marginal_styles(&styles.to_map())
        } else {
            non_page_styles(styles.to_map())
        };
        (*content).clone().styled_with_map(styles)
    }))
    .styled_with_map(root_page_styles))
}

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
}
