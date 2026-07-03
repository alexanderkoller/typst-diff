use typst_diff::{annotate, build_info, debug, decision, diff, eval, render_to_pdf, trace, world};

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Parser;
use tempfile::TempDir;
use typst::text::TextElem;

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
    /// Suppress fallback warnings on stderr; progress and hard errors are unchanged
    #[arg(long)]
    quiet: bool,
    /// Write structured YAML diagnostics next to the output PDF
    #[arg(long)]
    debug: bool,
    /// Write expensive JSONL pipeline traces next to the output PDF; implies --debug
    #[arg(long)]
    debug_trace: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let inputs = resolve_inputs(&args)?;

    eprintln!("typst-diff build: {}", build_info::build_report_line());
    eprintln!("Loading old document: {}", inputs.old.display());
    let old_world = world::SystemWorld::new(&inputs.old)
        .with_context(|| format!("failed to load old document {:?}", inputs.old))?;

    eprintln!("Loading new document: {}", inputs.new.display());
    let new_world = world::SystemWorld::new(&inputs.new)
        .with_context(|| format!("failed to load new document {:?}", inputs.new))?;

    run_pipeline(&args, &inputs, &old_world, &new_world)
}

fn run_pipeline(
    args: &Args,
    inputs: &ResolvedInputs,
    old_world: &world::SystemWorld,
    new_world: &world::SystemWorld,
) -> Result<()> {
    let capture_snapshots = args.debug || args.debug_trace;
    let debug_dir = debug::default_debug_dir(&args.output);
    let mut decision_recorder = decision::DecisionRecorder::default();
    let mut trace_writer = if args.debug_trace {
        Some(debug::JsonlTraceWriter::create(&debug_dir)?)
    } else {
        None
    };

    if let Some(patch) = showybox_source_patch(inputs, args.compact_substitutions)? {
        eprintln!("Applying source-level showybox body diff...");
        write_source_patch_log_and_pdf(args, &patch)?;
        return Ok(());
    }

    eprintln!("Evaluating old document...");
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("eval/old", "start"),
    )?;
    let old_eval = if capture_snapshots {
        Some(eval::eval_with_debug(old_world).context("failed to evaluate old document")?)
    } else {
        None
    };
    let old_content = if let Some(eval) = &old_eval {
        eval.annotated.clone()
    } else {
        eval::eval_to_realized_content(old_world).context("failed to evaluate old document")?
    };
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("eval/old", "end").snapshot_ref(if capture_snapshots {
            "old/realized-tree.yml"
        } else {
            ""
        }),
    )?;

    eprintln!("Evaluating new document...");
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("eval/new", "start"),
    )?;
    let new_eval = if capture_snapshots {
        Some(eval::eval_with_debug(new_world).context("failed to evaluate new document")?)
    } else {
        None
    };
    let new_content = if let Some(eval) = &new_eval {
        eval.annotated.clone()
    } else {
        eval::eval_to_realized_content(new_world).context("failed to evaluate new document")?
    };
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("eval/new", "end").snapshot_ref(if capture_snapshots {
            "new/realized-tree.yml"
        } else {
            ""
        }),
    )?;

    eprintln!("Diffing...");
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("diff", "start"),
    )?;
    let (diff_result, block_debug) = if capture_snapshots {
        let debug_events = trace_writer
            .as_mut()
            .map(|writer| writer as &mut dyn trace::DebugEventSink);
        let (result, debug) = diff::diff_annotated_with_rendered_regions_and_decisions(
            &old_content,
            &new_content,
            old_world,
            new_world,
            debug_events,
            Some(&mut decision_recorder),
            true,
        )?;
        (result, Some(debug.expect("snapshots requested")))
    } else {
        let (result, _) = diff::diff_annotated_with_rendered_regions_and_decisions(
            &old_content,
            &new_content,
            old_world,
            new_world,
            None,
            Some(&mut decision_recorder),
            false,
        )?;
        (result, None)
    };
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("diff", "end")
            .reason(format!("blocks={}", diff_result.blocks.len()))
            .snapshot_ref(if capture_snapshots {
                "diff/final-edits.yml"
            } else {
                ""
            }),
    )?;

    eprintln!("Annotating...");
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("annotate", "start"),
    )?;
    let annotated = if let Some(writer) = trace_writer.as_mut() {
        annotate::build_annotated_content_from_tree_with_debug_events(
            &diff_result,
            args.compact_substitutions,
            writer,
        )?
    } else {
        annotate::build_annotated_content_from_tree(&diff_result, args.compact_substitutions)
    };
    emit_cli_trace(
        &mut trace_writer,
        trace::PipelineTraceEvent::new("annotate", "end").snapshot_ref(if capture_snapshots {
            "output/annotated-content.yml"
        } else {
            ""
        }),
    )?;

    if capture_snapshots && trace_writer.is_none() {
        write_debug_bundle(
            args,
            inputs,
            &debug_dir,
            old_eval.as_ref().expect("snapshots requested"),
            new_eval.as_ref().expect("snapshots requested"),
            block_debug.as_ref().expect("snapshots requested"),
            &diff_result,
            &annotated,
            &decision_recorder.fallback_warnings_document(),
            Vec::new(),
        )?;
    }

    write_log_and_pdf(args, new_world, &diff_result, &annotated, &mut trace_writer)?;

    let trace_files = match trace_writer {
        Some(writer) => writer.finish()?,
        None => Vec::new(),
    };

    if capture_snapshots && args.debug_trace {
        write_debug_bundle(
            args,
            inputs,
            &debug_dir,
            old_eval.as_ref().expect("snapshots requested"),
            new_eval.as_ref().expect("snapshots requested"),
            block_debug.as_ref().expect("snapshots requested"),
            &diff_result,
            &annotated,
            &decision_recorder.fallback_warnings_document(),
            trace_files,
        )?;
    }

    decision_recorder.emit_stderr_warnings(args.quiet, std::io::stderr())?;

    Ok(())
}

fn write_debug_bundle(
    args: &Args,
    inputs: &ResolvedInputs,
    debug_dir: &std::path::Path,
    old_eval: &eval::EvalDebug,
    new_eval: &eval::EvalDebug,
    block_debug: &diff::DiffBlockDebug,
    diff_result: &diff::DiffResult,
    annotated: &typst::foundations::Content,
    fallback_warnings: &decision::FallbackWarningsDocument,
    trace_files: Vec<debug::DebugTraceFile>,
) -> Result<()> {
    eprintln!("Writing debug bundle to {}...", debug_dir.display());
    let build_line = build_info::build_report_line();
    debug::write_debug_bundle(&debug::DebugBundle {
        build_line: &build_line,
        args: debug_args(args),
        old_input: &inputs.old,
        new_input: &inputs.new,
        output: &args.output,
        debug_dir,
        old_eval,
        new_eval,
        block_debug,
        diff_result,
        annotated_output: annotated,
        fallback_warnings,
        trace_files,
    })
}

fn write_log_and_pdf(
    args: &Args,
    new_world: &world::SystemWorld,
    diff_result: &diff::DiffResult,
    annotated: &typst::foundations::Content,
    trace_writer: &mut Option<debug::JsonlTraceWriter>,
) -> Result<()> {
    if let Some(path) = &args.log_modifications {
        std::fs::write(path, diff_result.modification_log())
            .with_context(|| format!("failed to write modification log {:?}", path))?;
        eprintln!("Wrote modification log to {}", path.display());
    }

    eprintln!("Rendering to PDF...");
    emit_cli_trace(
        trace_writer,
        trace::PipelineTraceEvent::new("render", "start"),
    )?;
    let pdf_bytes = render_to_pdf(annotated, new_world).context("failed to render PDF")?;
    emit_cli_trace(
        trace_writer,
        trace::PipelineTraceEvent::new("render", "end")
            .reason(format!("pdf_bytes={}", pdf_bytes.len())),
    )?;

    std::fs::write(&args.output, &pdf_bytes)
        .with_context(|| format!("failed to write {:?}", args.output))?;

    eprintln!("Written to {}", args.output.display());
    Ok(())
}

struct SourcePatch {
    source: String,
    log: String,
}

fn showybox_source_patch(inputs: &ResolvedInputs, compact: bool) -> Result<Option<SourcePatch>> {
    let old_source = std::fs::read_to_string(&inputs.old)
        .with_context(|| format!("failed to read old source {:?}", inputs.old))?;
    let new_source = std::fs::read_to_string(&inputs.new)
        .with_context(|| format!("failed to read new source {:?}", inputs.new))?;

    if !old_source.contains("showybox") || !new_source.contains("showybox") {
        return Ok(None);
    }

    let old_calls = showybox_calls(&old_source);
    let new_calls = showybox_calls(&new_source);
    if old_calls.is_empty() || old_calls.len() != new_calls.len() {
        return Ok(None);
    }

    let mut replacements = Vec::new();
    let mut log = format!(
        "generated_by: {}\n\n",
        typst_diff::build_info::build_report_line()
    );
    for (index, (old_call, new_call)) in old_calls.iter().zip(&new_calls).enumerate() {
        let old_body = &old_source[old_call.body_start..old_call.body_end];
        let new_body = &new_source[new_call.body_start..new_call.body_end];
        if normalize_words(old_body) == normalize_words(new_body) {
            continue;
        }
        let markup = showybox_body_markup(old_body, new_body, compact);
        replacements.push((new_call.body_start, new_call.body_end, markup));
        log.push_str(&format!("## {}: modify\nblock: showybox\n", index + 1));
        log.push_str(&format!("deleted: {}\n", normalize_words(old_body)));
        log.push_str(&format!("inserted: {}\n\n", normalize_words(new_body)));
    }

    if replacements.is_empty() {
        return Ok(None);
    }

    let mut patched = new_source;
    for (start, end, replacement) in replacements.into_iter().rev() {
        patched.replace_range(start..end, &replacement);
    }

    Ok(Some(SourcePatch {
        source: patched,
        log,
    }))
}

fn write_source_patch_log_and_pdf(args: &Args, patch: &SourcePatch) -> Result<()> {
    if let Some(path) = &args.log_modifications {
        std::fs::write(path, &patch.log)
            .with_context(|| format!("failed to write modification log {:?}", path))?;
        eprintln!("Wrote modification log to {}", path.display());
    }

    let dir = TempDir::new().context("failed to create temporary patched source directory")?;
    let source_path = dir.path().join("patched.typ");
    std::fs::write(&source_path, &patch.source)
        .with_context(|| format!("failed to write {:?}", source_path))?;
    let patched_world = world::SystemWorld::new(&source_path)
        .with_context(|| format!("failed to load patched source {:?}", source_path))?;
    let content =
        eval::eval_to_content(&patched_world).context("failed to evaluate patched source")?;

    eprintln!("Rendering to PDF...");
    let pdf_bytes = render_to_pdf(&content, &patched_world).context("failed to render PDF")?;
    std::fs::write(&args.output, &pdf_bytes)
        .with_context(|| format!("failed to write {:?}", args.output))?;
    eprintln!("Written to {}", args.output.display());
    Ok(())
}

#[derive(Clone, Copy)]
struct ShowyboxCall {
    body_start: usize,
    body_end: usize,
}

fn showybox_calls(source: &str) -> Vec<ShowyboxCall> {
    let mut calls = Vec::new();
    let mut offset = 0;
    while let Some(relative) = source[offset..].find("#showybox(") {
        let start = offset + relative;
        let Some(args_end) = matching_delimiter(source, start + "#showybox".len(), '(', ')') else {
            offset = start + "#showybox(".len();
            continue;
        };
        let mut body_open = args_end + 1;
        while source
            .as_bytes()
            .get(body_open)
            .is_some_and(u8::is_ascii_whitespace)
        {
            body_open += 1;
        }
        if source.as_bytes().get(body_open) != Some(&b'[') {
            offset = args_end + 1;
            continue;
        }
        let Some(body_close) = matching_delimiter(source, body_open, '[', ']') else {
            offset = body_open + 1;
            continue;
        };
        calls.push(ShowyboxCall {
            body_start: body_open + 1,
            body_end: body_close,
        });
        offset = body_close + 1;
    }
    calls
}

fn matching_delimiter(source: &str, open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (relative, ch) in source[open..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == left {
            depth += 1;
        } else if ch == right {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(open + relative);
            }
        }
    }
    None
}

fn showybox_body_markup(old_body: &str, new_body: &str, compact: bool) -> String {
    let old_tokens = source_tokens(&normalize_words(old_body));
    let new_tokens = source_tokens(&normalize_words(new_body));
    let word_ops = diff::diff_words(&old_tokens, &new_tokens);
    let mut chunks = Vec::new();
    for op in word_ops {
        match op {
            diff::WordOp::Equal(tokens) => chunks.push(tokens_typst_text(&tokens)),
            diff::WordOp::Delete(tokens) => {
                if !compact {
                    chunks.push(format!(
                        "#strike[#text(fill: rgb(\"#dc0000\"))[{}]]",
                        tokens_typst_text(&tokens)
                    ));
                }
            }
            diff::WordOp::Insert(tokens) => chunks.push(format!(
                "#text(fill: rgb(\"#00b400\"))[{}]",
                tokens_typst_text(&tokens)
            )),
        }
    }
    format!("\n  {}\n", chunks.join(" "))
}

fn word_tokens(source: &str) -> Vec<String> {
    source
        .split_whitespace()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

fn normalize_words(source: &str) -> String {
    word_tokens(source).join(" ")
}

fn source_tokens(source: &str) -> Vec<diff::Token> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut kind = source.chars().next().map(source_token_kind);
    for (index, ch) in source.char_indices() {
        let next_kind = source_token_kind(ch);
        if Some(next_kind) != kind {
            push_source_token(&mut tokens, &source[start..index]);
            start = index;
            kind = Some(next_kind);
        }
    }
    push_source_token(&mut tokens, &source[start..]);
    tokens
}

fn push_source_token(tokens: &mut Vec<diff::Token>, text: &str) {
    if !text.is_empty() {
        tokens.push(diff::Token {
            text: text.to_string(),
            content: TextElem::packed(text),
        });
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceTokenKind {
    Space,
    Punctuation,
    Word,
}

fn source_token_kind(ch: char) -> SourceTokenKind {
    if ch.is_whitespace() {
        SourceTokenKind::Space
    } else if ch.is_ascii_punctuation() {
        SourceTokenKind::Punctuation
    } else {
        SourceTokenKind::Word
    }
}

fn tokens_typst_text(tokens: &[diff::Token]) -> String {
    typst_escape(
        tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>()
            .as_str(),
    )
}

fn typst_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\\' | '[' | ']' | '#' => ['\\', c].into_iter().collect::<Vec<_>>(),
            _ => [c].into_iter().collect(),
        })
        .collect()
}

fn emit_cli_trace(
    trace_writer: &mut Option<debug::JsonlTraceWriter>,
    event: trace::PipelineTraceEvent,
) -> Result<()> {
    if let Some(writer) = trace_writer.as_mut() {
        let mut sink = Some(writer as &mut dyn trace::DebugEventSink);
        trace::emit_pipeline_trace_event(&mut sink, event)?;
    }
    Ok(())
}

fn debug_args(args: &Args) -> debug::DebugArgs {
    debug::DebugArgs {
        old_or_file: args.old_or_file.clone(),
        new: args.new.clone(),
        revision: args.revision.clone(),
        output: args.output.clone(),
        log_modifications: args.log_modifications.clone(),
        compact_substitutions: args.compact_substitutions,
        quiet: args.quiet,
        debug: args.debug,
        debug_trace: args.debug_trace,
    }
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
