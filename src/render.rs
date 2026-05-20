use anyhow::Result;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{Content, StyleChain};
use typst::introspection::Introspector;
use typst::World;
use typst::ROUTINES;
use typst_pdf::{PdfOptions, pdf};

use crate::diag::format_diagnostics;

pub fn render_to_pdf(content: &Content, world: &dyn World) -> Result<Vec<u8>> {
    let library = world.library();
    let styles = StyleChain::new(&library.styles);

    let introspector = Introspector::default();
    let constraint = typst::comemo::Constraint::new();
    let mut sink = Sink::new();
    let traced = Traced::default();

    let mut engine = Engine {
        routines: &ROUTINES,
        world: world.track(),
        introspector: introspector.track_with(&constraint),
        traced: traced.track(),
        sink: sink.track_mut(),
        route: Route::default(),
    };

    // One layout iteration is intentional: the diff document has no counters or
    // cross-references requiring convergence. See design spec section "Rendering".
    let document = typst_layout::layout_document(&mut engine, content, styles)
        .map_err(|errs| anyhow::anyhow!("layout failed:\n{}", format_diagnostics(world, &errs)))?;
    drop(engine);

    // Surface any delayed errors collected during layout.
    let delayed = sink.delayed();
    if !delayed.is_empty() {
        return Err(anyhow::anyhow!("layout errors:\n{}", format_diagnostics(world, &delayed)));
    }

    pdf(&document, &PdfOptions::default()).map_err(|errs| {
        let msgs: Vec<String> = errs.iter().map(|d| d.message.to_string()).collect();
        anyhow::anyhow!("pdf export failed:\n{}", msgs.join("\n"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use typst::text::TextElem;
    use crate::world::SystemWorld;

    #[test]
    fn renders_simple_content_to_pdf() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = TextElem::packed("Hello, diff world.");
        let pdf = render_to_pdf(&content, &world).unwrap();
        assert!(pdf.starts_with(b"%PDF"), "expected PDF output");
    }
}
