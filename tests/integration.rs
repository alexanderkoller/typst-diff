use std::path::PathBuf;

fn fixtures(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn world_for(path: &str) -> typst_diff::world::SystemWorld {
    typst_diff::world::SystemWorld::new(fixtures(path)).unwrap()
}

#[test]
fn simple_diff_produces_valid_pdf() {
    let old_world = world_for("simple_old.typ");
    let new_world = world_for("simple_new.typ");
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let annotated = typst_diff::build_annotated_content(&result);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}

#[test]
fn multifile_diff_produces_valid_pdf() {
    let old_world = world_for("multifile_old/main.typ");
    let new_world = world_for("multifile_new/main.typ");
    let old = typst_diff::eval_to_content(&old_world).unwrap();
    let new = typst_diff::eval_to_content(&new_world).unwrap();
    let result = typst_diff::diff::diff_content(&old, &new);
    let annotated = typst_diff::build_annotated_content(&result);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "output is not a PDF");
    assert!(pdf.len() > 1000, "PDF suspiciously small");
}
