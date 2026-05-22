use typst::World;
use typst::diag::SourceDiagnostic;

/// Format a slice of diagnostics as "path:line:col: message" strings.
/// Falls back to plain message text if span information is unavailable.
pub fn format_diagnostics(world: &dyn World, diags: &[SourceDiagnostic]) -> String {
    diags
        .iter()
        .map(|d| format_one(world, d))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_one(world: &dyn World, d: &SourceDiagnostic) -> String {
    let location = d.span.id().and_then(|id| {
        let source = world.source(id).ok()?;
        let range = source.range(d.span)?;
        let line = source.lines().byte_to_line(range.start)?;
        let col = source.lines().byte_to_column(range.start)?;
        let path = id.vpath().as_rootless_path().display().to_string();
        Some(format!("{}:{}:{}", path, line + 1, col + 1))
    });

    match location {
        Some(loc) => format!("{}: {}", loc, d.message),
        None => d.message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SystemWorld;
    use std::fs;
    use tempfile::TempDir;
    use typst::syntax::Span;

    #[test]
    fn format_diagnostics_empty_returns_empty_string() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();

        assert_eq!(format_diagnostics(&world, &[]), "");
    }

    #[test]
    fn format_diagnostics_includes_virtual_path_line_col() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "first\nsecond\n").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let source = world.source(world.main()).unwrap();
        let span = source.root().span();
        let diagnostic = SourceDiagnostic::error(span, "problem");

        let formatted = format_diagnostics(&world, &[diagnostic]);

        assert!(formatted.contains("main.typ:1:1: problem"), "{formatted}");
    }

    #[test]
    fn format_diagnostics_falls_back_for_detached_span() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let diagnostic = SourceDiagnostic::error(Span::detached(), "detached problem");

        assert_eq!(
            format_diagnostics(&world, &[diagnostic]),
            "detached problem"
        );
    }

    #[test]
    fn format_diagnostics_formats_multiple_diagnostics() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "content").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let diagnostics = [
            SourceDiagnostic::error(Span::detached(), "first"),
            SourceDiagnostic::warning(Span::detached(), "second"),
        ];

        assert_eq!(format_diagnostics(&world, &diagnostics), "first\nsecond");
    }
}
