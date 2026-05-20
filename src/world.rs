use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::{FontSearcher, FontSlot};

pub struct SystemWorld {
    root: PathBuf,
    main: FileId,
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    font_slots: Vec<FontSlot>,
    source_cache: Arc<Mutex<HashMap<FileId, Source>>>,
    file_cache: Arc<Mutex<HashMap<FileId, FileResult<Bytes>>>>,
}

impl SystemWorld {
    pub fn new(entry: impl AsRef<Path>) -> Result<Self> {
        let entry = entry.as_ref().canonicalize()
            .with_context(|| format!("cannot find {:?}", entry.as_ref()))?;
        let root = entry.parent().unwrap().to_owned();
        let filename = entry.file_name().unwrap().to_str().unwrap();
        let main = FileId::new(None, VirtualPath::new(format!("/{filename}")));

        let fonts = FontSearcher::new().search();

        let world = Self {
            root,
            main,
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(fonts.book),
            font_slots: fonts.fonts,
            source_cache: Default::default(),
            file_cache: Default::default(),
        };

        // Pre-load the main file so it's in the cache.
        world.source(main).map_err(|e| anyhow::anyhow!("{e:?}"))?;

        Ok(world)
    }

    fn disk_path(&self, id: FileId) -> PathBuf {
        self.root.join(id.vpath().as_rootless_path())
    }
}

impl World for SystemWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        let mut cache = self.source_cache.lock().unwrap();
        if let Some(src) = cache.get(&id) {
            return Ok(src.clone());
        }
        let path = self.disk_path(id);
        let text = std::fs::read_to_string(&path)
            .map_err(|_| FileError::NotFound(path.clone()))?;
        let src = Source::new(id, text);
        cache.insert(id, src.clone());
        Ok(src)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let mut cache = self.file_cache.lock().unwrap();
        if let Some(result) = cache.get(&id) {
            return result.clone();
        }
        let path = self.disk_path(id);
        let result = std::fs::read(&path)
            .map(Bytes::new)
            .map_err(|_| FileError::NotFound(path.clone()));
        cache.insert(id, result.clone());
        result
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.font_slots[index].get()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn source_reads_file_by_virtual_path() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "Hello world").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let src = world.source(world.main()).unwrap();
        assert_eq!(src.text(), "Hello world");
    }

    #[test]
    fn source_resolves_include_path() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "#include \"ch.typ\"").unwrap();
        fs::write(dir.path().join("ch.typ"), "Chapter text").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        use typst::syntax::{FileId, VirtualPath};
        let ch_id = FileId::new(None, VirtualPath::new("/ch.typ"));
        let src = world.source(ch_id).unwrap();
        assert_eq!(src.text(), "Chapter text");
    }
}
