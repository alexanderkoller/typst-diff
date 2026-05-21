use std::path::PathBuf;
use std::process::Command;

fn fixtures(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn world_for(path: &str) -> typst_diff::world::SystemWorld {
    typst_diff::world::SystemWorld::new(fixtures(path)).unwrap()
}

#[test]
fn simple_diff_produces_valid_pdf() {
    let old_world = world_for("simple_old.typ");
    let new_world = world_for("simple_new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let annotated = typst_diff::build_annotated_content(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

#[test]
fn cli_diffs_working_tree_against_git_revision() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("main.typ"),
        "= Title\n\n#include \"chapter.typ\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("chapter.typ"), "The old text.").unwrap();

    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Typst Diff Test"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "old"]);

    std::fs::write(dir.path().join("chapter.typ"), "The new text.").unwrap();
    let output = dir.path().join("diff.pdf");
    let status = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["main.typ", "--revision", "HEAD", "-o"])
        .arg(&output)
        .status()
        .unwrap();

    assert!(status.success(), "CLI failed");
    let pdf = std::fs::read(output).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

#[test]
fn multifile_diff_produces_valid_pdf() {
    let old_world = world_for("multifile_old/main.typ");
    let new_world = world_for("multifile_new/main.typ");
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let annotated = typst_diff::build_annotated_content(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

#[test]
fn table_cell_changes_are_reported_and_rendered() {
    let old_world = world_for("table_old.typ");
    let new_world = world_for("table_new.typ");
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("modify table cell"), "{log}");
    assert!(log.contains("4 | 650"), "{log}");
    assert!(log.contains("5 | 100"), "{log}");
    assert!(log.contains("108"), "{log}");
    assert!(log.contains("97"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

#[test]
fn math_expression_changes_are_reported_and_rendered() {
    let old_world = world_for("math_old.typ");
    let new_world = world_for("math_new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("[γ]"), "{log}");
    assert!(log.contains("2n"), "{log}");
    assert!(log.contains("/ 6") || log.contains("6"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}
