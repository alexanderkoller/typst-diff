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

#[derive(Clone, Debug)]
struct TextRun {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    size: f64,
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
                    size: text.size.to_pt(),
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

fn run_size_with_fill(runs: &[TextRun], needle: &str, fill: typst::visualize::Paint) -> f64 {
    runs.iter()
        .find(|run| run.text.contains(needle) && run.fill == fill)
        .unwrap_or_else(|| {
            panic!("expected run containing {needle:?} with fill {fill:?}: {runs:?}")
        })
        .size
}

fn run_size(runs: &[TextRun], needle: &str) -> f64 {
    runs.iter()
        .find(|run| run.text.contains(needle))
        .unwrap_or_else(|| panic!("expected run containing {needle:?}: {runs:?}"))
        .size
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

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
            EditContent::Inserted(_)
            | EditContent::Deleted(_)
            | EditContent::OpaqueReplacement { .. } => {}
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

fn diff_temp_sources(
    old_source: &str,
    new_source: &str,
) -> (tempfile::TempDir, typst_diff::diff::DiffResult, Content) {
    let (dir, old_world, new_world) = temp_worlds(old_source, new_source);
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    (dir, result, annotated)
}

fn diff_temp_sources_with_world(
    old_source: &str,
    new_source: &str,
) -> (
    tempfile::TempDir,
    typst_diff::world::SystemWorld,
    typst_diff::diff::DiffResult,
    Content,
) {
    let (dir, old_world, new_world) = temp_worlds(old_source, new_source);
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    (dir, new_world, result, annotated)
}

fn rendered_main_body_and_footer_text(
    content: &Content,
    world: &typst_diff::world::SystemWorld,
) -> (String, String, Vec<TextRun>, Vec<TextRun>) {
    let document = typst_diff::eval::layout_document(world, content).unwrap();
    let first_page = &document.pages[0].frame;
    let page_height = first_page.height().to_pt();
    let (body_runs, footer_runs): (Vec<_>, Vec<_>) = rendered_text_runs(first_page)
        .into_iter()
        .partition(|run| run.y <= page_height * 0.8);
    let body = normalize_whitespace(
        &body_runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
    );
    let footer = normalize_whitespace(
        &footer_runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
    );
    (body, footer, body_runs, footer_runs)
}

fn assert_modified_words_include(
    result: &typst_diff::diff::DiffResult,
    deleted_needles: &[&str],
    inserted_needles: &[&str],
) {
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    for needle in deleted_needles {
        assert!(
            deleted.contains(needle),
            "expected deleted modified word {needle:?}; deleted={deleted:?}; inserted={inserted:?}"
        );
    }
    for needle in inserted_needles {
        assert!(
            inserted.contains(needle),
            "expected inserted modified word {needle:?}; deleted={deleted:?}; inserted={inserted:?}"
        );
    }
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
            EditContent::Inserted(_)
            | EditContent::Deleted(_)
            | EditContent::OpaqueReplacement { .. } => {}
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
            EditContent::Inserted(_)
            | EditContent::Deleted(_)
            | EditContent::OpaqueReplacement { .. } => {}
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
            EditContent::OpaqueReplacement { .. } => {}
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

fn assert_edit_paths_resolve_for_base(
    base: &typst_diff::annotated::AnnotatedContent,
    edits: &[typst_diff::diff::RealizedEdit],
) {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk_content(content: &EditContent) {
        if let EditContent::Nested { base, edits } = content {
            assert_edit_paths_resolve_for_base(base, edits);
        }
    }

    for edit in edits {
        match edit {
            RealizedEdit::ReplaceAt { path, content } => {
                assert!(
                    base.get_path(path).is_some(),
                    "ReplaceAt path does not resolve: {:?}",
                    path
                );
                walk_content(content);
            }
            RealizedEdit::InsertBefore { anchor, content }
            | RealizedEdit::InsertAfter { anchor, content } => {
                assert!(
                    base.get_path(anchor).is_some(),
                    "Insert anchor path does not resolve: {:?}",
                    anchor
                );
                walk_content(content);
            }
            RealizedEdit::Append { content } | RealizedEdit::WholeBlock(content) => {
                walk_content(content);
            }
        }
    }
}

fn edit_content_or_nested_matches(
    content: &typst_diff::diff::EditContent,
    matches_content: fn(&typst_diff::diff::EditContent) -> bool,
) -> bool {
    if matches_content(content) {
        return true;
    }
    if let typst_diff::diff::EditContent::Nested { edits, .. } = content {
        return edits
            .iter()
            .any(|edit| realized_edit_content_or_nested_matches(edit, matches_content));
    }
    false
}

fn realized_edit_content_or_nested_matches(
    edit: &typst_diff::diff::RealizedEdit,
    matches_content: fn(&typst_diff::diff::EditContent) -> bool,
) -> bool {
    use typst_diff::diff::RealizedEdit;

    match edit {
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content) => {
            edit_content_or_nested_matches(content, matches_content)
        }
    }
}

fn realized_edit_contains_replace_at_modified(edit: &typst_diff::diff::RealizedEdit) -> bool {
    use typst_diff::diff::{EditContent, RealizedEdit};

    match edit {
        RealizedEdit::ReplaceAt {
            content: EditContent::Modified { .. },
            ..
        } => true,
        RealizedEdit::ReplaceAt { content, .. }
        | RealizedEdit::InsertBefore { content, .. }
        | RealizedEdit::InsertAfter { content, .. }
        | RealizedEdit::Append { content }
        | RealizedEdit::WholeBlock(content) => {
            if let EditContent::Nested { edits, .. } = content {
                edits.iter().any(realized_edit_contains_replace_at_modified)
            } else {
                false
            }
        }
    }
}

fn changed_blocks(result: &typst_diff::diff::DiffResult) -> Vec<&typst_diff::diff::DiffBlockEdit> {
    result
        .blocks
        .iter()
        .filter(|block| !block.edits.is_empty())
        .collect()
}

#[test]
fn same_visible_text_visual_changes_are_reported_as_modifications() {
    let result = diff_annotated_corpus("36-heading-to-paragraph");
    let log = result.modification_log();
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        !changed_blocks(&result).is_empty(),
        "36-heading-to-paragraph should have at least one changed block"
    );
    assert!(
        deleted.contains("Background information"),
        "deleted={deleted:?}\n{log}"
    );
    assert!(
        inserted.contains("Background information"),
        "inserted={inserted:?}\n{log}"
    );

    for (case, needle) in [
        ("88-show-strong-style-only-change", "important"),
        ("94-bold-added-to-existing-word", "important"),
        ("97-highlight-color-changed", "critical"),
        ("98-subscript-superscript-style-change", "2"),
    ] {
        let result = diff_annotated_corpus(case);
        assert!(
            !changed_blocks(&result).is_empty(),
            "{case} should have at least one changed block"
        );
        assert_modified_words_include(&result, &[needle], &[needle]);
    }
}

#[test]
fn inserted_display_equation_is_tokenized_and_logged() {
    let result = diff_annotated_corpus("101-equation-number-reference-changed");
    let log = result.modification_log();
    let (_deleted, inserted) = collect_modified_word_texts(&result.blocks);

    assert!(inserted.contains('x'), "inserted={inserted:?}\n{log}");
    assert!(inserted.contains('y'), "inserted={inserted:?}\n{log}");
    assert!(inserted.contains("revised"), "inserted={inserted:?}\n{log}");
    assert!(log.contains("inserted:"), "{log}");
    assert!(log.contains('x'), "{log}");
    assert!(log.contains('y'), "{log}");
    assert!(!log.contains("text: \n"), "{log}");
}

#[test]
fn raw_block_changes_use_authored_lines_once() {
    let result = diff_annotated_corpus("24-code-block-changed");
    let log = result.modification_log();
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);

    assert!(
        deleted.contains("fn greet() -> &'static str {"),
        "deleted={deleted:?}\n{log}"
    );
    assert!(
        inserted.contains("fn greet(name: &str) -> String {"),
        "inserted={inserted:?}\n{log}"
    );
    assert!(
        !deleted.contains("fn greet() -> &'static str {fn greet() -> &'static str {"),
        "raw delete tokens should come from authored raw text, not synthesized RawLine plain text:\n{log}"
    );
    assert!(
        !inserted.contains("fn greet(name: &str) -> String {fn greet(name: &str) -> String {"),
        "raw insert tokens should come from authored raw text, not synthesized RawLine plain text:\n{log}"
    );
}

#[test]
fn same_visible_text_metadata_only_changes_stay_noop() {
    for case in [
        "95-link-target-changed-same-text",
        "96-label-changed-same-text",
    ] {
        let result = diff_annotated_corpus(case);
        let log = result.modification_log();

        assert!(changed_blocks(&result).is_empty(), "{case} changed:\n{log}");
        assert!(!log.lines().any(|line| line.starts_with("## ")), "{log}");
    }
}

fn only_changed_figure_block<'a>(
    result: &'a typst_diff::diff::DiffResult,
    case: &str,
) -> &'a typst_diff::diff::DiffBlockEdit {
    let changed = changed_blocks(result);
    assert_eq!(
        changed.len(),
        1,
        "{case} should produce one semantic-owner edit block"
    );
    let block = changed[0];
    assert!(
        matches!(
            block.base.annotation.semantic_kind,
            Some(typst_diff::annotated::SemanticKind::Figure)
        ),
        "{case} changed block should be owned by the figure, not layout scaffolding"
    );
    block
}

fn assert_figure_slots_are_patch_surface_paths(block: &typst_diff::diff::DiffBlockEdit) {
    use typst_diff::annotated::SlotStep;

    let body = block
        .base
        .annotation
        .slots
        .iter()
        .find(|slot| matches!(slot.label, SlotStep::FigureBody))
        .expect("figure body slot should exist");
    assert_eq!(body.path, vec![0]);

    if let Some(caption) = block
        .base
        .annotation
        .slots
        .iter()
        .find(|slot| matches!(slot.label, SlotStep::FigureCaption))
    {
        assert_eq!(caption.path, vec![1]);
    }
}

fn edit_is_whole_block_insert_or_delete(edit: &typst_diff::diff::RealizedEdit) -> bool {
    use typst_diff::diff::{EditContent, RealizedEdit};

    matches!(
        edit,
        RealizedEdit::WholeBlock(EditContent::Inserted(_))
            | RealizedEdit::WholeBlock(EditContent::Deleted(_))
    )
}

fn edit_content_is_opaque(content: &typst_diff::diff::EditContent) -> bool {
    matches!(
        content,
        typst_diff::diff::EditContent::OpaqueReplacement { .. }
    )
}

fn count_opaque_replacements(blocks: &[typst_diff::diff::DiffBlockEdit]) -> usize {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk(content: &EditContent) -> usize {
        match content {
            EditContent::OpaqueReplacement { .. } => 1,
            EditContent::Nested { edits, .. } => edits.iter().map(walk_edit).sum(),
            EditContent::Inserted(_) | EditContent::Deleted(_) | EditContent::Modified { .. } => 0,
        }
    }

    fn walk_edit(edit: &RealizedEdit) -> usize {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content) => walk(content),
        }
    }

    blocks
        .iter()
        .flat_map(|block| &block.edits)
        .map(walk_edit)
        .sum()
}

fn plain_occurrences(content: &Content, needle: &str) -> usize {
    content.plain_text().matches(needle).count()
}

#[test]
fn figure_caption_slot_paths_ignore_realized_layout_scaffolding() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("34-figure-with-caption");
    let figure = changed_blocks(&result)
        .into_iter()
        .find(|block| {
            matches!(
                block.base.annotation.semantic_kind,
                Some(typst_diff::annotated::SemanticKind::Figure)
            )
        })
        .expect("case 34 should include a changed figure block");

    assert_figure_slots_are_patch_surface_paths(figure);
    assert!(
        figure.edits.iter().any(|edit| matches!(
            edit,
            RealizedEdit::ReplaceAt {
                path,
                content: EditContent::Modified { .. },
            } if path == &vec![1]
        )),
        "caption edit should target authored caption path [1]"
    );
    assert!(
        !figure.edits.iter().any(|edit| matches!(
            edit,
            RealizedEdit::ReplaceAt { path, .. } if path == &vec![0, 0, 1]
        )),
        "caption edit must not target realized v/caption scaffolding"
    );
}

#[test]
fn figure_caption_add_delete_pair_by_semantic_owner_not_text() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let added = diff_annotated_corpus("71-figure-caption-added");
    let added_figure = only_changed_figure_block(&added, "71-figure-caption-added");
    assert_figure_slots_are_patch_surface_paths(added_figure);
    assert!(
        added_figure
            .edits
            .iter()
            .all(|edit| !edit_is_whole_block_insert_or_delete(edit)),
        "caption add should not become whole-block delete/insert"
    );
    assert!(matches!(
        added_figure.edits.as_slice(),
        [RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Inserted(_),
        }] if path.as_slice() == [1]
    ));

    let deleted = diff_annotated_corpus("72-figure-caption-deleted");
    let deleted_figure = only_changed_figure_block(&deleted, "72-figure-caption-deleted");
    assert_figure_slots_are_patch_surface_paths(deleted_figure);
    assert!(
        deleted_figure
            .edits
            .iter()
            .all(|edit| !edit_is_whole_block_insert_or_delete(edit)),
        "caption delete should not become whole-block delete/insert"
    );
    assert!(matches!(
        deleted_figure.edits.as_slice(),
        [RealizedEdit::InsertAfter {
            anchor,
            content: EditContent::Deleted(_),
        }] if anchor.as_slice() == [0]
    ));
}

#[test]
fn ownership_noise_does_not_steal_figure_or_duplicate_caption_text() {
    for (case, caption) in [
        ("71-figure-caption-added", "Distribution of measurements"),
        ("72-figure-caption-deleted", "Distribution of measurements"),
        (
            "73-figure-body-changed-caption-added",
            "Updated measurements",
        ),
    ] {
        let result = diff_annotated_corpus(case);
        let figure = only_changed_figure_block(&result, case);
        assert_figure_slots_are_patch_surface_paths(figure);
        assert_edit_paths_resolve_for_base(&figure.base, &figure.edits);

        let rendered = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
        assert_eq!(
            plain_occurrences(&rendered, caption),
            1,
            "{case} should not leave both a plain new caption and a patched caption"
        );
    }
}

#[test]
fn opaque_figure_body_and_caption_change_share_one_owner_block() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("92-diagram-caption-and-opaque-body-changed");
    let figure = only_changed_figure_block(&result, "92-diagram-caption-and-opaque-body-changed");
    assert_figure_slots_are_patch_surface_paths(figure);
    assert_eq!(figure.edits.len(), 2);
    assert!(matches!(
        &figure.edits[0],
        RealizedEdit::ReplaceAt { path, content }
            if path.as_slice() == [0] && edit_content_is_opaque(content)
    ));
    assert!(matches!(
        &figure.edits[1],
        RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Modified { .. },
        } if path.as_slice() == [1]
    ));
}

#[test]
fn text_empty_structural_visual_changes_produce_opaque_replacement() {
    for case in [
        "90-opaque-graphic-replaced",
        "91-raw-svg-graphic-replaced",
        "73-figure-body-changed-caption-added",
        "92-diagram-caption-and-opaque-body-changed",
    ] {
        let result = diff_annotated_corpus(case);
        assert!(
            count_opaque_replacements(&result.blocks) >= 1,
            "{case} should produce an opaque visual replacement"
        );
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
fn slot_paths_resolve_in_changed_blocks_for_representative_corpus_cases() {
    for case in [
        "18-list-item-changed",
        "19-list-item-added",
        "20-nested-list-changed",
        "35-table-changed",
        "64-table-row-inserted-middle",
        "65-table-row-deleted-middle",
        "69-nested-list-item-inserted",
        "87-show-paragraph-wrapper-changed",
    ] {
        let result = diff_annotated_corpus(case);
        for block in result.blocks.iter().filter(|b| !b.edits.is_empty()) {
            for slot in &block.base.annotation.slots {
                assert!(
                    block.base.get_path(&slot.path).is_some(),
                    "slot path should resolve for {case}: {:?} {:?}",
                    slot.label,
                    slot.path
                );
            }
        }
    }
}

#[test]
fn changed_block_edit_paths_resolve_for_representative_corpus_cases() {
    for case in [
        "18-list-item-changed",
        "19-list-item-added",
        "20-nested-list-changed",
        "35-table-changed",
        "64-table-row-inserted-middle",
        "65-table-row-deleted-middle",
        "69-nested-list-item-inserted",
        "87-show-paragraph-wrapper-changed",
    ] {
        let result = diff_annotated_corpus(case);
        for block in result.blocks.iter().filter(|b| !b.edits.is_empty()) {
            assert_edit_paths_resolve_for_base(&block.base, &block.edits);
        }
    }
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
fn modified_heading_does_not_apply_heading_style_twice() {
    let new_world = corpus_world("10-heading-text-changed/new.typ");
    let normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let annotated = annotated_corpus("10-heading-text-changed");

    let normal_document = typst_diff::eval::layout_document(&new_world, &normal).unwrap();
    let annotated_document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let normal_runs = rendered_text_runs(&normal_document.pages[0].frame);
    let annotated_runs = rendered_text_runs(&annotated_document.pages[0].frame);

    let normal_heading_run = normal_runs
        .iter()
        .find(|run| run.text.contains("Heading"))
        .expect("normal heading text should render");

    for needle in ["Old", "New", "Heading"] {
        let annotated_run = annotated_runs
            .iter()
            .find(|run| run.text.contains(needle))
            .unwrap_or_else(|| panic!("{needle} should render in annotated heading"));
        assert!(
            (annotated_run.size - normal_heading_run.size).abs() < 0.1,
            "{needle} should use the normal heading size once; normal={normal_heading_run:?}, annotated={annotated_run:?}"
        );
    }
}

#[test]
fn modified_numbered_headings_do_not_apply_heading_style_twice() {
    let new_world = corpus_world("40-cross-references/new.typ");
    let normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let annotated = annotated_corpus("40-cross-references");

    let normal_document = typst_diff::eval::layout_document(&new_world, &normal).unwrap();
    let annotated_document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let normal_runs = rendered_text_runs(&normal_document.pages[0].frame);
    let annotated_runs = rendered_text_runs(&annotated_document.pages[0].frame);
    let normal_heading_run = normal_runs
        .iter()
        .find(|run| run.text.contains("Methods"))
        .expect("normal numbered heading text should render");

    let oversized_runs: Vec<_> = annotated_runs
        .iter()
        .filter(|run| run.size > normal_heading_run.size + 0.1)
        .cloned()
        .collect();

    assert!(
        oversized_runs.is_empty(),
        "numbered heading diff should not apply heading size twice; normal={normal_heading_run:?}, oversized={oversized_runs:?}"
    );
}

#[test]
fn paragraph_to_heading_context_change_keeps_deleted_text_body_sized() {
    let old_world = corpus_world("36-heading-to-paragraph/old.typ");
    let new_world = corpus_world("36-heading-to-paragraph/new.typ");
    let old_normal = typst_diff::eval_to_realized_content(&old_world)
        .unwrap()
        .realized;
    let new_normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let annotated = annotated_corpus("36-heading-to-paragraph");

    let old_document = typst_diff::eval::layout_document(&old_world, &old_normal).unwrap();
    let new_document = typst_diff::eval::layout_document(&new_world, &new_normal).unwrap();
    let annotated_document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let old_runs = rendered_text_runs(&old_document.pages[0].frame);
    let new_runs = rendered_text_runs(&new_document.pages[0].frame);
    let annotated_runs = rendered_text_runs(&annotated_document.pages[0].frame);

    let body_size = run_size(&old_runs, "Background");
    let heading_size = run_size(&new_runs, "Background");
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let green = typst::visualize::Color::from_u8(0, 180, 0, 255).into();

    let deleted_size = run_size_with_fill(&annotated_runs, "Background", red);
    let inserted_size = run_size_with_fill(&annotated_runs, "Background", green);

    assert!(
        (deleted_size - body_size).abs() < 0.1,
        "deleted paragraph should keep body size; body={body_size}, deleted={deleted_size}, runs={annotated_runs:?}"
    );
    assert!(
        (inserted_size - heading_size).abs() < 0.1,
        "inserted heading should keep heading size; heading={heading_size}, inserted={inserted_size}, runs={annotated_runs:?}"
    );
}

#[test]
fn heading_level_context_change_keeps_deleted_text_at_old_level() {
    let old_world = corpus_world("38-fn-template-changed/old.typ");
    let new_world = corpus_world("38-fn-template-changed/new.typ");
    let old_normal = typst_diff::eval_to_realized_content(&old_world)
        .unwrap()
        .realized;
    let new_normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let annotated = annotated_corpus("38-fn-template-changed");

    let old_document = typst_diff::eval::layout_document(&old_world, &old_normal).unwrap();
    let new_document = typst_diff::eval::layout_document(&new_world, &new_normal).unwrap();
    let annotated_document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let old_runs = rendered_text_runs(&old_document.pages[0].frame);
    let new_runs = rendered_text_runs(&new_document.pages[0].frame);
    let annotated_runs = rendered_text_runs(&annotated_document.pages[0].frame);

    let old_heading_size = run_size(&old_runs, "Overview");
    let new_heading_size = run_size(&new_runs, "Overview");
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let green = typst::visualize::Color::from_u8(0, 180, 0, 255).into();

    let deleted_size = run_size_with_fill(&annotated_runs, "Overview", red);
    let inserted_size = run_size_with_fill(&annotated_runs, "Overview", green);

    assert!(
        (deleted_size - old_heading_size).abs() < 0.1,
        "deleted heading should keep old level; old={old_heading_size}, deleted={deleted_size}, runs={annotated_runs:?}"
    );
    assert!(
        (inserted_size - new_heading_size).abs() < 0.1,
        "inserted heading should keep new level; new={new_heading_size}, inserted={inserted_size}, runs={annotated_runs:?}"
    );
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
fn adjacent_old_new_variants_keep_word_separator_in_corpus_rendering() {
    let cases = [
        (
            "07-paragraphs-reordered",
            "matterentirely",
            "matter entirely",
        ),
        (
            "15-emph-content-changed",
            "literaturemodern",
            "literature modern",
        ),
        ("35-table-changed", "metricsshows", "metrics shows"),
    ];

    for (name, forbidden, expected) in cases {
        let annotated = annotated_corpus(name);
        let plain = annotated.plain_text();

        assert!(
            !plain.contains(forbidden),
            "{name} should not glue adjacent old/new variants:\n{plain}"
        );
        assert!(
            plain.contains(expected),
            "{name} should keep a separator between adjacent old/new variants:\n{plain}"
        );
    }
}

#[test]
fn adjacent_replacement_tokens_keep_separator_in_corpus_rendering() {
    let cases = [
        ("48-state-context", "progress: 2530%", "progress: 25 30%"),
        (
            "100-figure-inserted-before-figure-reference",
            "Figure 12 for",
            "Figure 1 2 for",
        ),
        (
            "36-heading-to-paragraph",
            "level-oneheading",
            "level-one heading",
        ),
    ];

    for (name, forbidden, expected) in cases {
        let annotated = annotated_corpus(name);
        let plain = normalize_whitespace(annotated.plain_text().as_str());

        assert!(
            !plain.contains(forbidden),
            "{name} should not glue adjacent replacement tokens:\n{plain}"
        );
        assert!(
            plain.contains(expected),
            "{name} should keep adjacent replacement tokens readable:\n{plain}"
        );
    }
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
fn cli_writes_debug_bundle_when_requested() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.typ"), "The old text.").unwrap();
    std::fs::write(dir.path().join("new.typ"), "The new text.").unwrap();
    let pdf_path = dir.path().join("changes.pdf");
    let log_path = dir.path().join("mods.txt");
    let debug_dir = pdf_path.with_extension("debug");

    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["old.typ", "new.typ", "-o"])
        .arg(&pdf_path)
        .args(["-l"])
        .arg(&log_path)
        .arg("--debug")
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff --debug");
    assert_valid_pdf(&std::fs::read(&pdf_path).unwrap());

    let yaml_files = [
        "manifest.yml",
        "old/raw-eval.yml",
        "old/normalized.yml",
        "old/realized-tree.yml",
        "old/blocks.yml",
        "new/raw-eval.yml",
        "new/normalized.yml",
        "new/realized-tree.yml",
        "new/blocks.yml",
        "diff/block-raw.yml",
        "diff/block-matched.yml",
        "diff/final-edits.yml",
        "diff/rendered-regions.yml",
        "output/annotated-content.yml",
    ];
    for rel in yaml_files {
        assert_yaml_file(&debug_dir.join(rel));
    }
    assert!(
        !debug_dir.join("diff/pipeline-events.jsonl").exists(),
        "--debug should not write pipeline trace JSONL"
    );
    assert!(
        !debug_dir
            .join("diff/rendered-region-frame-traces.jsonl")
            .exists(),
        "--debug should not write rendered frame trace JSONL"
    );

    let manifest = std::fs::read_to_string(debug_dir.join("manifest.yml")).unwrap();
    assert!(manifest.contains("schema_version: 2"), "{manifest}");
    assert!(manifest.contains("debug_trace: false"), "{manifest}");
    assert!(
        manifest.contains(&typst_diff::build_info::build_report_line()),
        "{manifest}"
    );
    assert!(manifest.contains("old.typ"), "{manifest}");
    assert!(manifest.contains("new.typ"), "{manifest}");
    assert!(manifest.contains("changes.pdf"), "{manifest}");

    let final_edits = std::fs::read_to_string(debug_dir.join("diff/final-edits.yml")).unwrap();
    assert!(final_edits.contains("modified"), "{final_edits}");
    assert!(final_edits.contains("old"), "{final_edits}");
    assert!(final_edits.contains("new"), "{final_edits}");

    let standalone_log = std::fs::read_to_string(log_path).unwrap();
    let debug_log = std::fs::read_to_string(debug_dir.join("output/modification-log.txt")).unwrap();
    assert_eq!(debug_log, standalone_log);
}

#[test]
fn cli_debug_trace_records_pipeline_events_without_frame_trace_for_normal_text() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.typ"), "The old text.").unwrap();
    std::fs::write(dir.path().join("new.typ"), "The new text.").unwrap();
    let pdf_path = dir.path().join("changes.pdf");
    let debug_dir = pdf_path.with_extension("debug");

    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["old.typ", "new.typ", "-o"])
        .arg(&pdf_path)
        .arg("--debug-trace")
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff --debug-trace");
    assert_valid_pdf(&std::fs::read(&pdf_path).unwrap());
    assert_yaml_file(&debug_dir.join("manifest.yml"));
    let trace_path = debug_dir.join("diff/pipeline-events.jsonl");
    assert_jsonl_file(&trace_path);
    assert!(
        !debug_dir
            .join("diff/rendered-region-frame-traces.jsonl")
            .exists(),
        "non-rendered-region diffs should not create a frame trace file"
    );
    let trace = std::fs::read_to_string(trace_path).unwrap();
    assert!(
        trace.contains(r#""format":"typst-diff-pipeline-events""#),
        "{trace}"
    );
    assert!(trace.contains(r#""stage":"diff/edit-zone""#), "{trace}");
    assert!(
        trace.contains(r#""event":"selected_replacement""#),
        "{trace}"
    );
    assert!(trace.contains(r#""stage":"render""#), "{trace}");

    let manifest = std::fs::read_to_string(debug_dir.join("manifest.yml")).unwrap();
    assert!(manifest.contains("debug_trace: true"), "{manifest}");
    assert!(manifest.contains("pipeline-events.jsonl"), "{manifest}");
    assert!(manifest.contains("present: false"), "{manifest}");
}

#[test]
fn cli_debug_trace_records_rendered_region_frame_trace_events() {
    let dir = tempfile::TempDir::new().unwrap();
    let pdf_path = dir.path().join("headers.pdf");
    let debug_dir = pdf_path.with_extension("debug");

    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .arg(corpus("43-alternating-headers/old.typ"))
        .arg(corpus("43-alternating-headers/new.typ"))
        .args(["-o"])
        .arg(&pdf_path)
        .arg("--debug-trace")
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff --debug-trace rendered-region trace");
    assert_valid_pdf(&std::fs::read(&pdf_path).unwrap());
    assert_jsonl_file(&debug_dir.join("diff/pipeline-events.jsonl"));
    let trace_path = debug_dir.join("diff/rendered-region-frame-traces.jsonl");
    assert_jsonl_file(&trace_path);
    let trace = std::fs::read_to_string(trace_path).unwrap();
    assert!(trace.contains(r#""region_kind":"header""#), "{trace}");
    assert!(trace.contains(r#""side":"new""#), "{trace}");
    assert!(trace.contains(r#""text":"Final Version""#), "{trace}");
    assert!(trace.contains(r#""text":"New Report""#), "{trace}");
    assert!(
        trace.lines().any(|line| {
            line.contains(r#""trace_id":"new/header/page-1""#)
                && line.contains(r#""text":"Final Version""#)
                && line.contains(r#""included":true"#)
                && line.contains(r#""artifact_depth_before":1"#)
                && line.contains(r#""artifact_depth_after":1"#)
        }),
        "{trace}"
    );
    assert!(trace.contains(r#""tag_element":"artifact""#), "{trace}");
    assert!(trace.contains(r#""tag_element":"context""#), "{trace}");
    assert!(trace.contains(r#""tag_element":"emph""#), "{trace}");
    assert!(trace.contains(r#""artifact_depth_before":"#), "{trace}");
    assert!(trace.contains(r#""artifact_depth_after":"#), "{trace}");
    assert!(trace.contains(r#""included":true"#), "{trace}");
}

#[test]
fn rendered_region_debug_events_do_not_change_diff_result() {
    struct CountingSink {
        events: usize,
    }

    impl typst_diff::trace::DebugEventSink for CountingSink {
        fn rendered_region_trace_event(
            &mut self,
            _event: &typst_diff::trace::FrameTraceEvent,
        ) -> anyhow::Result<()> {
            self.events += 1;
            Ok(())
        }
    }

    let old_world = corpus_world("43-alternating-headers/old.typ");
    let new_world = corpus_world("43-alternating-headers/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();

    let normal =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();
    let mut sink = CountingSink { events: 0 };
    let (with_debug, _) = typst_diff::diff::diff_annotated_with_rendered_regions_and_debug_events(
        &old, &new, &old_world, &new_world, &mut sink,
    )
    .unwrap();

    assert!(
        sink.events > 0,
        "expected debug sink to observe frame events"
    );
    assert_eq!(normal.modification_log(), with_debug.modification_log());
}

#[derive(Default)]
struct RecordingSink {
    pipeline: Vec<typst_diff::trace::PipelineTraceEvent>,
}

impl typst_diff::trace::DebugEventSink for RecordingSink {
    fn pipeline_trace_event(
        &mut self,
        event: &typst_diff::trace::PipelineTraceEvent,
    ) -> anyhow::Result<()> {
        self.pipeline.push(event.clone());
        Ok(())
    }
}

#[test]
fn pipeline_trace_does_not_change_diff_result_for_normal_text() {
    let (_dir, old_world, new_world) = temp_worlds("The old text.", "The new text.");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();

    let normal =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();
    let mut sink = RecordingSink::default();
    let (traced, _) = typst_diff::diff::diff_annotated_with_rendered_regions_and_debug_events(
        &old, &new, &old_world, &new_world, &mut sink,
    )
    .unwrap();

    assert_eq!(normal.modification_log(), traced.modification_log());
    assert!(
        sink.pipeline
            .iter()
            .any(|event| event.stage == "diff/edit-zone"
                && event.event == "selected_replacement"
                && event.selected_edit_kind.as_deref() == Some("replace")),
        "expected edit-zone replacement event"
    );
    assert!(
        sink.pipeline
            .iter()
            .any(|event| event.stage == "diff/replace-block"
                && event.selected_edit_kind.as_deref() == Some("modified")),
        "expected modified replacement decision"
    );
}

#[test]
fn traced_and_untraced_edit_zone_matching_are_identical() {
    let old = vec![
        typst_diff::eval::eval_snippet_to_content("alpha").unwrap(),
        typst_diff::eval::eval_snippet_to_content("bravo old").unwrap(),
    ];
    let new = vec![
        typst_diff::eval::eval_snippet_to_content("alpha").unwrap(),
        typst_diff::eval::eval_snippet_to_content("bravo new").unwrap(),
    ];
    let raw = typst_diff::diff::diff_blocks_raw(&old, &new);
    let untraced = typst_diff::diff::match_edit_zones(raw.clone());
    let mut sink = RecordingSink::default();
    let traced = typst_diff::diff::match_edit_zones_with_debug_events(raw, &mut sink).unwrap();

    assert_eq!(block_op_signatures(&untraced), block_op_signatures(&traced));
    assert!(
        sink.pipeline
            .iter()
            .any(|event| event.event == "similarity_candidate"
                && event.similarity.is_some()
                && event.threshold == Some(0.3)),
        "expected similarity candidate trace"
    );
}

#[test]
fn pipeline_trace_records_opaque_replacement_decision() {
    let old_world = corpus_world("90-opaque-graphic-replaced/old.typ");
    let new_world = corpus_world("90-opaque-graphic-replaced/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let mut sink = RecordingSink::default();

    typst_diff::diff::diff_annotated_with_block_debug_events(&old, &new, &mut sink).unwrap();

    assert!(
        sink.pipeline.iter().any(|event| {
            event
                .selected_edit_kind
                .as_deref()
                .is_some_and(|kind| kind.contains("opaque_replacement"))
        }),
        "expected opaque replacement trace event"
    );
}

#[test]
fn pipeline_trace_records_slot_recursion_decision() {
    let old_world = corpus_world("18-list-item-changed/old.typ");
    let new_world = corpus_world("18-list-item-changed/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let mut sink = RecordingSink::default();

    typst_diff::diff::diff_annotated_with_block_debug_events(&old, &new, &mut sink).unwrap();

    assert!(
        sink.pipeline
            .iter()
            .any(|event| event.stage == "diff/slot" && event.event == "start"),
        "expected slot recursion trace event"
    );
}

#[test]
fn pipeline_trace_records_rendered_region_skip_for_semantic_region_diff() {
    let old_world = corpus_world("80-footer-text-changed/old.typ");
    let new_world = corpus_world("80-footer-text-changed/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let mut sink = RecordingSink::default();

    typst_diff::diff::diff_annotated_with_rendered_regions_and_debug_events(
        &old, &new, &old_world, &new_world, &mut sink,
    )
    .unwrap();

    assert!(
        sink.pipeline.iter().any(|event| {
            event.stage == "diff/rendered-region"
                && event.event == "skip"
                && event
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("semantic page-region diff"))
        }),
        "expected semantic page-region skip trace event"
    );
}

fn block_op_signatures(blocks: &[typst_diff::diff::BlockOp]) -> Vec<String> {
    blocks
        .iter()
        .map(|op| match op {
            typst_diff::diff::BlockOp::Equal(old, new) => {
                format!(
                    "equal:{}=>{}",
                    old.content.plain_text(),
                    new.content.plain_text()
                )
            }
            typst_diff::diff::BlockOp::Delete(old) => {
                format!("delete:{}", old.content.plain_text())
            }
            typst_diff::diff::BlockOp::Insert(new) => {
                format!("insert:{}", new.content.plain_text())
            }
            typst_diff::diff::BlockOp::Replace(old, new) => {
                format!(
                    "replace:{}=>{}",
                    old.content.plain_text(),
                    new.content.plain_text()
                )
            }
        })
        .collect()
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

#[test]
fn cli_debug_manifest_records_revision_mode() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.typ"), "The old text.").unwrap();

    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Typst Diff Test"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "old"]);

    std::fs::write(dir.path().join("main.typ"), "The new text.").unwrap();
    let output = dir.path().join("revision.pdf");
    let cli = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["main.typ", "--revision", "HEAD", "-o"])
        .arg(&output)
        .arg("--debug")
        .output()
        .unwrap();

    assert_command_success(cli, "typst-diff --debug --revision");
    assert_valid_pdf(&std::fs::read(&output).unwrap());
    let manifest_path = output.with_extension("debug").join("manifest.yml");
    assert_yaml_file(&manifest_path);
    let manifest = std::fs::read_to_string(manifest_path).unwrap();
    assert!(manifest.contains("revision: HEAD"), "{manifest}");
    assert!(manifest.contains("main.typ"), "{manifest}");
    assert!(manifest.contains("revision.pdf"), "{manifest}");
}

fn assert_yaml_file(path: &std::path::Path) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| {
        panic!("failed to read YAML file {}: {err}", path.display());
    });
    serde_yaml::from_str::<serde_yaml::Value>(&text).unwrap_or_else(|err| {
        panic!(
            "failed to parse YAML file {}: {err}\n{text}",
            path.display()
        );
    });
}

fn assert_jsonl_file(path: &std::path::Path) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| {
        panic!("failed to read JSONL file {}: {err}", path.display());
    });
    assert!(
        !text.trim().is_empty(),
        "JSONL file is empty: {}",
        path.display()
    );
    for (line_index, line) in text.lines().enumerate() {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|err| {
            panic!(
                "failed to parse JSONL file {} line {}: {err}\n{line}",
                path.display(),
                line_index + 1
            );
        });
    }
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
        plain.contains("old") && plain.contains("mammals"),
        "rendered annotated content omitted deleted nested list words: {plain:?}"
    );
    assert!(
        plain.contains("updated") && plain.contains("warm-blooded vertebrates"),
        "rendered annotated content omitted inserted nested list words: {plain:?}"
    );
}

#[test]
fn nested_list_item_change_tree_render_styles_old_and_new_text() {
    let annotated = annotated_tree_corpus("20-nested-list-changed");

    assert!(
        text_is_struck(&annotated, "old") && text_is_struck(&annotated, "mammals"),
        "deleted nested list words should be struck in rendered annotated content"
    );
    assert!(
        text_has_any_style(&annotated, "updated")
            && text_has_any_style(&annotated, "warm-blooded vertebrates"),
        "inserted nested list words should be styled in rendered annotated content"
    );
}

#[test]
fn nested_list_item_change_preserves_nested_list_layout() {
    let result = diff_annotated_corpus("20-nested-list-changed");
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);

    assert_eq!(
        count_nodes::<typst::model::ListElem>(&annotated),
        3,
        "outer kingdom list, phylum sublist, and class sublist should all survive"
    );

    let new_world = corpus_world("20-nested-list-changed/new.typ");
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let runs = rendered_text_runs(&document.pages[0].frame);
    let phylum = runs
        .iter()
        .find(|run| run.text.contains("Phylum: Chordata"))
        .expect("Phylum item should render");
    let class = runs
        .iter()
        .find(|run| run.text.contains("Class: Mammalia"))
        .expect("Class item should render");
    let arthropoda = runs
        .iter()
        .find(|run| run.text.contains("Phylum: Arthropoda"))
        .expect("following Phylum item should render");

    assert!(
        class.y > phylum.y + phylum.size * 0.5,
        "Class item should render below Phylum item; phylum={phylum:?}, class={class:?}"
    );
    assert!(
        class.x > phylum.x + 5.0,
        "Class item should be indented under Phylum item; phylum={phylum:?}, class={class:?}"
    );

    let normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let normal_document = typst_diff::eval::layout_document(&new_world, &normal).unwrap();
    let normal_runs = rendered_text_runs(&normal_document.pages[0].frame);
    let normal_class = normal_runs
        .iter()
        .find(|run| run.text.contains("Class: Mammalia"))
        .expect("normal Class item should render");
    let normal_arthropoda = normal_runs
        .iter()
        .find(|run| run.text.contains("Phylum: Arthropoda"))
        .expect("normal following Phylum item should render");
    let annotated_gap = arthropoda.y - class.y;
    let normal_gap = normal_arthropoda.y - normal_class.y;

    assert!(
        annotated_gap <= normal_gap + 0.5,
        "modified class item should keep tight nested-list spacing; annotated_gap={annotated_gap}, normal_gap={normal_gap}, class={class:?}, arthropoda={arthropoda:?}, normal_class={normal_class:?}, normal_arthropoda={normal_arthropoda:?}"
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
fn opaque_wrapper_changes_are_reported_once() {
    for name in [
        "54-align-changed",
        "55-pad-changed",
        "57-columns-changed",
        "58-stack-changed",
        "60-rect-changed",
        "61-circle-changed",
        "62-ellipse-changed",
        "93-math-diagram-plot-changed",
        "56-place-changed",
    ] {
        let result = diff_annotated_corpus(name);
        let log = result.modification_log();
        let modifications = log.lines().filter(|line| line.starts_with("## ")).count();
        assert_eq!(
            modifications, 1,
            "unexpected modifications for {name}:\n{log}"
        );
        assert!(log.contains("Old"), "{name} log missing old text:\n{log}");
        assert!(log.contains("New"), "{name} log missing new text:\n{log}");
    }
}

#[test]
fn opaque_wrapper_change_renders_only_the_annotated_replacement() {
    let cases = [
        (
            "54-align-changed",
            Some("Old New centered announcement for the review board."),
            "New centered announcement for the review board.",
        ),
        (
            "55-pad-changed",
            Some("Old New padded note for readers."),
            "New padded note for readers.",
        ),
        (
            "57-columns-changed",
            Some("Old New first column text for comparison."),
            "New first column text for comparison.",
        ),
        (
            "56-place-changed",
            Some("Old New placed label"),
            "New placed label",
        ),
        (
            "58-stack-changed",
            Some("Old New stacked item"),
            "New stacked item",
        ),
        (
            "60-rect-changed",
            Some("Old New rectangle label"),
            "New rectangle label",
        ),
        (
            "61-circle-changed",
            Some("Old New circle label"),
            "New circle label",
        ),
        (
            "62-ellipse-changed",
            Some("Old New ellipse label"),
            "New ellipse label",
        ),
        (
            "93-math-diagram-plot-changed",
            Some("Old New trend"),
            "New trend",
        ),
        (
            "103-repeated-macro-containers-with-one-edit",
            None,
            "approval",
        ),
        ("105-paragraph-split-inside-wrapper", None, "It also has"),
    ];

    for (name, modified_surface, new_surface) in cases {
        let annotated = annotated_tree_corpus(name);
        let text = annotated.plain_text();

        if let Some(modified_surface) = modified_surface {
            assert_eq!(
                text.matches(modified_surface).count(),
                1,
                "{name} should render the word-level replacement surface exactly once:\n{text}"
            );
            let new_world = corpus_world(&format!("{name}/new.typ"));
            let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
            let rendered_text = rendered_text_runs(&document.pages[0].frame)
                .into_iter()
                .map(|run| run.text)
                .collect::<Vec<_>>()
                .join("");
            assert_eq!(
                rendered_text.matches(modified_surface).count(),
                1,
                "{name} should render the word-level replacement surface exactly once in the laid-out frame:\n{rendered_text}"
            );
            assert_eq!(
                rendered_text.matches(new_surface).count(),
                1,
                "{name} should not render both an annotated replacement and a plain new copy in the laid-out frame:\n{rendered_text}"
            );
        } else {
            assert_eq!(
                text.matches(new_surface).count(),
                1,
                "{name} should not render both a plain new copy and a diff copy:\n{text}"
            );
        }
    }
}

#[test]
fn repeated_macro_container_edit_is_reported_on_the_changed_instance() {
    let result = diff_annotated_corpus("103-repeated-macro-containers-with-one-edit");
    let log = result.modification_log();
    let modifications = log.lines().filter(|line| line.starts_with("## ")).count();

    assert_eq!(modifications, 1, "{log}");
    assert!(log.contains("deleted: data"), "{log}");
    assert!(log.contains("inserted: approval"), "{log}");
    assert!(!log.contains("Alpha"), "{log}");
    assert!(!log.contains("Gamma"), "{log}");
}

#[test]
fn paragraph_split_inside_wrapper_keeps_shared_prefix_localized() {
    let log = diff_annotated_corpus("105-paragraph-split-inside-wrapper").modification_log();

    assert!(log.contains("deleted: and"), "{log}");
    assert!(log.contains("inserted: .It also has"), "{log}");
    assert!(!log.contains("deleted: summary"), "{log}");
    assert!(!log.contains("inserted: summary"), "{log}");
}

#[test]
fn semantic_container_changes_are_word_localized() {
    let cases = [
        (
            "52-block-changed",
            ["deleted: Old | a manual", "inserted: New | an automated"],
            [
                "deleted: Old block content describes a manual workflow.",
                "inserted: New block content describes an automated workflow.",
            ],
        ),
        (
            "53-box-changed",
            ["deleted: old pending", "inserted: new approved"],
            ["deleted: old pending label", "inserted: new approved label"],
        ),
        (
            "87-show-paragraph-wrapper-changed",
            ["deleted: old", "inserted: new"],
            [
                "deleted: This wrapped paragraph mentions the old schedule.",
                "inserted: This wrapped paragraph mentions the new schedule.",
            ],
        ),
    ];

    for (name, expected, forbidden) in cases {
        let log = diff_annotated_corpus(name).modification_log();
        for needle in expected {
            assert!(
                log.contains(needle),
                "{name} log missing {needle:?}:\n{log}"
            );
        }
        for needle in forbidden {
            assert!(
                !log.contains(needle),
                "{name} log should not contain whole-container edit {needle:?}:\n{log}"
            );
        }
    }
}

#[test]
fn nearby_footnote_insert_treats_ambiguous_existing_note_as_delete_insert() {
    let log = diff_annotated_corpus("74-footnote-added-near-existing-footnote").modification_log();

    assert!(log.contains("inserted: It remains reproducible."), "{log}");
    assert!(
        log.contains("inserted: New note explains calibration."),
        "{log}"
    );
    assert!(
        log.contains("inserted: Existing note mentions revised settings."),
        "{log}"
    );
    assert!(
        log.contains("deleted: Existing note mentions baseline settings."),
        "{log}"
    );
}

#[test]
fn footnote_body_change_preserves_marker_paragraph_regression() {
    let old = r#"The API remains stable#footnote[Old footnote guidance for deployers.].

The rest of the paragraph is unchanged.
"#;
    let new = r#"The API remains stable#footnote[New footnote guidance for operators.].

The rest of the paragraph is unchanged.
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);

    assert_modified_words_include(&result, &["Old", "deployers"], &["New", "operators"]);
    let (body, footer, _body_runs, _footer_runs) =
        rendered_main_body_and_footer_text(&annotated, &new_world);

    assert_contains_in_order(
        &body,
        &[
            "The API remains stable",
            "1",
            ".",
            "The rest of the paragraph is unchanged.",
        ],
    );
    assert!(
        !body.contains("footnote guidance"),
        "footnote body should stay in the footer, not the main body:\nbody={body}\nfooter={footer}"
    );
    assert!(footer.contains("Old"), "footer={footer}");
    assert!(footer.contains("New"), "footer={footer}");
    assert!(footer.contains("deployers"), "footer={footer}");
    assert!(footer.contains("operators"), "footer={footer}");
}

#[test]
fn nearby_inserted_footnote_keeps_bodies_in_footer_without_ambiguous_pairing_regression() {
    let old = r#"The method is stable.#footnote[Existing note mentions baseline settings.]

The conclusion follows from the evaluation.
"#;
    let new = r#"The method is stable.#footnote[New note explains calibration.] It remains reproducible.#footnote[Existing note mentions revised settings.]

The conclusion follows from the evaluation.
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let log = result.modification_log();

    assert!(log.contains("inserted: It remains reproducible."), "{log}");
    assert!(
        log.contains("inserted: New note explains calibration."),
        "{log}"
    );
    assert!(
        log.contains("inserted: Existing note mentions revised settings."),
        "{log}"
    );
    assert!(
        log.contains("deleted: Existing note mentions baseline settings."),
        "{log}"
    );

    let (body, footer, _body_runs, _footer_runs) =
        rendered_main_body_and_footer_text(&annotated, &new_world);
    assert_contains_in_order(
        &body,
        &[
            "The method is stable.",
            "1",
            "It remains reproducible.",
            "2",
            "3",
            "The conclusion follows from the evaluation.",
        ],
    );
    assert!(
        !body.contains("Existing note mentions"),
        "existing footnote body should not become a standalone main-body paragraph:\nbody={body}\nfooter={footer}"
    );
    assert!(
        footer.contains("New note explains calibration."),
        "newly inserted footnote body should render in footer:\nbody={body}\nfooter={footer}"
    );
    assert!(footer.contains("Existing note mentions"), "footer={footer}");
    assert!(footer.contains("baseline"), "footer={footer}");
    assert!(footer.contains("revised"), "footer={footer}");
}

#[test]
fn inline_text_to_footnote_body_regression() {
    let old = "The procedure keeps the calibration note inline.\n";
    let new = "The procedure keeps the#footnote[calibration note] inline.\n";
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let log = result.modification_log();

    assert!(log.contains("text: calibration note"), "{log}");
    assert!(log.contains("inserted: calibration note"), "{log}");
    let (body, footer, _body_runs, _footer_runs) =
        rendered_main_body_and_footer_text(&annotated, &new_world);

    assert_contains_in_order(
        &body,
        &[
            "The procedure keeps the",
            "calibration note",
            "1",
            "inline.",
        ],
    );
    assert!(text_is_struck(&annotated, "calibration"));
    assert!(text_is_struck(&annotated, "note"));
    assert!(footer.contains("calibration note"), "footer={footer}");
}

#[test]
fn footnote_body_to_inline_text_regression() {
    let old = "The procedure keeps the#footnote[calibration note] inline.\n";
    let new = "The procedure keeps the calibration note inline.\n";
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);

    assert_modified_words_include(&result, &["calibration note"], &["calibration note"]);
    let (body, footer, _body_runs, _footer_runs) =
        rendered_main_body_and_footer_text(&annotated, &new_world);

    assert_contains_in_order(&body, &["The procedure keeps the calibration note inline."]);
    assert!(
        footer.contains("calibration note"),
        "deleted old footnote body should remain visible in the footer as a deletion:\nbody={body}\nfooter={footer}"
    );
}

#[test]
fn visible_number_before_footnote_marker_is_not_marker() {
    let old = "Step 1#footnote[Old note for deployers.] remains stable.\n";
    let new = "Step 1#footnote[New note for operators.] remains stable.\n";
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);

    assert_modified_words_include(&result, &["Old", "deployers"], &["New", "operators"]);
    let (body, footer, _body_runs, _footer_runs) =
        rendered_main_body_and_footer_text(&annotated, &new_world);
    assert_contains_in_order(&body, &["Step", "1", "1", "remains stable."]);
    assert!(
        !body.contains("note for"),
        "the ordinary visible 1 must not absorb footnote metadata:\nbody={body}\nfooter={footer}"
    );
    assert!(footer.contains("Old"), "footer={footer}");
    assert!(footer.contains("New"), "footer={footer}");
}

#[test]
fn section_references_stay_on_realized_text_path_regression() {
    let old = r#"#set heading(numbering: "1.")

= API <api>

See @api for the old contract.
"#;
    let new = r#"#set heading(numbering: "1.")

= API <api>

See @api for the new contract.
"#;
    let (_dir, result, annotated) = diff_temp_sources(old, new);
    let plain = annotated.plain_text();

    assert_modified_words_include(&result, &["old"], &["new"]);
    assert!(
        plain.contains("Section"),
        "section reference should remain realized inline text:\n{plain}"
    );
    assert!(
        !plain.contains("Footnote"),
        "section references should not be routed through footnote handling:\n{plain}"
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
    assert!(footer_text.contains("Page 1 of 2 3"), "{footer_text}");
    assert!(
        footer_runs.iter().any(|run| {
            run.text == "2" && run.fill == typst::visualize::Color::from_u8(220, 0, 0, 255).into()
        }),
        "expected deleted page total to render red in footer"
    );
    assert!(
        footer_runs.iter().any(|run| {
            run.text.contains('3')
                && run.fill == typst::visualize::Color::from_u8(0, 180, 0, 255).into()
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
    let header = result
        .rendered_regions
        .iter()
        .find(|region| region.kind == PageRegionKind::Header)
        .unwrap();
    assert_eq!(header.pages.len(), 3);
    for page in &header.pages {
        assert_eq!(
            page.segments.len(),
            2,
            "expected split left/right header segments on page {}",
            page.page
        );
    }
    assert_eq!(
        header.pages[0].segments[0].base.plain_text().as_str(),
        "New Report"
    );
    assert_eq!(
        header.pages[0].segments[1].base.plain_text().as_str(),
        "Final Version"
    );
    assert_eq!(
        header.pages[1].segments[0].base.plain_text().as_str(),
        "Final Version"
    );
    assert_eq!(
        header.pages[1].segments[1].base.plain_text().as_str(),
        "New Report"
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
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    assert_alternating_header_page_layout(&document.pages[0].frame, true);
    assert_alternating_header_page_layout(&document.pages[1].frame, false);
    assert_alternating_header_page_layout(&document.pages[2].frame, true);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

fn assert_alternating_header_page_layout(frame: &typst::layout::Frame, odd_page: bool) {
    let page_width = frame.width().to_pt();
    let page_height = frame.height().to_pt();
    let header_runs = rendered_text_runs(frame)
        .into_iter()
        .filter(|run| run.y < page_height * 0.2)
        .collect::<Vec<_>>();
    let header_text = header_runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>();
    for expected in ["Old", "New", "Report", "Draft", "Final", "Version"] {
        assert!(
            header_text.contains(expected),
            "missing {expected:?} in header text {header_text:?}"
        );
    }
    assert!(
        header_runs.iter().any(|run| run.x < page_width * 0.25),
        "expected left-side header runs: {header_runs:?}"
    );
    assert!(
        header_runs
            .iter()
            .any(|run| run.x + run.width > page_width * 0.75),
        "expected right-side header runs: {header_runs:?}"
    );

    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let green = typst::visualize::Color::from_u8(0, 180, 0, 255).into();
    assert!(
        header_runs
            .iter()
            .any(|run| run.text.contains("Old") && run.fill == red),
        "expected deleted Old in red: {header_runs:?}"
    );
    assert!(
        header_runs
            .iter()
            .any(|run| run.text.contains("Draft") && run.fill == red),
        "expected deleted Draft in red: {header_runs:?}"
    );
    assert!(
        header_runs
            .iter()
            .any(|run| run.text.contains("New") && run.fill == green),
        "expected inserted New in green: {header_runs:?}"
    );
    assert!(
        header_runs
            .iter()
            .any(|run| run.text.contains("Final") && run.fill == green),
        "expected inserted Final in green: {header_runs:?}"
    );

    let left_text = header_runs
        .iter()
        .filter(|run| run.x < page_width / 2.0)
        .map(|run| run.text.as_str())
        .collect::<String>();
    let right_text = header_runs
        .iter()
        .filter(|run| run.x >= page_width / 2.0)
        .map(|run| run.text.as_str())
        .collect::<String>();
    if odd_page {
        assert!(left_text.contains("Report"), "{left_text}");
        assert!(right_text.contains("Version"), "{right_text}");
    } else {
        assert!(left_text.contains("Version"), "{left_text}");
        assert!(right_text.contains("Report"), "{right_text}");
    }
}

#[test]
fn header_footer_add_delete_change_are_page_region_edits() {
    use typst_diff::diff::{EditContent, PageRegionKind, RegionPath};

    fn has_region(name: &str, kind: PageRegionKind, matches_content: fn(&EditContent) -> bool) {
        let result = diff_annotated_corpus(name);
        let region = result
            .regions
            .iter()
            .find(|region| region.path == RegionPath::RootPage(kind))
            .unwrap_or_else(|| panic!("expected {kind:?} region for {name}"));
        assert!(
            region
                .edits
                .iter()
                .any(|edit| realized_edit_content_or_nested_matches(edit, matches_content)),
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
    use typst_diff::diff::{PageRegionKind, RealizedEdit, RegionPath};

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
        header
            .edits
            .iter()
            .any(realized_edit_contains_replace_at_modified),
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
    assert_modified_words_include(
        &result,
        &["vertices", "tree", "connected"],
        &["nodes", "forest", "collection", "disjoint"],
    );
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        !deleted.contains("Definition 2"),
        "Definition 2 should keep shared prefix equal; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        !inserted.contains("Definition 2"),
        "Definition 2 should keep shared prefix equal; deleted={deleted:?}; inserted={inserted:?}"
    );

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
    assert!(
        text_has_any_style(&annotated, "forest"),
        "inserted Definition 2 word should be styled in rendered annotated content: {annotated_plain:?}"
    );
    assert!(
        text_is_struck(&annotated, "tree"),
        "deleted Definition 2 word should be struck in rendered annotated content: {annotated_plain:?}"
    );
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
fn block_prefix_then_body_replacement_keeps_inserted_body() {
    let (_dir, result, annotated) = diff_temp_sources(
        "#block[*Label* -- old body]\n",
        "#block[*Label* -- new body]\n",
    );

    assert_modified_words_include(&result, &["old"], &["new"]);
    assert!(text_has_any_style(&annotated, "new"));
    assert!(text_is_struck(&annotated, "old"));
}

#[test]
fn block_body_then_suffix_replacement_keeps_both_sides() {
    let (_dir, result, annotated) = diff_temp_sources(
        "#block[old body -- *Label*]\n",
        "#block[new body -- *Label*]\n",
    );

    assert_modified_words_include(&result, &["old"], &["new"]);
    assert!(text_has_any_style(&annotated, "new"));
    assert!(text_is_struck(&annotated, "old"));
}

#[test]
fn nested_wrapper_body_replacement_keeps_both_sides() {
    let (_dir, result, annotated) = diff_temp_sources(
        "#block[#block[*Definition* -- old text]]\n",
        "#block[#block[*Definition* -- new text]]\n",
    );

    assert_modified_words_include(&result, &["old"], &["new"]);
    assert!(text_has_any_style(&annotated, "new"));
    assert!(text_is_struck(&annotated, "old"));
}

#[test]
fn repeated_macro_wrappers_keep_changed_instance_insertions() {
    let old = r#"
#let card(title, body) = block(stroke: 0.5pt, inset: 6pt)[
  *#title*

  #body
]

#card("Alpha", [Ready for review.])

#card("Beta", [Waiting for data.])

#card("Gamma", [Scheduled for next week.])
"#;
    let new = r#"
#let card(title, body) = block(stroke: 0.5pt, inset: 6pt)[
  *#title*

  #body
]

#card("Alpha", [Ready for review.])

#card("Beta", [Waiting for approval.])

#card("Gamma", [Scheduled for next week.])
"#;
    let (_dir, result, annotated) = diff_temp_sources(old, new);
    let log = result.modification_log();

    assert_modified_words_include(&result, &["data"], &["approval"]);
    assert!(!log.contains("Alpha"), "{log}");
    assert!(!log.contains("Gamma"), "{log}");
    assert!(text_has_any_style(&annotated, "approval"));
    assert!(text_is_struck(&annotated, "data"));
}

#[test]
fn wrapper_second_paragraph_replacement_keeps_inserted_text() {
    let old = r#"
#block[
  *Label*

  First paragraph remains stable.

  Old second paragraph explains the baseline.
]
"#;
    let new = r#"
#block[
  *Label*

  First paragraph remains stable.

  New second paragraph explains the revision.
]
"#;
    let (_dir, result, annotated) = diff_temp_sources(old, new);

    assert_modified_words_include(&result, &["Old", "baseline"], &["New", "revision"]);
    assert!(text_has_any_style(&annotated, "revision"));
    assert!(text_is_struck(&annotated, "baseline"));
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
