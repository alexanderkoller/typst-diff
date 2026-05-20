use typst_diff::{build_annotated_content, diff, eval_to_content, render_to_pdf, world};

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(name = "typst-diff", about = "Diff two Typst documents and produce a PDF")]
struct Args {
    /// Path to the old document entry point
    old: PathBuf,
    /// Path to the new document entry point
    new: PathBuf,
    /// Output PDF path
    #[arg(short, long, default_value = "diff.pdf")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    eprintln!("Loading old document: {}", args.old.display());
    let old_world = world::SystemWorld::new(&args.old)
        .with_context(|| format!("failed to load old document {:?}", args.old))?;

    eprintln!("Loading new document: {}", args.new.display());
    let new_world = world::SystemWorld::new(&args.new)
        .with_context(|| format!("failed to load new document {:?}", args.new))?;

    eprintln!("Evaluating old document...");
    let old_content = eval_to_content(&old_world).context("failed to evaluate old document")?;

    eprintln!("Evaluating new document...");
    let new_content = eval_to_content(&new_world).context("failed to evaluate new document")?;

    eprintln!("Diffing...");
    let diff_result = diff::diff_content(&old_content, &new_content);

    eprintln!("Annotating...");
    let annotated = build_annotated_content(&diff_result);

    eprintln!("Rendering to PDF...");
    let pdf_bytes = render_to_pdf(&annotated, &new_world).context("failed to render PDF")?;

    std::fs::write(&args.output, &pdf_bytes)
        .with_context(|| format!("failed to write {:?}", args.output))?;

    eprintln!("Written to {}", args.output.display());
    Ok(())
}
