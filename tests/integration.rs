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

#[derive(Clone)]
struct TextRun {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    fill: typst::visualize::Paint,
}

fn rendered_text_runs(frame: &typst::layout::Frame) -> Vec<TextRun> {
    fn collect(frame: &typst::layout::Frame, origin: typst::layout::Point, out: &mut Vec<TextRun>) {
        for (pos, item) in frame.items() {
            let absolute = origin + *pos;
            match item {
                typst::layout::FrameItem::Text(text) => out.push(TextRun {
                    text: text.text.to_string(),
                    x: absolute.x.to_pt(),
                    y: absolute.y.to_pt(),
                    width: text.width().to_pt(),
                    fill: text.fill.clone(),
                }),
                typst::layout::FrameItem::Group(group) => {
                    collect(&group.frame, absolute, out);
                }
                typst::layout::FrameItem::Shape(_, _)
                | typst::layout::FrameItem::Image(_, _, _)
                | typst::layout::FrameItem::Link(_, _)
                | typst::layout::FrameItem::Tag(_) => {}
            }
        }
    }

    let mut out = Vec::new();
    collect(frame, typst::layout::Point::zero(), &mut out);
    out
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
        if let Some(block) = content.to_packed::<typst::layout::BlockElem>()
            && let Some(typst::layout::BlockBody::Content(body)) =
                block.body.get_cloned(Default::default())
        {
            return inner(&body, needle, styles);
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
    annotated_tree_corpus(name)
}

fn diff_annotated_corpus(name: &str) -> typst_diff::diff::DiffResult {
    let old_world = corpus_world(&format!("{name}/old.typ"));
    let new_world = corpus_world(&format!("{name}/new.typ"));
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    typst_diff::diff::diff_annotated(&old, &new)
}

fn annotated_tree_corpus(name: &str) -> Content {
    let result = diff_annotated_corpus(name);
    typst_diff::annotate::build_annotated_content_from_tree(&result, false)
}

fn text_has_any_style(content: &Content, needle: &str) -> bool {
    let mut found = false;
    let _ = content.traverse::<_, ()>(&mut |c| {
        if let Some(styled) = c.to_packed::<typst::foundations::StyledElem>()
            && styled.child.plain_text().contains(needle)
        {
            found = true;
        }
        std::ops::ControlFlow::Continue(())
    });
    found || max_style_count_for_text(content, needle) > 0
}

fn text_is_struck(content: &Content, needle: &str) -> bool {
    let mut found = false;
    let _ = content.traverse::<_, ()>(&mut |c| {
        if let Some(strike) = c.to_packed::<typst::text::StrikeElem>()
            && strike.body.plain_text().contains(needle)
        {
            found = true;
        }
        std::ops::ControlFlow::Continue(())
    });
    found
}

fn collect_modified_word_texts(blocks: &[typst_diff::diff::DiffBlockEdit]) -> (String, String) {
    use typst_diff::diff::{EditContent, RealizedEdit, WordOp};

    let mut deleted = Vec::new();
    let mut inserted = Vec::new();

    fn walk_content(content: &EditContent, deleted: &mut Vec<String>, inserted: &mut Vec<String>) {
        match content {
            EditContent::Modified { word_ops, .. } => {
                for op in word_ops {
                    match op {
                        WordOp::Delete(tokens) => {
                            deleted.push(tokens.iter().map(|t| t.text.as_str()).collect())
                        }
                        WordOp::Insert(tokens) => {
                            inserted.push(tokens.iter().map(|t| t.text.as_str()).collect())
                        }
                        WordOp::Equal(_) => {}
                    }
                }
            }
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(edit, deleted, inserted);
                }
            }
            EditContent::Inserted(_) | EditContent::Deleted(_) => {}
        }
    }

    fn walk_edit(edit: &RealizedEdit, deleted: &mut Vec<String>, inserted: &mut Vec<String>) {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content) => {
                walk_content(content, deleted, inserted);
            }
        }
    }

    for block in blocks {
        for edit in &block.edits {
            walk_edit(edit, &mut deleted, &mut inserted);
        }
    }
    (deleted.join(" | "), inserted.join(" | "))
}

fn collect_region_modified_word_texts(
    regions: &[typst_diff::diff::DiffRegionEdit],
) -> (String, String) {
    use typst_diff::diff::{EditContent, RealizedEdit, WordOp};

    fn walk_content(content: &EditContent, deleted: &mut Vec<String>, inserted: &mut Vec<String>) {
        match content {
            EditContent::Modified { word_ops, .. } => {
                for op in word_ops {
                    match op {
                        WordOp::Delete(tokens) => {
                            deleted.push(tokens.iter().map(|t| t.text.as_str()).collect())
                        }
                        WordOp::Insert(tokens) => {
                            inserted.push(tokens.iter().map(|t| t.text.as_str()).collect())
                        }
                        WordOp::Equal(_) => {}
                    }
                }
            }
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(edit, deleted, inserted);
                }
            }
            EditContent::Inserted(_) | EditContent::Deleted(_) => {}
        }
    }

    fn walk_edit(edit: &RealizedEdit, deleted: &mut Vec<String>, inserted: &mut Vec<String>) {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content) => walk_content(content, deleted, inserted),
        }
    }

    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    for region in regions {
        for edit in &region.edits {
            walk_edit(edit, &mut deleted, &mut inserted);
        }
    }
    (deleted.join(" | "), inserted.join(" | "))
}

fn collect_modified_bases(blocks: &[typst_diff::diff::DiffBlockEdit]) -> Vec<String> {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk_content(content: &EditContent, bases: &mut Vec<String>) {
        match content {
            EditContent::Modified { base, .. } => bases.push(base.plain_text().to_string()),
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(edit, bases);
                }
            }
            EditContent::Inserted(_) | EditContent::Deleted(_) => {}
        }
    }

    fn walk_edit(edit: &RealizedEdit, bases: &mut Vec<String>) {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content) => walk_content(content, bases),
        }
    }

    let mut bases = Vec::new();
    for block in blocks {
        for edit in &block.edits {
            walk_edit(edit, &mut bases);
        }
    }
    bases
}

fn count_edits(
    blocks: &[typst_diff::diff::DiffBlockEdit],
    matches_edit: fn(&typst_diff::diff::RealizedEdit) -> bool,
) -> usize {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk(edit: &RealizedEdit, matches_edit: fn(&RealizedEdit) -> bool) -> usize {
        let nested = match edit {
            RealizedEdit::ReplaceAt {
                content: EditContent::Nested { edits, .. },
                ..
            }
            | RealizedEdit::InsertBefore {
                content: EditContent::Nested { edits, .. },
                ..
            }
            | RealizedEdit::InsertAfter {
                content: EditContent::Nested { edits, .. },
                ..
            }
            | RealizedEdit::Append {
                content: EditContent::Nested { edits, .. },
            }
            | RealizedEdit::WholeBlock(EditContent::Nested { edits, .. }) => {
                edits.iter().map(|edit| walk(edit, matches_edit)).sum()
            }
            _ => 0,
        };
        usize::from(matches_edit(edit)) + nested
    }

    blocks
        .iter()
        .flat_map(|block| &block.edits)
        .map(|edit| walk(edit, matches_edit))
        .sum()
}

fn effective_plain_text(node: &typst_diff::annotated::AnnotatedContent) -> String {
    if !node.realized.plain_text().is_empty() || node.children.is_empty() {
        return node.realized.plain_text().to_string();
    }
    node.children
        .iter()
        .map(effective_plain_text)
        .collect::<Vec<_>>()
        .join("")
}

fn collect_edit_texts(
    blocks: &[typst_diff::diff::DiffBlockEdit],
) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    use typst_diff::diff::{EditContent, RealizedEdit, WordOp};

    fn walk_content(
        content: &EditContent,
        inserted: &mut Vec<String>,
        deleted: &mut Vec<String>,
        modified_inserted: &mut Vec<String>,
        modified_deleted: &mut Vec<String>,
    ) {
        match content {
            EditContent::Inserted(content) => {
                let text = content.plain_text().to_string();
                if !text.trim().is_empty() {
                    inserted.push(text);
                }
            }
            EditContent::Deleted(content) => {
                let text = content.plain_text().to_string();
                if !text.trim().is_empty() {
                    deleted.push(text);
                }
            }
            EditContent::Modified { word_ops, .. } => {
                for op in word_ops {
                    match op {
                        WordOp::Insert(tokens) => {
                            let text: String = tokens.iter().map(|t| t.text.as_str()).collect();
                            if !text.trim().is_empty() {
                                modified_inserted.push(text);
                            }
                        }
                        WordOp::Delete(tokens) => {
                            let text: String = tokens.iter().map(|t| t.text.as_str()).collect();
                            if !text.trim().is_empty() {
                                modified_deleted.push(text);
                            }
                        }
                        WordOp::Equal(_) => {}
                    }
                }
            }
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(edit, inserted, deleted, modified_inserted, modified_deleted);
                }
            }
        }
    }

    fn walk_edit(
        edit: &RealizedEdit,
        inserted: &mut Vec<String>,
        deleted: &mut Vec<String>,
        modified_inserted: &mut Vec<String>,
        modified_deleted: &mut Vec<String>,
    ) {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content) => {
                walk_content(
                    content,
                    inserted,
                    deleted,
                    modified_inserted,
                    modified_deleted,
                );
            }
        }
    }

    let mut inserted = Vec::new();
    let mut deleted = Vec::new();
    let mut modified_inserted = Vec::new();
    let mut modified_deleted = Vec::new();
    for block in blocks {
        for edit in &block.edits {
            walk_edit(
                edit,
                &mut inserted,
                &mut deleted,
                &mut modified_inserted,
                &mut modified_deleted,
            );
        }
    }
    (inserted, deleted, modified_inserted, modified_deleted)
}

fn assert_edit_contract_matches_render(corpus_name: &str) {
    let result = diff_annotated_corpus(corpus_name);
    let rendered = annotated_tree_corpus(corpus_name);
    let (inserted, deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);

    for text in inserted {
        assert!(
            rendered.plain_text().contains(&text),
            "Inserted status text missing from render for corpus {corpus_name}: {text:?}"
        );
        assert!(
            text_has_any_style(&rendered, &text),
            "Inserted status text present but not styled in corpus {corpus_name}: {text:?}"
        );
    }

    for text in deleted {
        assert!(
            rendered.plain_text().contains(&text),
            "Deleted status text missing from render for corpus {corpus_name}: {text:?}"
        );
        assert!(
            text_is_struck(&rendered, &text),
            "Deleted status text present but not struck in corpus {corpus_name}: {text:?}"
        );
    }

    for text in modified_inserted {
        let tokens: Vec<&str> = text.split_whitespace().filter(|t| t.len() >= 3).collect();
        if tokens.is_empty() {
            assert!(
                rendered.plain_text().contains(&text),
                "Modified-insert text missing from render for corpus {corpus_name}: {text:?}"
            );
            assert!(
                text_has_any_style(&rendered, &text),
                "Modified-insert text present but not styled in corpus {corpus_name}: {text:?}"
            );
        } else {
            for token in tokens {
                assert!(
                    rendered.plain_text().contains(token),
                    "Modified-insert token missing from render for corpus {corpus_name}: {token:?}"
                );
                assert!(
                    text_has_any_style(&rendered, token),
                    "Modified-insert token present but not styled in corpus {corpus_name}: {token:?}"
                );
            }
        }
    }

    for text in modified_deleted {
        let tokens: Vec<&str> = text.split_whitespace().filter(|t| t.len() >= 3).collect();
        if tokens.is_empty() {
            assert!(
                rendered.plain_text().contains(&text),
                "Modified-delete text missing from render for corpus {corpus_name}: {text:?}"
            );
            assert!(
                text_is_struck(&rendered, &text),
                "Modified-delete text present but not struck in corpus {corpus_name}: {text:?}"
            );
        } else {
            for token in tokens {
                assert!(
                    rendered.plain_text().contains(token),
                    "Modified-delete token missing from render for corpus {corpus_name}: {token:?}"
                );
                assert!(
                    text_is_struck(&rendered, token),
                    "Modified-delete token present but not struck in corpus {corpus_name}: {token:?}"
                );
            }
        }
    }
}

#[test]
fn simple_diff_produces_valid_pdf() {
    let old_world = world_for("simple_old.typ");
    let new_world = world_for("simple_new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_headings_keep_heading_formatting() {
    let annotated = annotated_corpus("12-heading-deleted");
    let plain = annotated.plain_text();

    assert!(plain.contains("Chapter Two"), "{plain}");
    let kept_heading_styles = max_style_count_for_text(&annotated, "Chapter One");
    let deleted_heading_styles = max_style_count_for_text(&annotated, "Chapter Two");
    assert!(
        deleted_heading_styles > kept_heading_styles,
        "deleted heading should keep heading styles and add deletion styles; kept={kept_heading_styles}, deleted={deleted_heading_styles}\n{plain}"
    );
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
    assert!(
        log.contains(&typst_diff::build_info::build_report_line()),
        "{log}"
    );
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
fn list_item_change_produces_slot_replace_edit_not_flat_modified() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("18-list-item-changed");

    let list_block = result
        .blocks
        .iter()
        .find(|b| !b.edits.is_empty())
        .expect("expected at least one changed block");

    assert!(
        list_block.edits.iter().any(|edit| matches!(
            edit,
            RealizedEdit::ReplaceAt {
                content: EditContent::Modified { .. },
                ..
            }
        )),
        "expected a path-addressed modified slot edit"
    );

    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        deleted.contains("Old") && deleted.contains("is being replaced"),
        "expected old changed chunks in deleted word ops, got {deleted:?}"
    );
    assert!(
        inserted.contains("New")
            && inserted.contains("replaces")
            && inserted.contains("the old one"),
        "expected new changed chunks in inserted word ops, got {inserted:?}"
    );
}

#[test]
fn list_item_added_produces_slot_level_insert_instead_of_flat_modified() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("19-list-item-added");
    let list_block = result
        .blocks
        .iter()
        .find(|b| !b.edits.is_empty())
        .expect("expected at least one changed block");

    assert_eq!(
        list_block.base.annotation.semantic_kind,
        Some(typst_diff::annotated::SemanticKind::List)
    );
    assert_eq!(
        list_block.base.annotation.slots.len(),
        4,
        "expected 4 list item slots"
    );
    let inserted = count_edits(&result.blocks, |edit| {
        matches!(
            edit,
            RealizedEdit::ReplaceAt {
                content: EditContent::Inserted(_),
                ..
            }
        )
    });
    assert_eq!(inserted, 1, "expected exactly one inserted list item");
    assert!(
        list_block.edits.iter().any(|edit| matches!(
            edit,
            RealizedEdit::ReplaceAt {
                content: EditContent::Inserted(content),
                ..
            } if content.plain_text().contains("Stable internet connection")
        )),
        "expected inserted list item text in inserted edit"
    );
    let child_texts: Vec<String> = list_block
        .base
        .annotation
        .slots
        .iter()
        .map(|slot| effective_plain_text(list_block.base.get_path(&slot.path).unwrap()))
        .collect();
    assert_eq!(
        child_texts,
        vec![
            "64-bit processor",
            "8 GB of RAM",
            "10 GB disk space",
            "Stable internet connection for updates",
        ],
        "slot paths should resolve to item bodies, not the realized list wrapper"
    );
}

#[test]
fn list_item_added_tree_render_contains_inserted_text() {
    let annotated = annotated_tree_corpus("19-list-item-added");
    let plain = annotated.plain_text();

    assert!(
        plain.contains("Stable internet connection"),
        "diff tree has inserted list item, but rendered annotated content omitted it: {plain:?}"
    );
}

#[test]
fn list_item_added_tree_render_styles_inserted_text() {
    let annotated = annotated_tree_corpus("19-list-item-added");

    assert!(
        text_has_any_style(&annotated, "Stable internet connection"),
        "inserted list item is present but not styled as changed"
    );
}

#[test]
fn list_item_added_tree_render_preserves_paragraph_list_boundary() {
    let annotated = annotated_tree_corpus("19-list-item-added");

    assert!(
        count_nodes::<typst::model::ParbreakElem>(&annotated) >= 1,
        "diff output should preserve the source blank line between paragraph and list"
    );
}

#[test]
fn list_item_change_tree_render_styles_deleted_and_inserted_text() {
    let annotated = annotated_tree_corpus("18-list-item-changed");

    assert!(
        text_is_struck(&annotated, "Old"),
        "deleted list item text should be struck in rendered annotated content"
    );
    assert!(
        text_has_any_style(&annotated, "New"),
        "inserted list item text should be styled in rendered annotated content"
    );
}

#[test]
fn nested_list_item_change_produces_nested_modified_child() {
    let result = diff_annotated_corpus("20-nested-list-changed");
    let list_block = result
        .blocks
        .iter()
        .find(|b| !b.edits.is_empty())
        .expect("expected at least one changed block");

    assert!(!list_block.edits.is_empty());

    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        deleted.contains("old") && deleted.contains("mammals"),
        "expected changed words from nested old description in deleted word ops, got {deleted:?}"
    );
    assert!(
        inserted.contains("updated") && inserted.contains("warm-blooded vertebrates"),
        "expected changed words from nested updated description in inserted word ops, got {inserted:?}"
    );

    let bases = collect_modified_bases(&result.blocks);
    assert!(
        bases.iter().any(|base| base
            == "Class: Mammalia (updated description of warm-blooded vertebrates)"),
        "expected modified base at level-3 class item, got {bases:?}"
    );
    assert!(
        bases.iter().all(|base| !base.contains("Phylum: Chordata")),
        "diff should not widen to the level-2 phylum item, got {bases:?}"
    );
}

#[test]
fn nested_list_item_change_tree_render_contains_old_and_new_text() {
    let annotated = annotated_tree_corpus("20-nested-list-changed");
    let plain = annotated.plain_text();

    assert!(
        plain.contains("old description of mammals"),
        "rendered annotated content omitted deleted nested list text: {plain:?}"
    );
    assert!(
        plain.contains("updated description of warm-blooded vertebrates"),
        "rendered annotated content omitted inserted nested list text: {plain:?}"
    );
}

#[test]
fn nested_list_item_change_tree_render_styles_old_and_new_text() {
    let annotated = annotated_tree_corpus("20-nested-list-changed");

    assert!(
        text_is_struck(&annotated, "old description of mammals"),
        "deleted nested list text should be struck in rendered annotated content"
    );
    assert!(
        text_has_any_style(
            &annotated,
            "updated description of warm-blooded vertebrates"
        ),
        "inserted nested list text should be styled in rendered annotated content"
    );
}

#[test]
fn edit_contract_guarantees_rendering_for_corpus_18() {
    assert_edit_contract_matches_render("18-list-item-changed");
}

#[test]
fn edit_contract_guarantees_rendering_for_corpus_19() {
    assert_edit_contract_matches_render("19-list-item-added");
}

#[test]
fn edit_contract_guarantees_rendering_for_corpus_20() {
    assert_edit_contract_matches_render("20-nested-list-changed");
}

#[test]
fn table_row_deleted_middle_keeps_deleted_cells_in_tree() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("65-table-row-deleted-middle");
    let deleted = count_edits(&result.blocks, |edit| {
        matches!(
            edit,
            RealizedEdit::InsertBefore {
                content: EditContent::Deleted(_),
                ..
            } | RealizedEdit::InsertAfter {
                content: EditContent::Deleted(_),
                ..
            } | RealizedEdit::Append {
                content: EditContent::Deleted(_),
            }
        )
    });
    assert_eq!(
        deleted, 3,
        "expected 3 deleted cells for removed middle row"
    );
}

#[test]
fn nested_list_item_inserted_uses_nested_changed_descendants() {
    let result = diff_annotated_corpus("69-nested-list-item-inserted");
    let list_block = result
        .blocks
        .iter()
        .find(|b| !b.edits.is_empty())
        .expect("expected changed outer list block");

    assert!(!list_block.edits.is_empty());
}

#[test]
fn table_changed_uses_changed_descendants_with_child_statuses() {
    let result = diff_annotated_corpus("35-table-changed");
    let table_block = result
        .blocks
        .iter()
        .find(|b| !b.edits.is_empty())
        .expect("expected changed table block");

    assert!(
        !table_block.edits.is_empty(),
        "expected at least one changed cell edit"
    );
}

#[test]
fn table_row_inserted_middle_includes_inserted_cells() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("64-table-row-inserted-middle");
    let inserted = count_edits(&result.blocks, |edit| {
        matches!(
            edit,
            RealizedEdit::ReplaceAt {
                content: EditContent::Inserted(_),
                ..
            }
        )
    });
    assert!(
        inserted >= 1,
        "expected inserted cells for the inserted middle row"
    );
}

#[test]
fn corpus_32_header_change_is_page_region_edit() {
    use typst_diff::diff::{PageRegionKind, RegionPath};

    let result = diff_annotated_corpus("32-headers-and-footers");
    assert!(
        result
            .regions
            .iter()
            .any(|region| region.path == RegionPath::RootPage(PageRegionKind::Header)),
        "expected a root page header region edit"
    );
    let (deleted, inserted) = collect_region_modified_word_texts(&result.regions);
    assert!(deleted.contains("Old"), "deleted region text: {deleted}");
    assert!(deleted.contains("Draft"), "deleted region text: {deleted}");
    assert!(inserted.contains("New"), "inserted region text: {inserted}");
    assert!(
        inserted.contains("Final"),
        "inserted region text: {inserted}"
    );

    let log = result.modification_log();
    assert!(log.contains("Old"), "{log}");
    assert!(log.contains("New"), "{log}");

    let new_world = corpus_world("32-headers-and-footers/new.typ");
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn corpus_42_contextual_footer_total_pages_is_rendered_region_edit() {
    use typst_diff::diff::PageRegionKind;

    let old_world = corpus_world("42-page-x-of-y/old.typ");
    let new_world = corpus_world("42-page-x-of-y/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();

    let footer = result
        .rendered_regions
        .iter()
        .find(|region| region.kind == PageRegionKind::Footer)
        .expect("expected contextual footer rendered region edit");
    assert_eq!(
        footer.pages.len(),
        3,
        "expected every new footer instance to be renderable"
    );

    let log = result.modification_log();
    assert!(log.contains("Page 1 of 3"), "{log}");
    assert!(log.contains("Page 2 of 3"), "{log}");
    assert!(log.contains("deleted: 2"), "{log}");
    assert!(log.contains("inserted: 3"), "{log}");

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let first_page = &document.pages[0].frame;
    let page_height = first_page.height().to_pt();
    let page_width = first_page.width().to_pt();
    let footer_runs = rendered_text_runs(first_page)
        .into_iter()
        .filter(|run| run.y > page_height * 0.8)
        .collect::<Vec<_>>();
    let footer_text = footer_runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>();
    assert!(footer_text.contains("Page 1 of 23"), "{footer_text}");
    assert!(
        footer_runs.iter().any(|run| {
            run.text == "2" && run.fill == typst::visualize::Color::from_u8(220, 0, 0, 255).into()
        }),
        "expected deleted page total to render red in footer"
    );
    assert!(
        footer_runs.iter().any(|run| {
            run.text == "3" && run.fill == typst::visualize::Color::from_u8(0, 180, 0, 255).into()
        }),
        "expected inserted page total to render green in footer"
    );
    let min_x = footer_runs
        .iter()
        .map(|run| run.x)
        .fold(f64::INFINITY, f64::min);
    let max_x = footer_runs
        .iter()
        .map(|run| run.x + run.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let footer_center = (min_x + max_x) / 2.0;
    assert!(
        (footer_center - page_width / 2.0).abs() < 10.0,
        "footer should stay centered: footer_center={footer_center}, page_width={page_width}"
    );

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn corpus_43_contextual_alternating_headers_are_rendered_region_edits() {
    use typst_diff::diff::PageRegionKind;

    let old_world = corpus_world("43-alternating-headers/old.typ");
    let new_world = corpus_world("43-alternating-headers/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();

    assert!(
        result
            .rendered_regions
            .iter()
            .any(|region| region.kind == PageRegionKind::Header),
        "expected contextual header rendered region edit"
    );

    let log = result.modification_log();
    assert!(log.contains("Old"), "{log}");
    assert!(log.contains("New"), "{log}");
    assert!(log.contains("Draft"), "{log}");
    assert!(log.contains("Final"), "{log}");
    assert!(
        !log.contains("Lorem ipsum"),
        "rendered header extraction should not absorb body text:\n{log}"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn header_footer_add_delete_change_are_page_region_edits() {
    use typst_diff::diff::{EditContent, PageRegionKind, RealizedEdit, RegionPath};

    fn has_region(name: &str, kind: PageRegionKind, matches_content: fn(&EditContent) -> bool) {
        let result = diff_annotated_corpus(name);
        let region = result
            .regions
            .iter()
            .find(|region| region.path == RegionPath::RootPage(kind))
            .unwrap_or_else(|| panic!("expected {kind:?} region for {name}"));
        assert!(
            region.edits.iter().any(|edit| match edit {
                RealizedEdit::WholeBlock(content)
                | RealizedEdit::ReplaceAt { content, .. }
                | RealizedEdit::InsertBefore { content, .. }
                | RealizedEdit::InsertAfter { content, .. }
                | RealizedEdit::Append { content } => matches_content(content),
            }),
            "unexpected region edits for {name}"
        );
    }

    has_region(
        "80-footer-text-changed",
        PageRegionKind::Footer,
        |content| matches!(content, EditContent::Modified { .. }),
    );
    has_region("81-header-added", PageRegionKind::Header, |content| {
        matches!(content, EditContent::Inserted(_))
    });
    has_region("82-header-deleted", PageRegionKind::Header, |content| {
        matches!(content, EditContent::Deleted(_))
    });
}

#[test]
fn grid_inside_header_uses_slot_level_region_edit() {
    use typst_diff::diff::{EditContent, PageRegionKind, RealizedEdit, RegionPath};

    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(header: grid(columns: (1fr, 1fr), [Old left], [Stable right]))

Body unchanged.
"#,
        r#"#set page(header: grid(columns: (1fr, 1fr), [New left], [Stable right]))

Body unchanged.
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let header = result
        .regions
        .iter()
        .find(|region| region.path == RegionPath::RootPage(PageRegionKind::Header))
        .expect("expected header region edit");
    assert!(
        header.edits.iter().any(|edit| matches!(
            edit,
            RealizedEdit::ReplaceAt {
                content: EditContent::Modified { .. },
                ..
            }
        )),
        "expected a slot-level replacement inside the header grid"
    );
    assert!(
        !header
            .edits
            .iter()
            .any(|edit| matches!(edit, RealizedEdit::WholeBlock(_))),
        "header grid should recurse instead of falling back to a whole-region edit"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn background_and_foreground_text_changes_are_page_region_edits() {
    use typst_diff::diff::{PageRegionKind, RegionPath};

    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(background: text(18pt)[DRAFT], foreground: text(8pt)[Review copy])

Body unchanged.
"#,
        r#"#set page(background: text(18pt)[FINAL], foreground: text(8pt)[Release copy])

Body unchanged.
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    assert!(
        result
            .regions
            .iter()
            .any(|region| region.path == RegionPath::RootPage(PageRegionKind::Background)),
        "expected background region edit"
    );
    assert!(
        result
            .regions
            .iter()
            .any(|region| region.path == RegionPath::RootPage(PageRegionKind::Foreground)),
        "expected foreground region edit"
    );
    let (deleted, inserted) = collect_region_modified_word_texts(&result.regions);
    assert!(deleted.contains("DRAFT"), "deleted region text: {deleted}");
    assert!(
        inserted.contains("FINAL"),
        "inserted region text: {inserted}"
    );
    assert!(deleted.contains("Review"), "deleted region text: {deleted}");
    assert!(
        inserted.contains("Release"),
        "inserted region text: {inserted}"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn multifile_diff_produces_valid_pdf() {
    let old_world = world_for("multifile_old/main.typ");
    let new_world = world_for("multifile_new/main.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
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

    let result = typst_diff::diff::diff_annotated(&old, &new);
    let log = result.modification_log();
    assert!(log.contains("vertices"), "{log}");
    assert!(log.contains("nodes"), "{log}");
    assert!(log.contains("tree"), "{log}");
    assert!(log.contains("forest"), "{log}");
    assert!(!log.contains("spanning tree as a subgraph"), "{log}");

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
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
fn inline_math_changes_are_diffed_as_equation_tokens() {
    let (_dir, old_world, new_world) = temp_worlds(
        "The kinetic energy is $E_k = 1/2 m v^2$ where $m$ is mass and $v$ is velocity.\n\nThe potential energy satisfies $E_p = m g h$ near the Earth's surface.",
        "The kinetic energy is $E_k = 1/2 m v^2$ where $m$ is mass and $v$ is velocity.\n\nThe total mechanical energy satisfies $E = E_k + E_p$ when no friction is present.",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();

    let result = typst_diff::diff::diff_annotated(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("deleted: potential"), "{log}");
    assert!(log.contains("inserted: total mechanical"), "{log}");
    assert!(log.contains("attach(base: [E], b: [p])"), "{log}");
    assert!(log.contains("attach(base: [E], b: [k])"), "{log}");
}

#[test]
fn display_math_changes_are_diffed_as_equation_tokens() {
    let (_dir, old_world, new_world) = temp_worlds(
        "The sum is:\n\n$ sum_(i=1)^n i = (n(n+1))/2 $\n\nThis result is known as the triangular number formula.",
        "The sum is:\n\n$ sum_(i=1)^n i^2 = (n(n+1)(2n+1))/6 $\n\nThis result is the sum-of-squares formula.",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();

    let result = typst_diff::diff::diff_annotated(&old, &new);
    let log = result.modification_log();

    assert!(log.contains("frac("), "{log}");
    assert!(log.contains("attach(base: [i], t: [2])"), "{log}");
    assert!(log.contains("sum-of-squares"), "{log}");
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

    let content = typst_diff::eval_to_realized_content(&world)
        .unwrap()
        .realized;
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
