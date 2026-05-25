use typst_diff::{annotate, diff, eval_to_realized_content, render_to_pdf, world};

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Parser;
use tempfile::TempDir;

#[derive(Parser)]
#[command(
    name = "typst-diff",
    about = "Diff two Typst documents and produce a PDF"
)]
struct Args {
    /// Path to the old document entry point, or the working-tree file when --revision is used
    old_or_file: PathBuf,
    /// Path to the new document entry point
    new: Option<PathBuf>,
    /// Compare the working-tree file against this Git revision
    #[arg(short = 'r', long)]
    revision: Option<String>,
    /// Output PDF path
    #[arg(short, long, default_value = "diff.pdf")]
    output: PathBuf,
    /// Write a plain-text log of detected insertions, deletions, and modifications
    #[arg(short = 'l', long)]
    log_modifications: Option<PathBuf>,
    /// Show substitutions as blue without red strikethrough; insertions green, deletions red
    #[arg(short = 's', long)]
    compact_substitutions: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let inputs = resolve_inputs(&args)?;

    eprintln!("Loading old document: {}", inputs.old.display());
    let old_world = world::SystemWorld::new(&inputs.old)
        .with_context(|| format!("failed to load old document {:?}", inputs.old))?;

    eprintln!("Loading new document: {}", inputs.new.display());
    let new_world = world::SystemWorld::new(&inputs.new)
        .with_context(|| format!("failed to load new document {:?}", inputs.new))?;

    eprintln!("Evaluating old document...");
    let old_content =
        eval_to_realized_content(&old_world).context("failed to evaluate old document")?;

    eprintln!("Evaluating new document...");
    let new_content =
        eval_to_realized_content(&new_world).context("failed to evaluate new document")?;

    eprintln!("Diffing...");
    let diff_result = diff::diff_annotated(&old_content, &new_content);
    if let Some(path) = &args.log_modifications {
        std::fs::write(path, diff_result.modification_log())
            .with_context(|| format!("failed to write modification log {:?}", path))?;
        eprintln!("Wrote modification log to {}", path.display());
    }

    eprintln!("Annotating...");
    let annotated =
        annotate::build_annotated_content_from_tree(&diff_result, args.compact_substitutions);

    eprintln!("Rendering to PDF...");
    let pdf_bytes = render_to_pdf(&annotated, &new_world).context("failed to render PDF")?;

    std::fs::write(&args.output, &pdf_bytes)
        .with_context(|| format!("failed to write {:?}", args.output))?;

    eprintln!("Written to {}", args.output.display());
    Ok(())
}

/// Absolute paths to the two documents to diff, plus an optional temp directory that
/// must stay alive until rendering is done (the `--revision` Git snapshot lives there).
struct ResolvedInputs {
    old: PathBuf,
    new: PathBuf,
    _snapshot: Option<TempDir>,
}

/// Validate the argument combination and produce concrete file paths.
///
/// `--revision` and an explicit `new` path are mutually exclusive; one of them must
/// be present.
fn resolve_inputs(args: &Args) -> Result<ResolvedInputs> {
    match (&args.revision, &args.new) {
        (Some(revision), None) => resolve_git_inputs(&args.old_or_file, revision),
        (Some(_), Some(_)) => {
            bail!(
                "--revision expects exactly one file argument, e.g. typst-diff main.typ --revision HEAD"
            )
        }
        (None, Some(new)) => Ok(ResolvedInputs {
            old: args.old_or_file.clone(),
            new: new.clone(),
            _snapshot: None,
        }),
        (None, None) => {
            bail!("missing new document path, or pass --revision to compare against Git")
        }
    }
}

/// Snapshot the entire Git repository at `revision` into a temp directory and return
/// paths to the old (snapshotted) and new (working-tree) copies of `file`.
///
/// The full repo snapshot (via `git archive | tar`) is needed rather than a single-file
/// checkout so that `#include` directives in the Typst source resolve correctly relative
/// to the snapshot root.
fn resolve_git_inputs(file: &PathBuf, revision: &str) -> Result<ResolvedInputs> {
    let working_file = file
        .canonicalize()
        .with_context(|| format!("cannot find working-tree file {:?}", file))?;
    let file_dir = working_file
        .parent()
        .context("working-tree file has no parent directory")?;
    let git_root = git_root(file_dir)?;
    let relative_file = working_file.strip_prefix(&git_root).with_context(|| {
        format!(
            "file {:?} is not inside Git root {:?}",
            working_file, git_root
        )
    })?;

    eprintln!(
        "Creating snapshot of Git revision {revision} for {}...",
        relative_file.display()
    );
    let snapshot = archive_revision(&git_root, revision)?;
    let old_file = snapshot.path().join(relative_file);

    if !old_file.exists() {
        bail!(
            "{} does not exist at Git revision {revision}",
            relative_file.display()
        );
    }

    Ok(ResolvedInputs {
        old: old_file,
        new: working_file,
        _snapshot: Some(snapshot),
    })
}

/// Return the absolute path to the Git repository root containing `cwd`.
fn git_root(cwd: &std::path::Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to run git rev-parse --show-toplevel")?;
    if !output.status.success() {
        bail!(
            "not in a Git working tree:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let root = String::from_utf8(output.stdout).context("Git root path is not valid UTF-8")?;
    Ok(PathBuf::from(root.trim()).canonicalize()?)
}

/// Run `git archive | tar -x` to unpack `revision` into a fresh temp directory.
fn archive_revision(git_root: &PathBuf, revision: &str) -> Result<TempDir> {
    let archive = Command::new("git")
        .args(["-C"])
        .arg(git_root)
        .args(["archive", "--format=tar", revision])
        .output()
        .with_context(|| format!("failed to run git archive for revision {revision}"))?;
    if !archive.status.success() {
        bail!(
            "failed to archive Git revision {revision}:\n{}",
            String::from_utf8_lossy(&archive.stderr).trim()
        );
    }

    let snapshot = TempDir::new().context("failed to create temporary Git snapshot directory")?;
    let mut tar = Command::new("tar")
        .args(["-x", "-C"])
        .arg(snapshot.path())
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run tar to unpack Git snapshot")?;
    tar.stdin
        .as_mut()
        .context("failed to open tar stdin")?
        .write_all(&archive.stdout)
        .context("failed to send Git archive to tar")?;
    let status = tar.wait().context("failed to wait for tar")?;
    if !status.success() {
        bail!("failed to unpack Git snapshot");
    }

    Ok(snapshot)
}
