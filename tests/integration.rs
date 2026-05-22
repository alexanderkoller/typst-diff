use std::path::PathBuf;
use std::process::Command;

use typst::foundations::Content;

fn fixtures(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn world_for(path: &str) -> typst_diff::world::SystemWorld {
    typst_diff::world::SystemWorld::new(fixtures(path)).unwrap()
}

fn corpus(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(rel)
}

fn corpus_world(path: &str) -> typst_diff::world::SystemWorld {
    typst_diff::world::SystemWorld::new(corpus(path)).unwrap()
}

fn temp_worlds(
    old_source: &str,
    new_source: &str,
) -> (
    tempfile::TempDir,
    typst_diff::world::SystemWorld,
    typst_diff::world::SystemWorld,
) {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.typ"), old_source).unwrap();
    std::fs::write(dir.path().join("new.typ"), new_source).unwrap();
    let old_world = typst_diff::world::SystemWorld::new(dir.path().join("old.typ")).unwrap();
    let new_world = typst_diff::world::SystemWorld::new(dir.path().join("new.typ")).unwrap();
    (dir, old_world, new_world)
}

fn assert_valid_pdf(pdf: &[u8]) {
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

fn count_nodes<T: typst::foundations::NativeElement>(content: &Content) -> usize {
    let mut count = 0;
    let _ = content.traverse::<_, ()>(&mut |c| {
        if c.is::<T>() {
            count += 1;
        }
        std::ops::ControlFlow::Continue(())
    });
    count
}

fn assert_contains_in_order(text: &str, needles: &[&str]) {
    let mut start = 0;
    for needle in needles {
        let found = text[start..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found after byte {start} in:\n{text}"));
        start += found + needle.len();
    }
}

fn assert_command_success(output: std::process::Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn max_style_count_for_text(content: &Content, needle: &str) -> usize {
    fn inner(content: &Content, needle: &str, styles: usize) -> usize {
        if let Some(text) = content.to_packed::<typst::text::TextElem>()
            && text.text.as_str().contains(needle)
        {
            return styles;
        }
        if let Some(seq) = content.to_packed::<typst::foundations::SequenceElem>() {
            return seq
                .children
                .iter()
                .map(|child| inner(child, needle, styles))
                .max()
                .unwrap_or(0);
        }
        if let Some(styled) = content.to_packed::<typst::foundations::StyledElem>() {
            return inner(&styled.child, needle, styles + styled.styles.iter().count());
        }
        if let Some(par) = content.to_packed::<typst::model::ParElem>() {
            return inner(&par.body, needle, styles);
        }
        if let Some(heading) = content.to_packed::<typst::model::HeadingElem>() {
            return inner(&heading.body, needle, styles);
        }
        if let Some(strong) = content.to_packed::<typst::model::StrongElem>() {
            return inner(&strong.body, needle, styles + 1);
        }
        if let Some(emph) = content.to_packed::<typst::model::EmphElem>() {
            return inner(&emph.body, needle, styles + 1);
        }
        if let Some(strike) = content.to_packed::<typst::text::StrikeElem>() {
            return inner(&strike.body, needle, styles + 1);
        }
        if content.plain_text().contains(needle) {
            return styles;
        }
        0
    }

    inner(content, needle, 0)
}

fn annotated_corpus(name: &str) -> Content {
    let old_world = corpus_world(&format!("{name}/old.typ"));
    let new_world = corpus_world(&format!("{name}/new.typ"));
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    typst_diff::build_annotated_content(&result, false)
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
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_headings_keep_heading_formatting() {
    let annotated = annotated_corpus("12-heading-deleted");
    let plain = annotated.plain_text();

    assert!(plain.contains("Chapter Two"), "{plain}");
    assert!(max_style_count_for_text(&annotated, "Chapter Two") >= 2);
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);
}

#[test]
fn bold_changes_keep_strong_formatting() {
    let annotated = annotated_corpus("14-bold-content-changed");
    let plain = annotated.plain_text();

    assert!(plain.contains("old"), "{plain}");
    assert!(plain.contains("new"), "{plain}");
    assert!(plain.contains("technical concept"), "{plain}");
    assert!(
        max_style_count_for_text(&annotated, "old") >= 2,
        "old style count: {}\n{plain}",
        max_style_count_for_text(&annotated, "old")
    );
    assert!(
        max_style_count_for_text(&annotated, "new") >= 2,
        "new style count: {}\n{plain}",
        max_style_count_for_text(&annotated, "new")
    );
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);
}

#[test]
fn emphasis_changes_keep_emphasis_formatting() {
    let annotated = annotated_corpus("15-emph-content-changed");
    let plain = annotated.plain_text();

    assert!(plain.contains("Felis"), "{plain}");
    assert!(plain.contains("domesticus"), "{plain}");
    assert!(plain.contains("catus"), "{plain}");
    assert!(
        max_style_count_for_text(&annotated, "domesticus") >= 2,
        "domesticus style count: {}\n{plain}",
        max_style_count_for_text(&annotated, "domesticus")
    );
    assert!(
        max_style_count_for_text(&annotated, "catus") >= 2,
        "catus style count: {}\n{plain}",
        max_style_count_for_text(&annotated, "catus")
    );
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);
}

#[test]
fn source_strikethrough_survives_annotation() {
    let annotated = annotated_corpus("17-source-has-strikethrough");
    let plain = annotated.plain_text();

    assert!(plain.contains("€120"), "{plain}");
    assert!(plain.contains("€95"), "{plain}");
    assert!(plain.contains("€89"), "{plain}");
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) >= 3);
}

#[test]
fn whole_document_rewrite_keeps_deleted_and_inserted_heading_formatting() {
    let annotated = annotated_corpus("27-whole-document-rewrite");
    let plain = annotated.plain_text();

    assert!(plain.contains("Medieval History"), "{plain}");
    assert!(plain.contains("Modern Computing"), "{plain}");
    assert!(max_style_count_for_text(&annotated, "Medieval") >= 2);
    assert!(
        max_style_count_for_text(&annotated, "Modern") >= 2,
        "Modern style count: {}\n{plain}",
        max_style_count_for_text(&annotated, "Modern")
    );
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);
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
    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["main.typ", "--revision", "HEAD", "-o"])
        .arg(&output)
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff");
    let pdf = std::fs::read(output).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn cli_requires_new_document_or_revision() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.typ"), "Hello").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .arg("main.typ")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing new document path"), "{stderr}");
}

#[test]
fn cli_rejects_missing_input_file() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("new.typ"), "Hello").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["missing.typ", "new.typ"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to load old document"), "{stderr}");
}

#[test]
fn cli_writes_modification_log_when_requested() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.typ"), "The old text.").unwrap();
    std::fs::write(dir.path().join("new.typ"), "The new text.").unwrap();
    let pdf_path = dir.path().join("diff.pdf");
    let log_path = dir.path().join("mods.txt");

    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["old.typ", "new.typ", "-o"])
        .arg(&pdf_path)
        .args(["-l"])
        .arg(&log_path)
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff");
    assert_valid_pdf(&std::fs::read(pdf_path).unwrap());
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("modify"), "{log}");
    assert!(log.contains("old"), "{log}");
    assert!(log.contains("new"), "{log}");
}

#[test]
fn cli_revision_mode_handles_file_in_subdirectory_with_include() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("chapters")).unwrap();
    std::fs::write(
        dir.path().join("chapters/main.typ"),
        "#include \"part.typ\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("chapters/part.typ"), "The old text.").unwrap();

    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Typst Diff Test"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "old"]);

    std::fs::write(dir.path().join("chapters/part.typ"), "The new text.").unwrap();
    let output = dir.path().join("diff.pdf");
    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path().join("chapters"))
        .args(["main.typ", "--revision", "HEAD", "-o"])
        .arg(&output)
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff");
    assert_valid_pdf(&std::fs::read(output).unwrap());
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert_command_success(output, &format!("git {args:?}"));
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
    assert_valid_pdf(&pdf);
}

#[test]
fn table_cell_changes_are_reported_and_rendered() {
    let old_world = world_for("table_old.typ");
    let new_world = world_for("table_new.typ");
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("modify slot"), "{log}");
    assert!(log.contains("TableCell(3)"), "{log}");
    assert!(log.contains("TableCell(5)"), "{log}");
    assert!(log.contains("4 650"), "{log}");
    assert!(log.contains("5 100"), "{log}");
    assert!(log.contains("108"), "{log}");
    assert!(log.contains("97"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert!(plain.contains("Metric"), "{plain}");
    assert!(plain.contains("Actual"), "{plain}");
    assert!(plain.contains("Throughput"), "{plain}");
    assert!(plain.contains("Latency"), "{plain}");
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn list_item_changes_are_reported_and_rendered() {
    let (_dir, old_world, new_world) = temp_worlds(
        "- Alpha item\n- Beta item\n- Gamma item\n",
        "- Alpha item\n- Better item\n- Gamma item\n",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_plain = new.plain_text();
    assert!(new_plain.contains("Alpha item"), "{new_plain}");
    assert!(new_plain.contains("Better item"), "{new_plain}");
    assert!(new_plain.contains("Gamma item"), "{new_plain}");

    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("modify slot"), "{log}");
    assert!(
        log.contains("ItemBody") || log.contains("ListItem"),
        "{log}"
    );
    assert!(log.contains("Beta"), "{log}");
    assert!(log.contains("Better"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert!(plain.contains("Alpha item"), "{plain}");
    assert!(plain.contains("Beta"), "{plain}");
    assert!(plain.contains("Better"), "{plain}");
    assert!(plain.contains("Gamma item"), "{plain}");
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn footnote_body_changes_are_reported_without_inline_leakage() {
    let (_dir, old_world, new_world) = temp_worlds(
        "The API remains stable#footnote[Old footnote guidance for deployers.].\n\nThe rest of the paragraph is unchanged.\n",
        "The API remains stable#footnote[New footnote guidance for operators.].\n\nThe rest of the paragraph is unchanged.\n",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("modify slot"), "{log}");
    assert!(log.contains("FootnoteBody"), "{log}");
    assert!(log.contains("Old"), "{log}");
    assert!(log.contains("New"), "{log}");
    assert!(log.contains("deployers"), "{log}");
    assert!(log.contains("operators"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert!(plain.contains("The API remains stable"), "{plain}");
    assert!(
        plain.contains("The rest of the paragraph is unchanged."),
        "{plain}"
    );
    assert!(plain.contains("Old"), "{plain}");
    assert!(plain.contains("New"), "{plain}");
    assert!(plain.contains("footnote guidance"), "{plain}");
    assert!(plain.contains("deployers"), "{plain}");
    assert!(plain.contains("operators"), "{plain}");
    assert_eq!(count_nodes::<typst::model::FootnoteElem>(&annotated), 1);
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn repeated_function_expansions_with_same_span_keep_their_own_content() {
    let old_world = corpus_world("39-fn-content-args-changed/old.typ");
    let new_world = corpus_world("39-fn-content-args-changed/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();

    let new_plain = new.plain_text();
    assert!(new_plain.contains("Definition 1"), "{new_plain}");
    assert!(new_plain.contains("Definition 2"), "{new_plain}");
    assert!(new_plain.contains("Theorem"), "{new_plain}");
    assert_eq!(new_plain.matches("Theorem").count(), 1, "{new_plain}");

    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();
    assert!(log.contains("vertices"), "{log}");
    assert!(log.contains("nodes"), "{log}");
    assert!(log.contains("tree"), "{log}");
    assert!(log.contains("forest"), "{log}");
    assert!(!log.contains("spanning tree as a subgraph"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let annotated_plain = annotated.plain_text();
    assert!(
        annotated_plain.contains("Definition 1"),
        "{annotated_plain}"
    );
    assert!(
        annotated_plain.contains("Definition 2"),
        "{annotated_plain}"
    );
    assert!(annotated_plain.contains("Theorem"), "{annotated_plain}");
    assert_eq!(
        annotated_plain.matches("Theorem").count(),
        1,
        "{annotated_plain}"
    );
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn repeated_same_span_blocks_preserve_document_order() {
    let (_dir, _old_world, world) = temp_worlds(
        "",
        r#"
#let panel(title, body) = block(
  inset: 6pt,
  width: 100%,
  [#title: #body],
)

#panel("Alpha")[first body]
#panel("Beta")[second body]
#panel("Gamma")[third body]
#panel("Delta")[fourth body]
"#,
    );

    let content = typst_diff::eval_to_realized_content(&world).unwrap();
    let plain = content.plain_text();
    assert_contains_in_order(
        &plain,
        &[
            "Alpha",
            "first body",
            "Beta",
            "second body",
            "Gamma",
            "third body",
            "Delta",
            "fourth body",
        ],
    );
    assert_eq!(plain.matches("Delta").count(), 1, "{plain}");
}

#[test]
fn repeated_same_span_blocks_diff_only_changed_invocations() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"
#let panel(title, body) = block(
  inset: 6pt,
  width: 100%,
  [#title: #body],
)

#panel("Alpha")[stable first]
#panel("Beta")[legacy middle term]
#panel("Gamma")[stable third]
#panel("Delta")[classic final term]
"#,
        r#"
#let panel(title, body) = block(
  inset: 6pt,
  width: 100%,
  [#title: #body],
)

#panel("Alpha")[stable first]
#panel("Beta")[fresh middle term]
#panel("Gamma")[stable third]
#panel("Delta")[modern final term]
"#,
    );

    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert_eq!(log.matches("modify slot").count(), 2, "{log}");
    assert!(log.contains("legacy"), "{log}");
    assert!(log.contains("fresh"), "{log}");
    assert!(log.contains("classic"), "{log}");
    assert!(log.contains("modern"), "{log}");
    assert!(!log.contains("stable first"), "{log}");
    assert!(!log.contains("stable third"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert_contains_in_order(
        &plain,
        &[
            "Alpha",
            "stable first",
            "Beta",
            "legacy",
            "fresh",
            "Gamma",
            "stable third",
            "Delta",
            "classic",
            "modern",
        ],
    );
    assert!(count_nodes::<typst::text::StrikeElem>(&annotated) > 0);

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn repeated_same_span_nested_slots_keep_separate_children() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"
#let checklist(title, body) = block(
  inset: 6pt,
  width: 100%,
  [*#title* #body],
)

#checklist("Release")[
- Build artifacts
- Smoke test staging target
]

#checklist("Docs")[
- Update changelog
- Publish draft guide
]
"#,
        r#"
#let checklist(title, body) = block(
  inset: 6pt,
  width: 100%,
  [*#title* #body],
)

#checklist("Release")[
- Build artifacts
- Smoke test canary target
]

#checklist("Docs")[
- Update changelog
- Publish release guide
]
"#,
    );

    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_plain = new.plain_text();
    assert_contains_in_order(
        &new_plain,
        &[
            "Release",
            "Build artifacts",
            "Smoke test canary target",
            "Docs",
            "Update changelog",
            "Publish release guide",
        ],
    );

    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();
    assert_eq!(log.matches("modify slot").count(), 2, "{log}");
    assert!(log.contains("staging"), "{log}");
    assert!(log.contains("canary"), "{log}");
    assert!(log.contains("draft"), "{log}");
    assert!(log.contains("release"), "{log}");
    assert!(!log.contains("Build artifacts"), "{log}");
    assert!(!log.contains("Update changelog"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert_contains_in_order(
        &plain,
        &[
            "Release",
            "Build artifacts",
            "staging",
            "canary",
            "Docs",
            "Update changelog",
            "draft",
            "release",
        ],
    );

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn repeated_same_span_table_wrappers_preserve_cell_identity() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"
#let metric_table(title, value) = block[
  *#title*
  #table(
    columns: 2,
    [Metric], [Value],
    [Latency], value,
  )
]

#metric_table("North", [42 ms])
#metric_table("South", [18 ms])
"#,
        r#"
#let metric_table(title, value) = block[
  *#title*
  #table(
    columns: 2,
    [Metric], [Value],
    [Latency], value,
  )
]

#metric_table("North", [47 ms])
#metric_table("South", [18 ms])
"#,
    );

    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_plain = new.plain_text();
    assert_contains_in_order(&new_plain, &["North", "47 ms", "South", "18 ms"]);

    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();
    assert_eq!(log.matches("modify slot").count(), 1, "{log}");
    assert!(log.contains("42"), "{log}");
    assert!(log.contains("47"), "{log}");
    assert!(!log.contains("South"), "{log}");
    assert!(!log.contains("18 ms"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let plain = annotated.plain_text();
    assert_contains_in_order(&plain, &["North", "42", "47", "South", "18 ms"]);

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn figure_caption_changes_are_reported_and_rendered() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("old.typ"),
        "#figure(rect(width: 20pt, height: 10pt), caption: [Old caption])\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("new.typ"),
        "#figure(rect(width: 20pt, height: 10pt), caption: [New caption])\n",
    )
    .unwrap();

    let old_world = typst_diff::world::SystemWorld::new(dir.path().join("old.typ")).unwrap();
    let new_world = typst_diff::world::SystemWorld::new(dir.path().join("new.typ")).unwrap();
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("modify slot"), "{log}");
    assert!(log.contains("FigureCaption"), "{log}");
    assert!(log.contains("Old"), "{log}");
    assert!(log.contains("New"), "{log}");

    let annotated = typst_diff::build_annotated_content(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
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
    assert_valid_pdf(&pdf);
}
