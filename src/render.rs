//! Render annotated [`Content`] to PDF bytes.

use anyhow::Result;
use typst::ROUTINES;
use typst::World;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{Content, StyleChain, Target, TargetElem};
use typst::introspection::Introspector;
use typst_pdf::{PdfOptions, pdf};

use crate::build_info;
use crate::diag::format_diagnostics;

/// Layout `content` and export it as PDF bytes.
///
/// Uses the same convergence loop as [`crate::eval::layout_introspector`]: up to 5
/// layout passes until the `Introspector` stabilises (needed for cross-references,
/// footnote numbers, etc. in the annotated document). Tagged PDF is disabled because
/// the annotation markup doesn't carry accessible metadata.
pub fn render_to_pdf(content: &Content, world: &dyn World) -> Result<Vec<u8>> {
    let library = world.library();
    let base = StyleChain::new(&library.styles);
    let target = TargetElem::target.set(Target::Paged).wrap();
    let styles = base.chain(&target);

    let traced = Traced::default();

    let mut introspector = Introspector::default();
    let mut final_sink = Sink::new();
    let mut document = None;

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

        document = Some(laid_out);
        final_sink = sink;
        introspector = next_introspector;

        if converged {
            break;
        }
    }

    let document = document.expect("layout loop always runs at least once");

    // Surface any delayed errors collected during layout.
    let delayed = final_sink.delayed();
    if !delayed.is_empty() {
        return Err(anyhow::anyhow!(
            "layout errors:\n{}",
            format_diagnostics(world, &delayed)
        ));
    }

    let pdf_options = PdfOptions {
        timestamp: build_info::pdf_timestamp(),
        tagged: false,
        ..PdfOptions::default()
    };

    let mut bytes = pdf(&document, &pdf_options).map_err(|errs| {
        let msgs: Vec<String> = errs.iter().map(|d| d.message.to_string()).collect();
        anyhow::anyhow!("pdf export failed:\n{}", msgs.join("\n"))
    })?;
    embed_build_comment(&mut bytes);
    Ok(bytes)
}

fn embed_build_comment(pdf: &mut Vec<u8>) {
    let comment = format!(
        "\n% {}\n% typst-diff-build-unix: {}\n",
        build_info::build_report_line(),
        build_info::BUILD_UNIX
    );
    if let Some(pos) = find_last_subslice(pdf, b"%%EOF") {
        pdf.splice(pos..pos, comment.bytes());
    } else {
        pdf.extend_from_slice(comment.as_bytes());
    }
}

fn find_last_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SystemWorld;
    use std::fs;
    use tempfile::TempDir;
    use typst::foundations::Content;
    use typst::layout::PageElem;
    use typst::text::TextElem;

    #[test]
    fn renders_simple_content_to_pdf() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = TextElem::packed("Hello, diff world.");
        let pdf = render_to_pdf(&content, &world).unwrap();
        assert!(pdf.starts_with(b"%PDF"), "expected PDF output");
    }

    #[test]
    fn embeds_typst_diff_build_identity_in_pdf_bytes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = TextElem::packed("Hello, diff world.");

        let pdf = render_to_pdf(&content, &world).unwrap();
        let pdf_text = String::from_utf8_lossy(&pdf);

        assert!(pdf_text.contains(&crate::build_info::build_report_line()));
        assert!(pdf_text.contains(crate::build_info::BUILD_UNIX));
    }

    #[test]
    fn renders_sequence_to_pdf() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = Content::sequence([
            TextElem::packed("First paragraph."),
            TextElem::packed(" Second paragraph."),
        ]);

        let pdf = render_to_pdf(&content, &world).unwrap();

        assert!(pdf.starts_with(b"%PDF"), "expected PDF output");
        assert!(pdf.len() > 1000);
    }

    #[test]
    fn renders_page_styled_content_to_pdf() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = TextElem::packed("Landscape text").styled(PageElem::flipped.set(true));

        let pdf = render_to_pdf(&content, &world).unwrap();

        assert!(pdf.starts_with(b"%PDF"), "expected PDF output");
        assert!(pdf.len() > 1000);
    }

    #[test]
    fn render_reports_layout_error_for_missing_reference() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "See @missing.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = crate::eval_to_content(&world).unwrap();

        let err = render_to_pdf(&content, &world).unwrap_err();

        assert!(err.to_string().contains("layout errors"), "{err}");
    }
}
