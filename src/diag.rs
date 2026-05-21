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
