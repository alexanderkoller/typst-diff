use std::path::PathBuf;
use std::process::Command;

use typst::foundations::Content;
use typst::model::RefElem;

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
    font_family: String,
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
                    font_family: text.font.info().family.clone(),
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

fn reference_targets(content: &Content) -> std::collections::BTreeMap<String, usize> {
    let mut targets = std::collections::BTreeMap::new();
    let _ = content.traverse::<_, ()>(&mut |c| {
        if let Some(reference) = c.to_packed::<RefElem>() {
            *targets
                .entry(reference.target.resolve().to_string())
                .or_insert(0) += 1;
        }
        std::ops::ControlFlow::Continue(())
    });
    targets
}

fn assert_live_introspection_matches_new(
    new_world: &typst_diff::world::SystemWorld,
    annotated: &Content,
    label: &str,
) {
    let new_content = typst_diff::eval_to_realized_content(new_world).unwrap();
    let annotated_tags = count_nodes::<typst::introspection::TagElem>(annotated);
    let new_tags = count_nodes::<typst::introspection::TagElem>(&new_content.realized);
    assert!(
        annotated_tags <= new_tags,
        "{label}: annotated document should not add old-side labels/tags; annotated={annotated_tags}, new={new_tags}"
    );
    assert_eq!(
        count_nodes::<typst::introspection::StateUpdateElem>(annotated),
        count_nodes::<typst::introspection::StateUpdateElem>(&new_content.realized),
        "{label}: annotated document should keep exactly the new-side state updates"
    );
    assert_eq!(
        reference_targets(annotated),
        reference_targets(&new_content.realized),
        "{label}: annotated document should keep exactly the new-side live reference targets"
    );
    let annotated_contexts = count_nodes::<typst::foundations::ContextElem>(annotated);
    let new_contexts = count_nodes::<typst::foundations::ContextElem>(&new_content.realized);
    assert!(
        annotated_contexts <= new_contexts,
        "{label}: annotated document should not add old-side context expressions; annotated={annotated_contexts}, new={new_contexts}"
    );
}

fn rendered_document_text(content: &Content, world: &typst_diff::world::SystemWorld) -> String {
    let document = typst_diff::eval::layout_document(world, content).unwrap();
    normalize_whitespace(
        &document
            .pages
            .iter()
            .flat_map(|page| rendered_text_runs(&page.frame))
            .map(|run| run.text)
            .collect::<String>(),
    )
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

fn diff_annotated_corpus_with_rendered_regions(name: &str) -> typst_diff::diff::DiffResult {
    let old_world = corpus_world(&format!("{name}/old.typ"));
    let new_world = corpus_world(&format!("{name}/new.typ"));
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
        .unwrap()
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

fn struck_texts(content: &Content) -> Vec<String> {
    let mut out = Vec::new();
    let _ = content.traverse::<_, ()>(&mut |c| {
        if let Some(strike) = c.to_packed::<typst::text::StrikeElem>() {
            out.push(strike.body.plain_text().to_string());
        }
        std::ops::ControlFlow::Continue(())
    });
    out
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
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => {
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
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => walk_content(content, deleted, inserted),
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
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => walk_content(content, bases),
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

fn collect_replace_at_modified_paths_and_bases(
    blocks: &[typst_diff::diff::DiffBlockEdit],
) -> Vec<(Vec<usize>, String)> {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk_content(prefix: &[usize], content: &EditContent, out: &mut Vec<(Vec<usize>, String)>) {
        match content {
            EditContent::Modified { base, .. } => {
                out.push((prefix.to_vec(), base.plain_text().to_string()));
            }
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(prefix, edit, out);
                }
            }
            EditContent::Inserted(_)
            | EditContent::Deleted(_)
            | EditContent::OpaqueReplacement { .. } => {}
        }
    }

    fn walk_edit(prefix: &[usize], edit: &RealizedEdit, out: &mut Vec<(Vec<usize>, String)>) {
        match edit {
            RealizedEdit::ReplaceAt { path, content } => {
                let mut full_path = prefix.to_vec();
                full_path.extend(path.iter().copied());
                walk_content(&full_path, content, out);
            }
            RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => {
                walk_content(prefix, content, out);
            }
        }
    }

    let mut out = Vec::new();
    for block in blocks {
        for edit in &block.edits {
            walk_edit(&[], edit, &mut out);
        }
    }
    out
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
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => {
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
            RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => {
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
        | RealizedEdit::WholeBlock(content)
        | RealizedEdit::LogOnly(content)
        | RealizedEdit::MarkBaseInserted(content) => {
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
        | RealizedEdit::WholeBlock(content)
        | RealizedEdit::LogOnly(content)
        | RealizedEdit::MarkBaseInserted(content) => {
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
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    let new_world = corpus_world("101-equation-number-reference-changed/new.typ");
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let text = rendered_document_text(&annotated, &new_world);
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let green = typst::visualize::Color::from_u8(0, 180, 0, 255).into();
    let green_text = rendered_text_runs(&document.pages[0].frame)
        .into_iter()
        .filter(|run| run.fill == green)
        .map(|run| run.text)
        .collect::<String>();

    assert!(inserted.contains('x'), "inserted={inserted:?}\n{log}");
    assert!(inserted.contains('y'), "inserted={inserted:?}\n{log}");
    assert!(deleted.contains('1'), "deleted={deleted:?}\n{log}");
    assert!(inserted.contains('2'), "inserted={inserted:?}\n{log}");
    assert!(inserted.contains("revised"), "inserted={inserted:?}\n{log}");
    assert!(log.contains("inserted:"), "{log}");
    assert!(log.contains('x'), "{log}");
    assert!(log.contains('y'), "{log}");
    assert!(!log.contains("text: \n"), "{log}");
    assert_live_introspection_matches_new(&new_world, &annotated, "numbered equation insertion");
    assert!(
        text_is_struck(&annotated, "1"),
        "old reference number should be visibly marked as deleted/asserted:\n{}",
        struck_texts(&annotated).join(" | ")
    );
    assert!(
        (green_text.contains('x') || green_text.contains('𝑥'))
            && (green_text.contains('y') || green_text.contains('𝑦'))
            && green_text.contains('1'),
        "inserted equation and number should render in insertion green; green_text={green_text:?}"
    );
    assert!(
        text.contains("See Equation 1 2 for the revised equation."),
        "new-side equation label/reference should remain live while the number change is marked:\n{text}"
    );
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
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => walk(content),
        }
    }

    blocks
        .iter()
        .flat_map(|block| &block.edits)
        .map(walk_edit)
        .sum()
}

fn opaque_replacement_payloads<'a>(
    blocks: &'a [typst_diff::diff::DiffBlockEdit],
) -> Vec<(&'a typst_diff::diff::OldDisplaySurface, &'a Content)> {
    use typst_diff::diff::{EditContent, RealizedEdit};

    fn walk_content<'a>(
        content: &'a EditContent,
        out: &mut Vec<(&'a typst_diff::diff::OldDisplaySurface, &'a Content)>,
    ) {
        match content {
            EditContent::OpaqueReplacement { old, new } => out.push((old, new)),
            EditContent::Nested { edits, .. } => {
                for edit in edits {
                    walk_edit(edit, out);
                }
            }
            EditContent::Inserted(_) | EditContent::Deleted(_) | EditContent::Modified { .. } => {}
        }
    }

    fn walk_edit<'a>(
        edit: &'a RealizedEdit,
        out: &mut Vec<(&'a typst_diff::diff::OldDisplaySurface, &'a Content)>,
    ) {
        match edit {
            RealizedEdit::ReplaceAt { content, .. }
            | RealizedEdit::InsertBefore { content, .. }
            | RealizedEdit::InsertAfter { content, .. }
            | RealizedEdit::Append { content }
            | RealizedEdit::WholeBlock(content)
            | RealizedEdit::LogOnly(content)
            | RealizedEdit::MarkBaseInserted(content) => walk_content(content, out),
        }
    }

    let mut out = Vec::new();
    for block in blocks {
        for edit in &block.edits {
            walk_edit(edit, &mut out);
        }
    }
    out
}

fn plain_occurrences(content: &Content, needle: &str) -> usize {
    content.plain_text().matches(needle).count()
}

fn equation_node_count(content: &Content) -> usize {
    let mut count = 0;
    let _ = content.traverse::<_, ()>(&mut |c| {
        if c.is::<typst::math::EquationElem>() {
            count += 1;
        }
        std::ops::ControlFlow::Continue(())
    });
    count
}

fn math_cancel_count(content: &Content) -> usize {
    let mut count = 0;
    let _ = content.traverse::<_, ()>(&mut |c| {
        if c.is::<typst::math::CancelElem>() {
            count += 1;
        }
        std::ops::ControlFlow::Continue(())
    });
    count
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
fn figure_caption_label_renders_once_around_body_edit() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let case = "89-show-figure-caption-template-changed";
    let result = diff_annotated_corpus(case);
    let figure = only_changed_figure_block(&result, case);
    assert_figure_slots_are_patch_surface_paths(figure);
    assert_edit_paths_resolve_for_base(&figure.base, &figure.edits);

    assert!(matches!(
        figure.edits.as_slice(),
        [RealizedEdit::ReplaceAt {
            path,
            content: EditContent::Modified { .. },
        }] if path.as_slice() == [1]
    ));

    let log = result.modification_log();
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(deleted.contains("Measurements"), "{log}");
    assert!(inserted.contains("Updated measurements"), "{log}");
    assert!(!log.contains("MeasurementsFigure"), "{log}");
    assert!(!log.contains("measurementsFigure"), "{log}");
    assert!(!log.contains("Figure -> Exhibit"), "{log}");

    let modified = collect_replace_at_modified_paths_and_bases(&result.blocks);
    assert!(
        modified.iter().any(|(path, _base)| path.as_slice() == [1]),
        "caption body edit should target [1]: {modified:?}"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let world = corpus_world(&format!("{case}/new.typ"));
    let document = typst_diff::eval::layout_document(&world, &annotated).unwrap();
    let rendered = normalize_whitespace(
        rendered_text_runs(&document.pages[0].frame)
            .iter()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join("")
            .as_str(),
    );

    assert!(
        rendered.contains("Figure") || rendered.contains("Exhibit"),
        "{rendered}"
    );
    assert!(!rendered.contains("Figure 2"), "{rendered}");
    assert!(
        rendered.contains("Figure 1") || rendered.contains("Exhibit"),
        "{rendered}"
    );
    assert!(rendered.contains("Measurements"), "{rendered}");
    assert!(rendered.contains("Updated measurements"), "{rendered}");
    assert!(
        !rendered.contains("Figure: Measurements Figure: Updated measurements"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("Exhibit: Measurements Exhibit: Updated measurements"),
        "{rendered}"
    );
    assert_contains_in_order(&rendered, &["Measurements", "Updated measurements"]);
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
fn deleted_figure_caption_uses_inert_old_display_payload() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let case = "72-figure-caption-deleted";
    let expected = "Distribution of measurements";
    let result = diff_annotated_corpus_with_rendered_regions(case);
    let figure = only_changed_figure_block(&result, case);
    let payload_text = match figure.edits.as_slice() {
        [
            RealizedEdit::InsertAfter {
                anchor,
                content: EditContent::Deleted(content),
            },
        ] if anchor.as_slice() == [0] => content.plain_text().to_string(),
        edits => panic!("unexpected caption edit shape: {}", edits.len()),
    };
    assert!(
        payload_text.contains(expected),
        "deleted caption old display should contain caption text: {payload_text:?}"
    );

    let (_inserted, deleted, _modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert_eq!(deleted.len(), 1);
    assert!(
        deleted[0].contains(expected),
        "deleted caption log should contain caption text: {deleted:?}"
    );
    assert!(
        modified_deleted.is_empty(),
        "deleted caption label must not be represented as token-level modified deletes: {modified_deleted:?}"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let struck = struck_texts(&annotated);
    assert!(
        struck.iter().any(|text| text.contains(expected)),
        "expected struck caption text, got {struck:?}"
    );

    let world = corpus_world(&format!("{case}/new.typ"));
    let document = typst_diff::eval::layout_document(&world, &annotated).unwrap();
    let rendered_text = normalize_whitespace(
        rendered_text_runs(&document.pages[0].frame)
            .iter()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join("")
            .as_str(),
    );

    assert!(rendered_text.contains(expected), "{rendered_text}");
    assert_eq!(
        rendered_text
            .matches("Distribution of measurements")
            .count(),
        1,
        "{rendered_text}"
    );
    assert!(!rendered_text.contains("Figure 2"), "{rendered_text}");
    assert!(!rendered_text.contains("FigureFigure"), "{rendered_text}");
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

        if case == "72-figure-caption-deleted" {
            let world = corpus_world(&format!("{case}/new.typ"));
            let document = typst_diff::eval::layout_document(&world, &rendered).unwrap();
            let rendered_text = normalize_whitespace(
                rendered_text_runs(&document.pages[0].frame)
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
                    .as_str(),
            );
            assert!(!rendered_text.contains("Figure 2"), "{rendered_text}");
            assert!(!rendered_text.contains("FigureFigure"), "{rendered_text}");
            assert!(
                !rendered_text.contains("Figure: Distribution of measurements Figure"),
                "{rendered_text}"
            );
        }
    }
}

#[test]
fn figure_body_word_diff_does_not_renumber_caption() {
    use typst::visualize::RectElem;
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("63-figure-body-changed");
    let figure = only_changed_figure_block(&result, "63-figure-body-changed");
    assert_figure_slots_are_patch_surface_paths(figure);
    match figure.edits.as_slice() {
        [
            RealizedEdit::ReplaceAt {
                path,
                content: EditContent::Nested { base, edits },
            },
        ] => {
            assert_eq!(path.as_slice(), [0], "figure body edit should target [0]");
            assert!(
                base.realized.is::<RectElem>(),
                "figure body edit should preserve the realized rectangle boundary"
            );
            assert!(
                matches!(
                    edits.as_slice(),
                    [RealizedEdit::ReplaceAt {
                        path,
                        content: EditContent::Modified { .. },
                    }] if path.as_slice() == [0]
                ),
                "rectangle body should receive the word modification"
            );
        }
        _ => panic!("figure body edit should be a nested rectangle edit"),
    }

    let (_inserted, _deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert!(
        modified_inserted.iter().any(|text| text == "New"),
        "figure body should keep word-level inserted text, got modified_inserted={modified_inserted:?}"
    );
    assert!(
        modified_deleted.iter().any(|text| text == "Old"),
        "figure body should keep word-level deleted text, got modified_deleted={modified_deleted:?}"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let plain = normalize_whitespace(annotated.plain_text().as_str());
    assert!(
        plain.contains("Old New figure body label"),
        "annotated figure should preserve the body word diff without opaque replacement:\n{plain}"
    );
    assert!(
        plain.contains("Stable caption"),
        "annotated figure should retain the caption body:\n{plain}"
    );

    let world = corpus_world("63-figure-body-changed/new.typ");
    let document = typst_diff::eval::layout_document(&world, &annotated).unwrap();
    let rendered = normalize_whitespace(
        rendered_text_runs(&document.pages[0].frame)
            .iter()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>()
            .join("")
            .as_str(),
    );
    assert!(
        rendered.contains("Figure 1: Stable caption"),
        "patched figure should keep the source figure number:\n{rendered}"
    );
    assert!(
        !rendered.contains("Figure 2: Stable caption"),
        "patched figure should not inherit duplicate realization state:\n{rendered}"
    );
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
fn opaque_figure_body_replacement_keeps_old_and_new_visual_surfaces() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let result = diff_annotated_corpus("73-figure-body-changed-caption-added");
    let figure = only_changed_figure_block(&result, "73-figure-body-changed-caption-added");
    assert_figure_slots_are_patch_surface_paths(figure);
    assert_eq!(figure.edits.len(), 2);

    let RealizedEdit::ReplaceAt {
        path,
        content: EditContent::OpaqueReplacement { old, new },
    } = &figure.edits[0]
    else {
        panic!("figure body edit should be an opaque replacement");
    };
    assert_eq!(path.as_slice(), [0]);
    assert!(
        !old.as_content().is_empty(),
        "old opaque payload should keep the old visual surface"
    );
    assert!(
        !new.is_empty(),
        "new opaque payload should keep the new visual surface"
    );
}

#[test]
fn top_level_opaque_visual_replacements_claim_their_empty_carriers() {
    for case in ["90-opaque-graphic-replaced", "91-raw-svg-graphic-replaced"] {
        let result = diff_annotated_corpus(case);
        assert_eq!(
            count_opaque_replacements(&result.blocks),
            1,
            "{case} should emit exactly one opaque visual replacement:\n{}",
            result.modification_log()
        );

        let payloads = opaque_replacement_payloads(&result.blocks);
        assert_eq!(payloads.len(), 1);
        let (old, new) = payloads[0];
        assert!(
            !old.as_content().is_empty(),
            "{case} old payload should keep the old visual surface"
        );
        assert!(
            !new.is_empty(),
            "{case} new payload should keep the new visual surface"
        );
    }
}

#[test]
fn text_empty_scaffolding_changes_do_not_produce_opaque_replacement_frames() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#align(center)[
  #v(2em)
  #text(18pt)[Title]
  #v(2em)
]"#,
        r#"#align(center)[
  #v(4em)
  #text(18pt)[Title]
  #v(1em)
]"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);

    assert_eq!(count_opaque_replacements(&result.blocks), 0);

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
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
        "diff/fallback-warnings.yml",
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
fn cli_emits_fallback_warning_by_default_and_quiet_suppresses_stderr_only() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("old.typ"), "The old text.").unwrap();
    std::fs::write(dir.path().join("new.typ"), "The new text.").unwrap();

    let warned_pdf = dir.path().join("warned.pdf");
    let warned = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["old.typ", "new.typ", "-o"])
        .arg(&warned_pdf)
        .output()
        .unwrap();
    assert!(warned.status.success(), "{warned:?}");
    let warned_stderr = String::from_utf8_lossy(&warned.stderr);
    assert!(
        warned_stderr.contains("FB-010-word-diff-or-opaque-replacement-ladder"),
        "{warned_stderr}"
    );

    let quiet_pdf = dir.path().join("quiet.pdf");
    let quiet_debug_dir = quiet_pdf.with_extension("debug");
    let quiet = Command::new(env!("CARGO_BIN_EXE_typst-diff"))
        .current_dir(dir.path())
        .args(["old.typ", "new.typ", "-o"])
        .arg(&quiet_pdf)
        .args(["--quiet", "--debug"])
        .output()
        .unwrap();
    assert!(quiet.status.success(), "{quiet:?}");
    let quiet_stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        !quiet_stderr.contains("FB-010-word-diff-or-opaque-replacement-ladder"),
        "{quiet_stderr}"
    );
    let warnings =
        std::fs::read_to_string(quiet_debug_dir.join("diff/fallback-warnings.yml")).unwrap();
    assert!(
        warnings.contains("FB-010-word-diff-or-opaque-replacement-ladder"),
        "{warnings}"
    );
    assert!(warnings.contains("total_count: 1"), "{warnings}");
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
    assert!(trace.contains(r#""record":"decision_event""#), "{trace}");
    assert!(
        trace.contains(r#""warning_code":"FB-010-word-diff-or-opaque-replacement-ladder""#),
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
    let old_world = corpus_world("92-diagram-caption-and-opaque-body-changed/old.typ");
    let new_world = corpus_world("92-diagram-caption-and-opaque-body-changed/new.typ");
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
    let normal_phylum = normal_runs
        .iter()
        .find(|run| run.text.contains("Phylum: Chordata"))
        .expect("normal Phylum item should render");
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
        (phylum.x - normal_phylum.x).abs() <= 0.5,
        "modified Phylum item should keep normal list-item text position; phylum={phylum:?}, normal_phylum={normal_phylum:?}"
    );
    assert!(
        (class.x - normal_class.x).abs() <= 0.5,
        "modified Class item should keep normal list-item text position; class={class:?}, normal_class={normal_class:?}"
    );
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
fn deleted_old_table_keeps_table_structure_in_annotated_tree() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#table(
  columns: 2,
  [Old A], [Old B],
  [1], [2],
)
"#,
        "After\n",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);

    assert_eq!(count_nodes::<typst::model::TableElem>(&annotated), 1);
    assert!(annotated.plain_text().contains("Old A"));

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_old_grid_keeps_grid_structure_in_annotated_tree() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#grid(
  columns: 2,
  [Old A], [Old B],
  [1], [2],
)
"#,
        "After\n",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);

    assert_eq!(count_nodes::<typst::layout::GridElem>(&annotated), 1);
    assert!(annotated.plain_text().contains("Old A"));

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_old_boxed_table_keeps_opaque_box_surface_in_annotated_tree() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#box(width: 100%)[
  #table(
    columns: 2,
    [Old A], [Old B],
    [1], [2],
  )
]
"#,
        "After\n",
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);

    assert!(
        count_nodes::<typst::layout::BoxElem>(&annotated) >= 1,
        "deleted boxed table should remain an opaque box surface"
    );
    assert!(annotated.plain_text().contains("Old A"));

    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn shown_boxed_table_change_recurses_into_cells_without_opaque_duplicate() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#show table: it => box(width: 100%)[#it]

#table(
  columns: 2,
  [Project], [Value],
  [A01], [1],
)
"#,
        r#"#show table: it => box(width: 100%)[#it]

#table(
  columns: 2,
  [Project], [Value],
  [A01], [2],
)
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let (inserted, deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);

    assert_eq!(count_opaque_replacements(&result.blocks), 0);
    assert!(
        modified_deleted.iter().any(|text| text == "1"),
        "expected deleted table cell value, got deleted={deleted:?} modified_deleted={modified_deleted:?}"
    );
    assert!(
        modified_inserted.iter().any(|text| text == "2"),
        "expected inserted table cell value, got inserted={inserted:?} modified_inserted={modified_inserted:?}"
    );

    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    assert!(
        compact.plain_text().contains('2') && count_nodes::<typst::text::StrikeElem>(&compact) == 0,
        "compact table diff should show inserted value without struck deleted substitution: {}",
        compact.plain_text()
    );
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn authored_boxed_table_change_recurses_into_cells_without_opaque_replacement() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#box(table(
  columns: 2,
  [Project], [Value],
  [A01], [1],
))
"#,
        r#"#box(table(
  columns: 2,
  [Project], [Value],
  [A01], [2],
))
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let (inserted, deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "authored boxed table should recurse into table cells"
    );
    assert!(
        modified_deleted.iter().any(|text| text == "1"),
        "expected deleted table cell value, got deleted={deleted:?} modified_deleted={modified_deleted:?}"
    );
    assert!(
        modified_inserted.iter().any(|text| text == "2"),
        "expected inserted table cell value, got inserted={inserted:?} modified_inserted={modified_inserted:?}"
    );

    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    assert!(
        compact.plain_text().contains('2') && count_nodes::<typst::text::StrikeElem>(&compact) == 0,
        "compact table diff should show inserted value without struck deleted substitution: {}",
        compact.plain_text()
    );
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_boxed_table_change_recurses_into_cells_without_opaque_replacement() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#context [
  #let value = "1"
  #box(table(
    columns: 2,
    [Project], [Value],
    [A01], [#value],
  ))
]
"#,
        r#"#context [
  #let value = "2"
  #box(table(
    columns: 2,
    [Project], [Value],
    [A01], [#value],
  ))
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let (_inserted, _deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);

    assert_eq!(count_opaque_replacements(&result.blocks), 0);
    assert!(modified_deleted.iter().any(|text| text == "1"));
    assert!(modified_inserted.iter().any(|text| text == "2"));
}

#[test]
fn context_boxed_table_after_generated_pagebreak_recurses_into_cells() {
    let old = r#"#set page(width: 220pt, height: 90pt, margin: 5pt)
#block(height: 70pt)[Intro]

#context [
  #box(table(
    columns: 2,
    [Item], [Value],
    [A], [1],
  ))
]
"#;
    let new = r#"#set page(width: 220pt, height: 90pt, margin: 5pt)
#block(height: 70pt)[Intro]

#context [
  #box(table(
    columns: 2,
    [Item], [Value],
    [A], [2],
  ))
]
"#;
    let (_dir, result, _annotated) = diff_temp_sources(old, new);
    let log = result.modification_log();

    let opaque_replacements = count_opaque_replacements(&result.blocks);
    assert!(
        opaque_replacements == 0,
        "context table after generated pagebreak should keep ownership of the visible table; opaque_replacements={opaque_replacements}:\n{log}"
    );
    let (_inserted, _deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert!(modified_deleted.iter().any(|text| text == "1"));
    assert!(modified_inserted.iter().any(|text| text == "2"));
}

#[test]
fn context_boxed_table_after_pagebreak_large_change_recurses_into_cells() {
    let old = r#"#set page(width: 230pt, height: 95pt, margin: 5pt)
#block(height: 72pt)[Intro]

#context [
  #box(table(
    columns: 4,
    [Project], [2027], [2028], [2029],
    [A01], [10], [20], [30],
    [B01], [40], [50], [60],
    [C01], [70], [80], [90],
    [D01], [100], [110], [120],
    [Total], [220], [260], [300],
  ))
]
"#;
    let new = r#"#set page(width: 230pt, height: 95pt, margin: 5pt)
#block(height: 72pt)[Intro]

#context [
  #box(table(
    columns: 4,
    [Project], [2027], [2028], [2029],
    [C01], [7], [8], [9],
    [Total], [7], [8], [9],
  ))
]
"#;
    let (_dir, result, _annotated) = diff_temp_sources(old, new);
    let log = result.modification_log();

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "large context table replacement after pagebreak should recurse through table cells:\n{log}"
    );
    let (_inserted, _deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert!(modified_deleted.iter().any(|text| text == "70"));
    assert!(modified_inserted.iter().any(|text| text == "7"));
}

#[test]
fn boxed_table_with_local_style_and_deleted_row_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#box[
  #set text(size: 9pt)
  #table(
    columns: 2,
    [Project], [Value],
    [A01], [1],
    [B01], [2],
  )
]
"#,
        r#"#box[
  #set text(size: 9pt)
  #table(
    columns: 2,
    [Project], [Value],
    [A01], [1],
  )
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "deleted table row inside styled box should be represented through table cell slots"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(
        !compact.plain_text().contains("B01"),
        "compact mode should hide deleted row cells"
    );
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_boxed_table_with_local_style_and_deleted_row_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#context [
  #box[
    #set text(size: 9pt)
    #table(
      columns: 2,
      [Project], [Value],
      [A01], [1],
      [B01], [2],
    )
  ]
]
"#,
        r#"#context [
  #box[
    #set text(size: 9pt)
    #table(
      columns: 2,
      [Project], [Value],
      [A01], [1],
    )
  ]
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "context-generated styled boxed table should be represented through table cell slots"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(
        !compact.plain_text().contains("B01"),
        "compact mode should hide deleted row cells"
    );
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_state_generated_boxed_table_deleted_row_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1"), ("B01", "2")))

#context [
  #let data = rows.final()
  #box[
    #set text(size: 9pt)
    #table(
      columns: 2,
      [Project], [Value],
      ..data.map(((project, value)) => ([#project], [#value])).flatten(),
    )
  ]
]
"#,
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1"),))

#context [
  #let data = rows.final()
  #box[
    #set text(size: 9pt)
    #table(
      columns: 2,
      [Project], [Value],
      ..data.map(((project, value)) => ([#project], [#value])).flatten(),
    )
  ]
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "state-generated context table should be represented through table cell slots"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(!compact.plain_text().contains("B01"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_boxed_table_with_header_spans_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let years = ("2027/2", "2028")

#context [
  #let rows = (("A01", "1", "2"), ("B01", "3", "4"))
  #box[
    #set text(size: 9pt)
    #table(
      columns: (35pt, auto, auto),
      table.header(
        table.cell(rowspan: 2)[Project],
        table.cell(colspan: 2)[Years],
        ..years,
      ),
      ..rows.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
      table.hline(),
      table.cell[Total], [4], [6],
    )
  ]
]
"#,
        r#"#let years = ("2027/2", "2028")

#context [
  #let rows = (("A01", "1", "2"),)
  #box[
    #set text(size: 9pt)
    #table(
      columns: (35pt, auto, auto),
      table.header(
        table.cell(rowspan: 2)[Project],
        table.cell(colspan: 2)[Years],
        ..years,
      ),
      ..rows.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
      table.hline(),
      table.cell[Total], [1], [2],
    )
  ]
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "context-generated boxed table with spanning header cells should recurse into table cells"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(
        !compact.plain_text().contains("B01"),
        "compact mode should hide cells from the deleted row"
    );
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_state_generated_boxed_table_with_header_spans_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1", "2"), ("B01", "3", "4")))
#let years = ("2027/2", "2028")

#context [
  #let data = rows.final()
  #let totals = data.fold((0, 0), (acc, row) => (acc.at(0) + int(row.at(1)), acc.at(1) + int(row.at(2))))
  #box[
    #set text(size: 9pt)
    #table(
      columns: (35pt, auto, auto),
      fill: (x, y) => if y <= 1 { luma(230) },
      stroke: (x, y) => (
        left: if x < 1 { 1pt } else { 0.5pt },
        right: 0.5pt,
        top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
        bottom: 1pt,
      ),
      table.header(
        table.cell(rowspan: 2)[Project],
        table.cell(colspan: 2)[Years],
        ..years,
      ),
      ..data.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
      table.hline(stroke: 1pt),
      table.cell(fill: luma(230))[Total], [#str(totals.at(0))], [#str(totals.at(1))],
    )
  ]
]
"#,
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1", "2"),))
#let years = ("2027/2", "2028")

#context [
  #let data = rows.final()
  #let totals = data.fold((0, 0), (acc, row) => (acc.at(0) + int(row.at(1)), acc.at(1) + int(row.at(2))))
  #box[
    #set text(size: 9pt)
    #table(
      columns: (35pt, auto, auto),
      fill: (x, y) => if y <= 1 { luma(230) },
      stroke: (x, y) => (
        left: if x < 1 { 1pt } else { 0.5pt },
        right: 0.5pt,
        top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
        bottom: 1pt,
      ),
      table.header(
        table.cell(rowspan: 2)[Project],
        table.cell(colspan: 2)[Years],
        ..years,
      ),
      ..data.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
      table.hline(stroke: 1pt),
      table.cell(fill: luma(230))[Total], [#str(totals.at(0))], [#str(totals.at(1))],
    )
  ]
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "state-generated boxed table with spanning header cells should recurse into table cells"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(!compact.plain_text().contains("B01"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_state_generated_positional_box_table_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1", "2"), ("B01", "3", "4")))
#let years = ("2027/2", "2028")

#context [
  #let data = rows.final()
  #box(table(
    columns: 3,
    fill: (x, y) => if y <= 1 { luma(230) },
    stroke: (x, y) => (
      left: if x < 1 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    table.header(
      table.cell(rowspan: 2)[Project],
      table.cell(colspan: 2)[Years],
      ..years,
    ),
    ..data.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
    table.hline(stroke: 1pt),
    table.cell(fill: luma(230))[Total], [4], [6],
  ))
]
"#,
        r#"#let rows = state("rows", ())
#rows.update((("A01", "1", "2"),))
#let years = ("2027/2", "2028")

#context [
  #let data = rows.final()
  #box(table(
    columns: 3,
    fill: (x, y) => if y <= 1 { luma(230) },
    stroke: (x, y) => (
      left: if x < 1 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    table.header(
      table.cell(rowspan: 2)[Project],
      table.cell(colspan: 2)[Years],
      ..years,
    ),
    ..data.map(((project, a, b)) => ([#project], [#a], [#b])).flatten(),
    table.hline(stroke: 1pt),
    table.cell(fill: luma(230))[Total], [1], [2],
  ))
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "state-generated positional box(table(...)) should recurse into table cells"
    );
    assert!(compact.plain_text().contains("A01"));
    assert!(!compact.plain_text().contains("B01"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_generated_dynamic_box_table_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let years = ("2027/2", "2028")

#context [
  #let category-funds = (
    "Staff": (1000, 2000),
    "Direct Costs": (3000, 4000),
    "Instrumentation": (0, 0),
    "Fellowships": (0, 0),
    "Global Funds": (0, 0),
  )
  #let year-funds = years.enumerate().map(((i, y)) => (
    y,
    ..category-funds.values().map(x => str(x.at(i))),
    str(category-funds.values().map(x => x.at(i)).sum()),
  )).flatten()
  #let category-sums = category-funds.values().map(x => str(x.sum()))
  #let category-sums = category-sums + (str(category-funds.values().map(x => x.sum()).sum()),)
  #let categories = category-funds.len()

  #box(table(
    columns: 2 + categories,
    fill: (x, y) => if y <= 1 { luma(230) },
    stroke: (x, y) => (
      left: if x < 2 or x == 6 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    row-gutter: (auto, 4pt, auto),
    align: (x, y) => if x == 0 and y > 0 { left } else { center },
    table.header(
      table.cell(rowspan: 2, align(center + horizon)[Financial Year / Funding Period]),
      table.cell(colspan: categories)[Funding for],
      table.cell(rowspan: 2, align(center + horizon)[Total]),
      ..category-funds.keys(),
    ),
    ..year-funds,
    table.hline(stroke: 1pt),
    table.cell(fill: luma(230))[Total], ..category-sums,
  ))
]
"#,
        r#"#let years = ("2027/2", "2028")

#context [
  #let category-funds = (
    "Staff": (1000, 2000),
    "Direct Costs": (30, 40),
    "Instrumentation": (0, 0),
    "Fellowships": (0, 0),
    "Global Funds": (0, 0),
  )
  #let year-funds = years.enumerate().map(((i, y)) => (
    y,
    ..category-funds.values().map(x => str(x.at(i))),
    str(category-funds.values().map(x => x.at(i)).sum()),
  )).flatten()
  #let category-sums = category-funds.values().map(x => str(x.sum()))
  #let category-sums = category-sums + (str(category-funds.values().map(x => x.sum()).sum()),)
  #let categories = category-funds.len()

  #box(table(
    columns: 2 + categories,
    fill: (x, y) => if y <= 1 { luma(230) },
    stroke: (x, y) => (
      left: if x < 2 or x == 6 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    row-gutter: (auto, 4pt, auto),
    align: (x, y) => if x == 0 and y > 0 { left } else { center },
    table.header(
      table.cell(rowspan: 2, align(center + horizon)[Financial Year / Funding Period]),
      table.cell(colspan: categories)[Funding for],
      table.cell(rowspan: 2, align(center + horizon)[Total]),
      ..category-funds.keys(),
    ),
    ..year-funds,
    table.hline(stroke: 1pt),
    table.cell(fill: luma(230))[Total], ..category-sums,
  ))
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "dynamic context-generated funding table should recurse into cells"
    );
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    assert!(compact.plain_text().contains("Direct Costs"));
    assert!(!compact.plain_text().contains("10000"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn reused_named_table_recurses_into_cells_without_opaque_replacement() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set table(fill: (x, y) => if y < 2 { luma(230) })
#show table.cell.where(y: 0): strong

#let overview = table(
  columns: 3,
  table.header([Funding for], [2027], [2028]),
  [Staff], [1000], [2000],
  [Direct costs], [3000], [4000],
  [Total], [4000], [6000],
)

#overview
"#,
        r#"#set table(fill: (x, y) => if y < 2 { luma(230) })
#show table.cell.where(y: 0): strong

#let overview = table(
  columns: 3,
  table.header([Funding for], [2027], [2028]),
  [Staff], [1000], [2000],
  [Direct costs], [30], [40],
  [Total], [1030], [2040],
)

#overview
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "a table stored in a variable and reused should still carry table-cell slots"
    );
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    assert!(compact.plain_text().contains("Direct costs"));
    assert!(!compact.plain_text().contains("3000"));
    assert!(!compact.plain_text().contains("4000"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn context_generated_wide_overview_box_table_recurses_into_cells() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#let years = ("2027/2", "2028", "2029", "2030", "2031/1")
#let highlight = luma(85%)
#let requested = state("requested", ())
#requested.update((
  "Staff": (81600, 163200, 163200, 163200, 81600),
  "Direct Costs": (3000, 4000, 3000, 3000, 2000),
  "Instrumentation": (0, 0, 0, 0, 0),
  "Fellowships": (0, 0, 0, 0, 0),
  "Global Funds": (0, 0, 0, 0, 0),
))

#context [
  #let category-funds = requested.final()
  #let year-funds = years.enumerate().map(((i, y)) => (
    y,
    ..category-funds.values().map(x => str(calc.round(x.at(i) / 1000, digits: 1))),
    str(calc.round(category-funds.values().map(x => x.at(i)).sum() / 1000, digits: 1)),
  )).flatten()
  #let category-sums = category-funds.values().map(x => str(calc.round(x.sum() / 1000, digits: 1)))
  #let category-sums = category-sums + (str(calc.round(category-funds.values().map(x => x.sum()).sum() / 1000, digits: 1)),)
  #let categories = category-funds.len()

  #box(table(
    columns: 2 + categories,
    fill: (x, y) => if y <= 1 { highlight },
    stroke: (x, y) => (
      left: if x < 2 or x == 6 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    row-gutter: (auto, 4pt, auto),
    align: (x, y) => if x == 0 and y > 0 { left } else { center },
    table.header(
      table.cell(rowspan: 2, align(center + horizon)[Financial Year / Funding Period]),
      table.cell(colspan: categories)[Funding for],
      table.cell(rowspan: 2, align(center + horizon)[Total]),
      ..category-funds.keys(),
    ),
    ..year-funds,
    table.hline(stroke: 1pt),
    table.cell(fill: highlight)[Total], ..category-sums,
  ))
]
"#,
        r#"#let years = ("2027/2", "2028", "2029", "2030", "2031/1")
#let highlight = luma(85%)
#let requested = state("requested", ())
#requested.update((
  "Staff": (81600, 163200, 163200, 163200, 81600),
  "Direct Costs": (30, 40, 30, 30, 20),
  "Instrumentation": (0, 0, 0, 0, 0),
  "Fellowships": (0, 0, 0, 0, 0),
  "Global Funds": (0, 0, 0, 0, 0),
))

#context [
  #let category-funds = requested.final()
  #let year-funds = years.enumerate().map(((i, y)) => (
    y,
    ..category-funds.values().map(x => str(calc.round(x.at(i) / 1000, digits: 1))),
    str(calc.round(category-funds.values().map(x => x.at(i)).sum() / 1000, digits: 1)),
  )).flatten()
  #let category-sums = category-funds.values().map(x => str(calc.round(x.sum() / 1000, digits: 1)))
  #let category-sums = category-sums + (str(calc.round(category-funds.values().map(x => x.sum()).sum() / 1000, digits: 1)),)
  #let categories = category-funds.len()

  #box(table(
    columns: 2 + categories,
    fill: (x, y) => if y <= 1 { highlight },
    stroke: (x, y) => (
      left: if x < 2 or x == 6 { 1pt } else { 0.5pt },
      right: 0.5pt,
      top: if y == 0 or y == 2 { 1pt } else { 0.5pt },
      bottom: 1pt,
    ),
    row-gutter: (auto, 4pt, auto),
    align: (x, y) => if x == 0 and y > 0 { left } else { center },
    table.header(
      table.cell(rowspan: 2, align(center + horizon)[Financial Year / Funding Period]),
      table.cell(colspan: categories)[Funding for],
      table.cell(rowspan: 2, align(center + horizon)[Total]),
      ..category-funds.keys(),
    ),
    ..year-funds,
    table.hline(stroke: 1pt),
    table.cell(fill: highlight)[Total], ..category-sums,
  ))
]
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "wide context-generated overview table should recurse into table cells"
    );
    let compact = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    assert!(compact.plain_text().contains("Direct Costs"));
    assert!(!compact.plain_text().contains("13"));
    assert_eq!(count_nodes::<typst::text::StrikeElem>(&compact), 0);
    let pdf = typst_diff::render_to_pdf(&compact, &new_world).unwrap();
    assert_valid_pdf(&pdf);
}

#[test]
fn show_wrapper_context_generated_forward_state_table_recurses_into_cells() {
    let old = r#"#let requested = state("requested", ())
#let years = ("2027/2", "2028")

#let wrapper(body) = [
  #context [
    #let category-funds = requested.final()
    #let year-funds = years.enumerate().map(((i, y)) => (
      y,
      ..category-funds.values().map(x => str(x.at(i))),
      str(category-funds.values().map(x => x.at(i)).sum()),
    )).flatten()
    #let category-sums = category-funds.values().map(x => str(x.sum()))
    #let category-sums = category-sums + (str(category-funds.values().map(x => x.sum()).sum()),)
    #let categories = category-funds.len()

    #box(table(
      columns: 2 + categories,
      fill: (x, y) => if y <= 1 { luma(230) },
      stroke: 0.5pt,
      table.header(
        table.cell(rowspan: 2)[Financial Year / Funding Period],
        table.cell(colspan: categories)[Funding for],
        table.cell(rowspan: 2)[Total],
        ..category-funds.keys(),
      ),
      ..year-funds,
      table.hline(stroke: 1pt),
      table.cell(fill: luma(230))[Total], ..category-sums.map(str),
    ))
  ]

  #body
]

#show: wrapper

#requested.update((
  "Staff": (1000, 2000),
  "Direct Costs": (3000, 4000),
))
"#;
    let new = r#"#let requested = state("requested", ())
#let years = ("2027/2", "2028")

#let wrapper(body) = [
  #context [
    #let category-funds = requested.final()
    #let year-funds = years.enumerate().map(((i, y)) => (
      y,
      ..category-funds.values().map(x => str(x.at(i))),
      str(category-funds.values().map(x => x.at(i)).sum()),
    )).flatten()
    #let category-sums = category-funds.values().map(x => str(x.sum()))
    #let category-sums = category-sums + (str(category-funds.values().map(x => x.sum()).sum()),)
    #let categories = category-funds.len()

    #box(table(
      columns: 2 + categories,
      fill: (x, y) => if y <= 1 { luma(230) },
      stroke: 0.5pt,
      table.header(
        table.cell(rowspan: 2)[Financial Year / Funding Period],
        table.cell(colspan: categories)[Funding for],
        table.cell(rowspan: 2)[Total],
        ..category-funds.keys(),
      ),
      ..year-funds,
      table.hline(stroke: 1pt),
      table.cell(fill: luma(230))[Total], ..category-sums.map(str),
    ))
  ]

  #body
]

#show: wrapper

#requested.update((
  "Staff": (1000, 2000),
  "Direct Costs": (30, 40),
))
"#;
    let (_dir, old_world, new_world) = temp_worlds(old, new);
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);

    assert_eq!(
        count_opaque_replacements(&result.blocks),
        0,
        "show-wrapper context table fed by later state.final() should recurse into cells"
    );
    let (_inserted, _deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert!(modified_deleted.iter().any(|text| text == "3000"));
    assert!(modified_inserted.iter().any(|text| text == "30"));
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

    let (inserted, _deleted, modified_inserted, _modified_deleted) =
        collect_edit_texts(&result.blocks);
    assert!(
        inserted.iter().any(|text| text.contains("Review risks")),
        "nested inserted list item should be represented as inserted content, got inserted={inserted:?}"
    );
    assert!(
        modified_inserted
            .iter()
            .all(|text| !text.contains("Review risks")),
        "nested inserted list item should not be flattened into a parent word insertion, got modified_inserted={modified_inserted:?}"
    );
}

#[test]
fn nested_list_item_inserted_does_not_synthesize_leading_parbreak() {
    use typst::foundations::SequenceElem;
    use typst::model::{ListElem, ParbreakElem};

    fn has_sequence_starting_with_parbreak_list(content: &Content) -> bool {
        let mut found = false;
        let _ = content.traverse::<_, ()>(&mut |node| {
            if let Some(seq) = node.to_packed::<SequenceElem>()
                && seq.children.len() >= 2
                && seq.children[0].is::<ParbreakElem>()
                && seq.children[1].is::<ListElem>()
            {
                found = true;
                return std::ops::ControlFlow::Break(());
            }
            std::ops::ControlFlow::Continue(())
        });
        found
    }

    let annotated = annotated_tree_corpus("69-nested-list-item-inserted");

    assert!(
        !has_sequence_starting_with_parbreak_list(&annotated),
        "nested list insertion must not synthesize a leading ParbreakElem before a list"
    );
}

#[test]
fn nested_list_item_inserted_preserves_nested_list_layout() {
    let result = diff_annotated_corpus("69-nested-list-item-inserted");
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let new_world = corpus_world("69-nested-list-item-inserted/new.typ");

    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let runs = rendered_text_runs(&document.pages[0].frame);
    let review = runs
        .iter()
        .find(|run| run.text.contains("Review risks"))
        .expect("inserted nested list item should render");
    let notify = runs
        .iter()
        .find(|run| run.text.contains("Notify team"))
        .expect("following nested list item should render");

    let normal = typst_diff::eval_to_realized_content(&new_world)
        .unwrap()
        .realized;
    let normal_document = typst_diff::eval::layout_document(&new_world, &normal).unwrap();
    let normal_runs = rendered_text_runs(&normal_document.pages[0].frame);
    let normal_review = normal_runs
        .iter()
        .find(|run| run.text.contains("Review risks"))
        .expect("normal inserted nested list item should render");
    let normal_notify = normal_runs
        .iter()
        .find(|run| run.text.contains("Notify team"))
        .expect("normal following nested list item should render");

    let annotated_gap = notify.y - review.y;
    let normal_gap = normal_notify.y - normal_review.y;
    assert!(
        annotated_gap <= normal_gap + 0.5,
        "inserted nested list item should keep tight spacing; annotated_gap={annotated_gap}, normal_gap={normal_gap}, review={review:?}, notify={notify:?}, normal_review={normal_review:?}, normal_notify={normal_notify:?}"
    );
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
fn table_cell_same_text_style_only_change_is_slot_modified_edit() {
    let (_dir, result, _annotated) = diff_temp_sources(
        r#"#table(columns: 2, [Metric], [#emph[Same]], [Other], [Stable])

Body unchanged.
"#,
        r#"#table(columns: 2, [Metric], [#strong[Same]], [Other], [Stable])

Body unchanged.
"#,
    );
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        deleted.contains("Same") && inserted.contains("Same"),
        "style-only table cell change should be represented as modified words; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        result.blocks.iter().any(|block| {
            block
                .edits
                .iter()
                .any(realized_edit_contains_replace_at_modified)
        }),
        "style-only table cell change should remain a slot-level ReplaceAt Modified edit"
    );
}

#[test]
fn table_raw_cell_change_uses_raw_line_diff_not_whole_cell_replacement() {
    let (_dir, result, _annotated) = diff_temp_sources(
        r#"#table(columns: 1, [
```txt
alpha
old line
omega
```
])
"#,
        r#"#table(columns: 1, [
```txt
alpha
new line
omega
```
])
"#,
    );
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        deleted.contains("old line") && inserted.contains("new line"),
        "raw table cell should report changed raw lines; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        !deleted.contains("alpha") && !inserted.contains("alpha"),
        "raw table cell should not report unchanged prefix line as changed; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        !deleted.contains("omega") && !inserted.contains("omega"),
        "raw table cell should not report unchanged suffix line as changed; deleted={deleted:?}; inserted={inserted:?}"
    );
}

#[test]
fn repeated_same_text_table_style_change_targets_one_cell() {
    let (_dir, result, _annotated) = diff_temp_sources(
        r#"#table(columns: 2, [Same], [Same], [Other], [Stable])

Body unchanged.
"#,
        r#"#table(columns: 2, [Same], [#strong[Same]], [Other], [Stable])

Body unchanged.
"#,
    );
    let modified = collect_replace_at_modified_paths_and_bases(&result.blocks);
    let same_modified = modified
        .iter()
        .filter(|(_path, base)| base.contains("Same"))
        .collect::<Vec<_>>();
    assert_eq!(
        same_modified.len(),
        1,
        "exactly one repeated Same cell should be modified; modified={modified:?}"
    );
    assert!(
        same_modified[0].0.last().is_some_and(|index| *index == 1),
        "the second Same cell should be targeted, not an arbitrary repeated text cell; modified={modified:?}"
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
fn corpus_46_cetz_canvas_change_is_opaque_replacement() {
    let result = diff_annotated_corpus("46-package-cetz");
    let opaque_replacements = opaque_replacement_payloads(&result.blocks);
    assert_eq!(
        opaque_replacements.len(),
        1,
        "changed CeTZ canvas should be retained as one opaque replacement:\n{}",
        result.modification_log()
    );
    let (old, new) = opaque_replacements[0];
    assert!(
        !old.as_content().is_empty(),
        "old CeTZ opaque payload should retain the old visual carrier"
    );
    assert!(
        !new.is_empty(),
        "new CeTZ opaque payload should retain the new visual carrier"
    );

    let log = result.modification_log();
    assert!(
        log.contains("block: [opaque visual content]")
            && log.contains("deleted: [old visual]")
            && log.contains("inserted: [new visual]"),
        "modification log should report the opaque CeTZ change:\n{log}"
    );
}

#[test]
fn semantic_owner_edits_are_anchored_to_realized_carriers() {
    let pad_log = diff_annotated_corpus("55-pad-changed").modification_log();
    assert!(
        pad_log.contains("## 3: modify"),
        "pad edit should stay after the heading and wrapper shell:\n{pad_log}"
    );
    assert_eq!(
        pad_log
            .lines()
            .filter(|line| line.starts_with("## "))
            .count(),
        1,
        "{pad_log}"
    );

    let box_log = diff_annotated_corpus("53-box-changed").modification_log();
    assert!(
        !box_log.contains("## 0: modify"),
        "inline box owner should not be emitted as an early standalone edit:\n{box_log}"
    );
    assert_eq!(
        box_log
            .lines()
            .filter(|line| line.starts_with("## "))
            .count(),
        1,
        "{box_log}"
    );

    let math_log = diff_annotated_corpus("22-inline-math-changed").modification_log();
    assert!(
        !math_log.contains("## 2: modify\nblock: \n"),
        "inline equation owner should not be emitted as an empty standalone edit:\n{math_log}"
    );
    assert!(
        math_log.contains("attach(base: [E], b: [p])")
            && math_log.contains("attach(base: [E], b: [k])"),
        "inline equation origins should be retained in the paragraph edit:\n{math_log}"
    );
    let math_annotated = annotated_tree_corpus("22-inline-math-changed");
    assert!(
        math_cancel_count(&math_annotated) > 0,
        "deleted inline equation should be rendered as cancelled math, not flattened struck text:\n{}",
        math_annotated.plain_text()
    );

    let display_log = diff_annotated_corpus("23-display-math-changed").modification_log();
    assert_eq!(
        display_log
            .lines()
            .filter(|line| line.starts_with("## "))
            .count(),
        3,
        "{display_log}"
    );
    assert!(
        display_log.contains("## 2: modify\nblock: \n"),
        "display equation edit should stay anchored to its realized carrier:\n{display_log}"
    );

    let display_annotated = annotated_tree_corpus("23-display-math-changed");
    assert_eq!(
        equation_node_count(&display_annotated),
        3,
        "display equation edit should render old and new display formulas once, plus the inline n occurrence:\n{}",
        display_annotated.plain_text()
    );
}

#[test]
fn figure_slot_edits_are_anchored_to_figure_carrier() {
    let caption_added = diff_annotated_corpus("71-figure-caption-added").modification_log();
    assert_eq!(
        caption_added
            .lines()
            .filter(|line| line.starts_with("## "))
            .collect::<Vec<_>>(),
        vec!["## 5: insert"],
        "{caption_added}"
    );

    let diagram =
        diff_annotated_corpus("92-diagram-caption-and-opaque-body-changed").modification_log();
    let headings = diagram
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect::<Vec<_>>();
    assert_eq!(headings, vec!["## 5: modify", "## 5: modify"], "{diagram}");
    assert!(diagram.contains("[opaque visual content]"), "{diagram}");
    assert!(diagram.contains("deleted: Old"), "{diagram}");
    assert!(diagram.contains("inserted: New"), "{diagram}");
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
fn footnote_visible_text_same_text_style_only_change_is_modified_edit() {
    let (_dir, result, _annotated) = diff_temp_sources(
        r#"#emph[Stable term]#footnote[Existing note remains unchanged.] stays visible.
"#,
        r#"#strong[Stable term]#footnote[Existing note remains unchanged.] stays visible.
"#,
    );
    let (deleted, inserted) = collect_modified_word_texts(&result.blocks);
    assert!(
        deleted.contains("Stable term") && inserted.contains("Stable term"),
        "style-only visible text beside a footnote should be represented as modified words; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        count_edits(&result.blocks, realized_edit_contains_replace_at_modified) >= 1,
        "style-only visible text beside a footnote should not be swallowed by the footnote-special path"
    );
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
fn deleted_label_does_not_duplicate_active_new_label() {
    let old = r#"#set heading(numbering: "1.")

= API <api>

The old contract is stable.

See @api.
"#;
    let new = r#"#set heading(numbering: "1.")

= API <api>

The new contract is stable.

See @api.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let new_content = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_tags = count_nodes::<typst::introspection::TagElem>(&new_content.realized);
    let annotated_tags = count_nodes::<typst::introspection::TagElem>(&annotated);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_eq!(
        annotated_tags, new_tags,
        "annotated document should keep only the new-side active labels/tags"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn inserted_label_and_reference_from_new_remain_live() {
    let old = r#"The old document has no cross reference.
"#;
    let new = r#"#set heading(numbering: "1.")

= New API <api>

See @api for the new contract.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "inserted label/reference");
    assert!(
        text.contains("New API") && text.contains("See") && text.contains('1'),
        "new reference should render in annotated output:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn changed_text_empty_context_label_from_new_remains_live() {
    let old = r#"#show ref: it => {
  let target = query(it.target).at(0, default: none)
  if target == none { return [missing #str(it.target)] }
  if target.func() == metadata { return [project #target.value.lbl] }
  it
}

#context [#metadata((kind: "project", lbl: "old-project"))#label("old-project")]

See @old-project.
"#;
    let new = r#"#show ref: it => {
  let target = query(it.target).at(0, default: none)
  if target == none { return [missing #str(it.target)] }
  if target.func() == metadata { return [project #target.value.lbl] }
  it
}

#context [#metadata((kind: "project", lbl: "new-project"))#label("new-project")]

See @new-project.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "changed contextual label");
    assert!(
        text.contains("new-project") && !text.contains("missing"),
        "new contextual label should resolve in annotated output:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_old_state_does_not_make_new_context_create_missing_label_link() {
    let old = r#"#let projects = state("projects", (:))
#let register(id) = [
  #projects.update(data => {
    data.insert(id, (id: id, title: [Old Project]))
    data
  })
  #context [#metadata((kind: "project", lbl: id))#label(id)]
]
#let lpref(id) = context {
  let data = projects.final().at(id, default: none)
  if data == none { [unknown #id] } else { link(label(id), data.title) }
}

#register("old-project")

Old body.
"#;
    let new = r#"#let projects = state("projects", (:))
#let register(id) = [
  #projects.update(data => {
    data.insert(id, (id: id, title: [Old Project]))
    data
  })
  #context [#metadata((kind: "project", lbl: id))#label(id)]
]
#let lpref(id) = context {
  let data = projects.final().at(id, default: none)
  if data == none { [unknown #id] } else { link(label(id), data.title) }
}

Reference: #lpref("old-project")
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("unknown old-project"),
        "new context must not see deleted old state updates:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_old_state_before_same_new_context_lookup_does_not_survive() {
    let definitions = r#"#let projects = state("projects", (:))
#let register(id) = [
  #projects.update(data => {
    data.insert(id, (id: id, title: [Old Project]))
    data
  })
  #context [#metadata((kind: "project", lbl: id))#label(id)]
]
#let lpref(id) = context {
  let data = projects.final().at(id, default: none)
  if data == none { [unknown #id] } else { link(label(id), data.title) }
}
"#;
    let old = format!(
        r#"{definitions}
#register("old-project")

Reference: #lpref("old-project")
"#
    );
    let new = format!(
        r#"{definitions}
Reference: #lpref("old-project")
"#
    );
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(&old, &new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("unknown old-project"),
        "deleted registration must not affect later new context lookup:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn inserted_state_update_from_new_executes() {
    let old = r#"The old document has no state.
"#;
    let new = r#"#let status = state("status", "unset")

#status.update("new")

#context [Final status: #status.final()]
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "inserted state update");
    assert!(
        text.contains("Final status: new"),
        "new state update should execute in annotated output:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn unchanged_new_state_update_inside_modified_container_stays_live() {
    let old = r#"#let status = state("status", "unset")

- #status.update("new") Old item

#context [Final status: #status.final()]
"#;
    let new = r#"#let status = state("status", "unset")

- #status.update("new") New item

#context [Final status: #status.final()]
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("Final status: new"),
        "unchanged new state update should stay live in nested base:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn unchanged_new_text_empty_state_update_stays_live() {
    let old = r#"#let ids = state("ids", ())

#ids.update(values => values + ("target",))

Old visible text.

#context {
  [Registered: ]
  ids.final().join(", ")
}
"#;
    let new = r#"#let ids = state("ids", ())

#ids.update(values => values + ("target",))

New visible text.

#context {
  [Registered: ]
  ids.final().join(", ")
}
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_modified_words_include(&result, &["Old"], &["New"]);
    assert!(
        text.contains("Registered:") && text.contains("target"),
        "new text-empty state registry should stay live:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn unchanged_new_text_empty_label_target_stays_live() {
    let old = r#"#set heading(numbering: "1.")

#heading(outlined: false)[] <target>

Old visible text.

See @target.
"#;
    let new = r#"#set heading(numbering: "1.")

#heading(outlined: false)[] <target>

New visible text.

See @target.
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_modified_words_include(&result, &["Old"], &["New"]);
    assert!(
        text.contains("See") && text.contains('1'),
        "new text-empty label target should stay live:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_reference_to_deleted_label_renders_inertly() {
    let old = r#"#set heading(numbering: "1.")

= Legacy API <legacy>

See @legacy for the old contract.
"#;
    let new = r#"#set heading(numbering: "1.")

= Current API <current>

The current contract stands alone.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let new_content = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_tags = count_nodes::<typst::introspection::TagElem>(&new_content.realized);
    let annotated_tags = count_nodes::<typst::introspection::TagElem>(&annotated);
    let new_refs = count_nodes::<typst::model::RefElem>(&new_content.realized);
    let annotated_refs = count_nodes::<typst::model::RefElem>(&annotated);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_eq!(
        annotated_tags, new_tags,
        "annotated document should not keep deleted labels active"
    );
    assert_eq!(
        annotated_refs, new_refs,
        "annotated document should render deleted references as inert content"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_link_to_deleted_label_renders_inertly() {
    let old = r#"= Old Target <old-target>

#link(<old-target>)[Project Old] will be removed.
"#;
    let new = r#"The replacement text has no old target.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("Project Old") && text.contains("replacement text"),
        "deleted link body should remain visible without keeping its old target live:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn new_missing_reference_show_rule_is_preserved_when_old_target_existed() {
    let old = r#"#show ref: it => {
  let target = query(it.target).at(0, default: none)
  if target == none { return [invalid #str(it.target)] }
  if target.func() == metadata { return [project] }
  it
}

= Old Project
#metadata((kind: "project")) <project>

See @project.
"#;
    let new = r#"#show ref: it => {
  let target = query(it.target).at(0, default: none)
  if target == none { return [invalid #str(it.target)] }
  if target.func() == metadata { return [project] }
  it
}

See @project.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("invalid project"),
        "new missing-reference show rule should remain effective:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn new_missing_label_link_show_rule_is_preserved_when_old_target_existed() {
    let old = r#"#show link: it => [link fallback: #it.body]

= Old Project <project>

See #link(<project>)[project].
"#;
    let new = r#"#show link: it => [link fallback: #it.body]

See #link(<project>)[project].
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        text.contains("link fallback: project"),
        "new missing-link show rule should remain effective:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_heading_figure_and_equation_labels_are_inert() {
    let old = r#"#set heading(numbering: "1.")
#set math.equation(numbering: "(1)")

= Legacy Heading <legacy-heading>

#figure(rect(width: 1cm, height: 5mm), caption: [Legacy figure]) <legacy-figure>

$ x = y $ <legacy-equation>

See @legacy-heading, @legacy-figure, and @legacy-equation.
"#;
    let new = r#"The new document has no legacy labels.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted mixed labels");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_reference_to_old_target_and_new_reference_to_new_target_keep_only_new_live() {
    let old = r#"#set heading(numbering: "1.")

= Legacy API <old-api>

See @old-api for the legacy contract.
"#;
    let new = r#"#set heading(numbering: "1.")

= Current API <new-api>

See @new-api for the current contract.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "old and new references");
    assert!(
        text.contains("Current API") && text.contains('1'),
        "new reference should remain live:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn equal_visible_reference_uses_new_side_reference() {
    let old = r#"#set heading(numbering: "1.")

= API <old-api>

See @old-api.
"#;
    let new = r#"#set heading(numbering: "1.")

= API <new-api>

See @new-api.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "equal visible new reference");
    assert!(
        text.contains("See") && text.contains('1'),
        "new-side equal reference should resolve:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn opaque_replacement_strips_old_labels_but_keeps_new_labels() {
    let old = r#"#figure(rect(width: 1cm, height: 5mm), caption: [Legacy figure]) <legacy-figure>

See @legacy-figure.
"#;
    let new = r#"#figure(circle(radius: 4mm), caption: [Current figure]) <current-figure>

See @current-figure.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "opaque replacement labels");
    assert!(
        text.contains("Current figure"),
        "new replacement label/reference should render:\n{text}"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn deleted_state_update_does_not_affect_active_new_state() {
    let old = r#"#let status = state("status", "unset")

#status.update("old")

The old contract is stable.

#context [Final status: #status.final()]
"#;
    let new = r#"#let status = state("status", "unset")

#status.update("new")

The new contract is stable.

#context [Final status: #status.final()]
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let new_content = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let new_state_updates =
        count_nodes::<typst::introspection::StateUpdateElem>(&new_content.realized);
    let annotated_state_updates = count_nodes::<typst::introspection::StateUpdateElem>(&annotated);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_eq!(
        annotated_state_updates, new_state_updates,
        "annotated document should keep only the new-side active state updates"
    );
    assert_valid_pdf(&pdf);
}

#[test]
fn old_context_state_get_is_inert_when_deleted() {
    let old = r#"#let progress = state("progress", "unset")

#progress.update("old")

#context [Old current progress: #progress.get()]
"#;
    let new = r#"The new document does not define progress.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted state.get context");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_context_state_final_is_inert_when_deleted() {
    let old = r#"#let progress = state("progress", "unset")

#progress.update("old")

#context [Old final progress: #progress.final()]
"#;
    let new = r#"The new document does not define progress.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted state.final context");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_context_inside_deleted_block_is_inert() {
    let old = r#"#let progress = state("progress", "unset")

#progress.update("old")

#block[
  #context [Old final progress: #progress.final()]
]
"#;
    let new = r#"The new document does not define progress.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted block context");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_hidden_state_update_is_inert_when_deleted() {
    let old = r#"#let project = state("project", ())

#hide[#project.update(values => values + ("old-project",))]

#context [Old projects: #project.final().join(", ")]
"#;
    let new = r#"The new document has no hidden project state.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted hidden state update");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_empty_structural_deletions_are_dropped() {
    let old = r#"#let project = state("project", ())

#hide[#project.update(values => values + ("old-project",))]

#context [#metadata((kind: "project"))#label("old-project")]

Visible text.
"#;
    let new = r#"Visible text.
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let (inserted, deleted, modified_inserted, modified_deleted) =
        collect_edit_texts(&result.blocks);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert!(
        inserted.is_empty()
            && deleted.is_empty()
            && modified_inserted.is_empty()
            && modified_deleted.is_empty(),
        "empty old structural content should not create visible edit payloads: inserted={inserted:?} deleted={deleted:?} modified_inserted={modified_inserted:?} modified_deleted={modified_deleted:?}"
    );
    assert_live_introspection_matches_new(&new_world, &annotated, "dropped empty old structure");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_page_counter_contexts_are_inert_when_deleted() {
    let old = r#"Old page display: #context counter(page).display()

Old final page: #context counter(page).final().first()
"#;
    let new = r#"The new document has no page counter contexts.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted page counter contexts");
    assert_valid_pdf(&pdf);
}

#[test]
fn old_query_and_here_contexts_are_inert_when_deleted() {
    let old = r#"= Legacy

Old heading count: #context query(heading.where(level: 1)).len()

Old page number: #context here().page()
"#;
    let new = r#"The new document has no query or here context.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "deleted query/here contexts");
    assert_valid_pdf(&pdf);
}

#[test]
fn modified_labelled_base_does_not_duplicate_new_label() {
    let old = r#"#set heading(numbering: "1.")

= Old API <api>

See @api.
"#;
    let new = r#"#set heading(numbering: "1.")

= New API <api>

See @api.
"#;
    let (_dir, new_world, result, annotated) = diff_temp_sources_with_world(old, new);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_modified_words_include(&result, &["Old"], &["New"]);
    assert_live_introspection_matches_new(&new_world, &annotated, "modified labelled base");
    assert_valid_pdf(&pdf);
}

#[test]
fn nested_old_label_and_context_inside_figure_caption_are_inert() {
    let old = r#"#figure(
  rect(width: 1cm, height: 5mm),
  caption: [Legacy caption #context counter(page).display() <legacy-caption>],
) <legacy-figure>

See @legacy-figure.
"#;
    let new = r#"#figure(
  rect(width: 1cm, height: 5mm),
  caption: [Current caption],
) <current-figure>

See @current-figure.
"#;
    let (_dir, new_world, _result, annotated) = diff_temp_sources_with_world(old, new);
    let text = rendered_document_text(&annotated, &new_world);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();

    assert_live_introspection_matches_new(&new_world, &annotated, "nested figure caption");
    assert!(
        text.contains("Current caption"),
        "new caption should render:\n{text}"
    );
    assert_valid_pdf(&pdf);
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
fn corpus_82_deleted_header_does_not_restyle_body_text() {
    let result = diff_annotated_corpus("82-header-deleted");
    assert!(
        result.blocks.iter().all(|block| {
            !block.edits.iter().any(|edit| {
                matches!(
                    edit,
                    typst_diff::diff::RealizedEdit::WholeBlock(
                        typst_diff::diff::EditContent::Deleted(_)
                    )
                )
            }) || block.page_styles.is_empty()
        }),
        "deleted old-only blocks must not carry live page styles"
    );

    let new_world = corpus_world("82-header-deleted/new.typ");
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    assert_eq!(
        count_nodes::<typst::layout::PagebreakElem>(&annotated),
        0,
        "deleted old pagebreaks must not remain live in the annotated new-world document"
    );
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let runs = rendered_text_runs(&document.pages[0].frame);
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let page_width = document.pages[0].frame.width().to_pt();

    assert!(
        runs.iter().any(|run| run.text.contains("Report")),
        "body heading should remain on page 1: {runs:?}"
    );
    assert!(
        runs.iter()
            .any(|run| run.text.contains("The body text is unchanged.")),
        "body paragraph should remain on page 1: {runs:?}"
    );
    assert!(
        runs.iter().any(|run| {
            run.text.contains("Old Header") && run.fill == red && run.x > page_width * 0.65
        }),
        "deleted header should keep the old right alignment on page 1: {runs:?}"
    );
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
fn corpus_41_contextual_header_style_change_is_rendered_region_edit() {
    use typst_diff::diff::{PageRegionKind, WordOp};

    let old_world = corpus_world("41-running-header-query/old.typ");
    let new_world = corpus_world("41-running-header-query/new.typ");
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();
    let header = result
        .rendered_regions
        .iter()
        .find(|region| region.kind == PageRegionKind::Header)
        .expect("expected contextual header rendered region edit");
    let first_page = header
        .pages
        .iter()
        .find(|page| page.page == 1)
        .expect("expected page 1 header edit");

    assert!(
        first_page.changed,
        "page 1 should be changed by emph-to-bold header styling"
    );
    assert_eq!(first_page.base.plain_text().as_str(), "First Chapter");
    assert!(
        first_page.word_ops.iter().any(|op| {
            matches!(op, WordOp::Delete(tokens) if tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>()
                .contains("First Chapter"))
        }),
        "expected deleted styled First Chapter token: {:?}",
        first_page.word_ops
    );
    assert!(
        first_page.word_ops.iter().any(|op| {
            matches!(op, WordOp::Insert(tokens) if tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<String>()
                .contains("First Chapter"))
        }),
        "expected inserted styled First Chapter token: {:?}",
        first_page.word_ops
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let page_height = document.pages[0].frame.height().to_pt();
    let header_runs = rendered_text_runs(&document.pages[0].frame)
        .into_iter()
        .filter(|run| run.y < page_height * 0.2)
        .collect::<Vec<_>>();
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let green = typst::visualize::Color::from_u8(0, 180, 0, 255).into();
    let red_header_text = header_runs
        .iter()
        .filter(|run| run.fill == red)
        .map(|run| run.text.as_str())
        .collect::<String>();
    let green_header_text = header_runs
        .iter()
        .filter(|run| run.fill == green)
        .map(|run| run.text.as_str())
        .collect::<String>();
    assert!(
        red_header_text.contains("First Chapter"),
        "expected deleted page-1 running header in red: {header_runs:?}"
    );
    assert!(
        green_header_text.contains("First Chapter"),
        "expected inserted page-1 running header in green: {header_runs:?}"
    );
}

#[test]
fn rendered_region_special_characters_do_not_panic() {
    use typst_diff::diff::{PageRegionKind, RenderedRegionAlignment, RenderedRegionWrapper};

    struct NoopSink;
    impl typst_diff::trace::DebugEventSink for NoopSink {}

    let old_source = r#"#let edge = "Edge [ # \ \" é"
#set page(header: align(right, context [#edge #counter(page).final().first()]))

Body.
"#;
    let new_source = r#"#let edge = "Edge [ # \ \" é"
#set page(header: align(right, context [#edge #counter(page).final().first()]))

Body.
#pagebreak()
More body.
"#;
    let (_dir, old_world, new_world) = temp_worlds(old_source, new_source);
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result =
        typst_diff::diff::diff_annotated_with_rendered_regions(&old, &new, &old_world, &new_world)
            .unwrap();
    let header = result
        .rendered_regions
        .iter()
        .find(|region| region.kind == PageRegionKind::Header)
        .expect("expected contextual header rendered region edit");
    assert_eq!(
        header.wrapper,
        RenderedRegionWrapper::Align(RenderedRegionAlignment::Right),
        "rendered header alignment should come from the AlignElem content tree"
    );

    let annotated = typst_diff::annotate::build_annotated_content_from_tree_with_debug_events(
        &result,
        false,
        &mut NoopSink,
    )
    .expect("special rendered-region characters should not fail annotation");
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let header_text = rendered_text_runs(&document.pages[0].frame)
        .into_iter()
        .filter(|run| run.y < document.pages[0].frame.height().to_pt() * 0.2)
        .map(|run| run.text)
        .collect::<String>();
    for expected in ["Edge", "[", "#", "\\", "é"] {
        assert!(
            header_text.contains(expected),
            "missing {expected:?} in rendered header text {header_text:?}"
        );
    }
    assert!(
        header_text.contains('"') || header_text.contains('“') || header_text.contains('”'),
        "missing quote in rendered header text {header_text:?}"
    );
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
fn static_header_same_text_style_only_change_is_region_modified_edit() {
    use typst_diff::diff::{EditContent, PageRegionKind, RegionPath, WordOp};

    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(header: align(right, emph[Same Header]))

Body unchanged.
"#,
        r#"#set page(header: align(right, strong[Same Header]))

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
        .expect("expected static header region edit");

    assert!(
        header.edits.iter().any(|edit| {
            realized_edit_content_or_nested_matches(edit, |content| {
                matches!(content, EditContent::Modified { word_ops, .. } if word_ops.iter().any(|op| {
                    matches!(op, WordOp::Delete(tokens) if tokens.iter().map(|token| token.text.as_str()).collect::<String>().contains("Same Header"))
                }) && word_ops.iter().any(|op| {
                    matches!(op, WordOp::Insert(tokens) if tokens.iter().map(|token| token.text.as_str()).collect::<String>().contains("Same Header"))
                }))
            })
        }),
        "style-only static header change should produce modified delete/insert word ops"
    );
}

#[test]
fn deleted_static_header_preserves_explicit_font_style() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(header: text(font: "New Computer Modern Sans")[Old Header])

Body unchanged.
"#,
        r#"Body unchanged.
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let runs = rendered_text_runs(&document.pages[0].frame);

    assert!(
        runs.iter().any(|run| {
            run.text.contains("Old Header")
                && run.fill == red
                && run.font_family == "New Computer Modern Sans"
        }),
        "deleted header should keep its explicit font style: {runs:?}"
    );
}

#[test]
fn deleted_static_header_preserves_pad_wrapper() {
    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(header: pad(left: 2cm)[Old Header])

Body unchanged.
"#,
        r#"Body unchanged.
"#,
    );
    let old = typst_diff::eval_to_realized_content(&old_world).unwrap();
    let new = typst_diff::eval_to_realized_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_annotated(&old, &new);
    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, false);
    let document = typst_diff::eval::layout_document(&new_world, &annotated).unwrap();
    let red = typst::visualize::Color::from_u8(220, 0, 0, 255).into();
    let runs = rendered_text_runs(&document.pages[0].frame);

    assert!(
        runs.iter()
            .any(|run| { run.text.contains("Old Header") && run.fill == red && run.x > 120.0 }),
        "deleted header should keep its pad wrapper: {runs:?}"
    );
}

#[test]
fn static_header_raw_change_uses_raw_line_diff_not_whole_region_replacement() {
    use typst_diff::diff::{PageRegionKind, RegionPath};

    let (_dir, old_world, new_world) = temp_worlds(
        r#"#set page(header: ```txt
alpha
old line
omega
```)

Body unchanged.
"#,
        r#"#set page(header: ```txt
alpha
new line
omega
```)

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
        .expect("expected static header region edit");
    let (deleted, inserted) = collect_region_modified_word_texts(std::slice::from_ref(header));
    assert!(
        deleted.contains("old line") && inserted.contains("new line"),
        "raw static header should report changed raw lines; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        !deleted.contains("alpha") && !inserted.contains("alpha"),
        "raw static header should not report unchanged prefix line as changed; deleted={deleted:?}; inserted={inserted:?}"
    );
    assert!(
        !deleted.contains("omega") && !inserted.contains("omega"),
        "raw static header should not report unchanged suffix line as changed; deleted={deleted:?}; inserted={inserted:?}"
    );
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
fn inserted_context_page_set_renders_after_annotation() {
    use typst_diff::diff::{EditContent, RealizedEdit};

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.typ"), "").unwrap();
    let world = typst_diff::world::SystemWorld::new(dir.path().join("main.typ")).unwrap();
    let titlepage = typst_diff::eval::eval_snippet_to_content(
        r#"#context [
  #set page(margin: (x: 2cm, y: 3cm), footer: [])
  #align(center)[New title]
]
"#,
    )
    .unwrap();
    let result = typst_diff::diff::DiffResult {
        blocks: vec![typst_diff::diff::DiffBlockEdit {
            base: typst_diff::AnnotatedContent {
                realized: typst::foundations::Content::sequence([]),
                annotation: Default::default(),
                children: vec![],
            },
            base_provenance: typst_diff::diff::BlockBaseProvenance::LiveNew,
            edits: vec![RealizedEdit::WholeBlock(EditContent::Inserted(titlepage))],
            page_styles: Default::default(),
        }],
        root_styles: Default::default(),
        regions: vec![],
        rendered_regions: vec![],
    };

    let annotated = typst_diff::annotate::build_annotated_content_from_tree(&result, true);
    let pdf = typst_diff::render_to_pdf(&annotated, &world).unwrap();

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
