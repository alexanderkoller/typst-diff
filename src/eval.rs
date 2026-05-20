use anyhow::Result;
use typst::comemo::Track;
use typst::engine::{Route, Sink, Traced};
use typst::foundations::Content;
use typst::World;
use typst::ROUTINES;

pub fn eval_to_content(world: &dyn World) -> Result<Content> {
    let source = world.source(world.main())
        .map_err(|e| anyhow::anyhow!("cannot read main source: {e}"))?;
    let mut sink = Sink::new();
    let traced = Traced::default();
    typst_eval::eval(
        &ROUTINES,
        world.track(),
        traced.track(),
        sink.track_mut(),
        Route::default().track(),
        &source,
    )
    .map(|module| module.content())
    .map_err(|errs| anyhow::anyhow!("eval errors: {} diagnostic(s)", errs.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use typst::text::TextElem;
    use crate::world::SystemWorld;

    #[test]
    fn eval_extracts_text_nodes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "Hello *world*.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = eval_to_content(&world).unwrap();

        let mut texts = Vec::new();
        let _ = content.traverse::<_, ()>(&mut |c| {
            if let Some(t) = c.to_packed::<TextElem>() {
                texts.push(t.text.to_string());
            }
            std::ops::ControlFlow::Continue(())
        });
        assert!(texts.contains(&"Hello".to_string()));
        assert!(texts.contains(&"world".to_string()));
    }

    #[test]
    fn eval_inlines_includes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "#include \"ch.typ\"").unwrap();
        fs::write(dir.path().join("ch.typ"), "Included text.").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = eval_to_content(&world).unwrap();
        let plain = content.plain_text();
        assert!(plain.contains("Included text."));
    }
}
