# typst-diff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a CLI tool that compares two Typst documents and produces a PDF showing additions (green) and deletions (red strikethrough) at word granularity.

**Architecture:** Evaluate both documents to `Content` trees using the Typst compiler, run a two-level (block then word) LCS diff, annotate the diff by wrapping changed content with color+strike styling, then render to PDF via `typst_layout` + `typst_pdf` — no `.typ` intermediate file.

**Tech Stack:** Rust 2024 edition; `typst 0.14.2`, `typst-eval`, `typst-layout`, `typst-pdf`, `typst-kit` (fonts), `similar` (LCS diff), `clap` (CLI), `anyhow` (errors).

---

## File Map

```
typst-diff/
├── Cargo.toml
├── src/
│   ├── main.rs        CLI arg parsing + top-level orchestration
│   ├── world.rs       World trait impl: filesystem file loading + font searching
│   ├── eval.rs        Thin wrapper: path → Content via typst_eval::eval
│   ├── diff.rs        Two-level diff: extract_blocks, extract_words, diff_content
│   ├── annotate.rs    Build annotated Content from DiffResult
│   └── render.rs      layout_document + typst_pdf → Vec<u8>
└── tests/
    ├── integration.rs
    └── fixtures/
        ├── simple_old.typ
        ├── simple_new.typ
        ├── multifile_old/
        │   ├── main.typ
        │   └── chapter.typ
        └── multifile_new/
            ├── main.typ
            └── chapter.typ
```

---

## Task 1: Project scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Write Cargo.toml**

```toml
[package]
name = "typst-diff"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "typst-diff"
path = "src/main.rs"

[dependencies]
typst = "0.14.2"
typst-eval = "0.14.2"
typst-layout = "0.14.2"
typst-pdf = "0.14.2"
typst-kit = "0.14.2"
similar = "2"
clap = { version = "4", features = ["derive"] }
anyhow = "1"

[dev-dependencies]
```

- [ ] **Step 2: Write placeholder main.rs**

```rust
fn main() {
    println!("typst-diff");
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build
```

Expected: compiles without errors. Fetching crates on first run will take 1-2 minutes.

- [ ] **Step 4: Commit**

```bash
git init
git add Cargo.toml src/main.rs
git commit -m "chore: project scaffolding"
```

---

## Task 2: World implementation

**Files:**
- Create: `src/world.rs`
- Modify: `src/main.rs` (add `mod world;`)

The `World` trait provides the Typst compiler with file system access and fonts.
`source()` reads `.typ` source files; `file()` reads binary assets (images etc.);
`font()` serves fonts via typst-kit's `FontSearcher`. Paths in Typst are virtual
(e.g. `/main.typ`) and resolved relative to a root directory on disk.

- [ ] **Step 1: Write the failing test**

Add to the bottom of `src/world.rs` (create the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;  // add tempfile = "3" to [dev-dependencies]

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
```

Add to `[dev-dependencies]` in Cargo.toml:
```toml
tempfile = "3"
```

- [ ] **Step 2: Run test to confirm it fails**

```bash
cargo test world::tests
```

Expected: compile error — `SystemWorld` not defined.

- [ ] **Step 3: Implement SystemWorld**

Write `src/world.rs`:

```rust
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
use typst_kit::fonts::{FontSearcher, Fonts};

pub struct SystemWorld {
    root: PathBuf,
    main: FileId,
    library: LazyHash<Library>,
    fonts: Fonts,
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
            fonts,
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
        &self.fonts.book
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
            .map(Bytes::from)
            .map_err(|_| FileError::NotFound(path.clone()));
        cache.insert(id, result.clone());
        result
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.fonts[index].get()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}
```

Add `mod world;` and `pub use world::SystemWorld;` to `src/main.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test world::tests
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/world.rs src/main.rs Cargo.toml
git commit -m "feat: World trait implementation with filesystem + font support"
```

---

## Task 3: Eval wrapper

**Files:**
- Create: `src/eval.rs`
- Modify: `src/main.rs` (add `mod eval;`)

Wraps `typst_eval::eval` to return a `Content` tree from a `World`.

- [ ] **Step 1: Write the failing test**

Add to `src/eval.rs`:

```rust
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
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test eval::tests
```

Expected: compile error — `eval_to_content` not defined.

- [ ] **Step 3: Implement eval_to_content**

Write `src/eval.rs`:

```rust
use anyhow::Result;
use typst::comemo::Track;
use typst::engine::{Route, Sink, Traced};
use typst::foundations::Content;
use typst::World;
use typst::ROUTINES;

pub fn eval_to_content(world: &dyn World) -> Result<Content> {
    let source = world.source(world.main())
        .map_err(|e| anyhow::anyhow!("cannot read main source: {e:?}"))?;
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
```

Add `mod eval; pub use eval::eval_to_content;` to `src/main.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test eval::tests
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/eval.rs src/main.rs
git commit -m "feat: eval wrapper producing Content tree from World"
```

---

## Task 4: Block extraction

**Files:**
- Create: `src/diff.rs`
- Modify: `src/main.rs` (add `mod diff;`)

Segments a flat `Content` tree into a `Vec<Content>` where each entry is one
"block" (paragraph, heading, code block, display equation, or other atomic node).

- [ ] **Step 1: Write the failing test**

Add to `src/diff.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Content, SequenceElem};
    use typst::model::{HeadingElem, ParbreakElem};
    use typst::text::TextElem;

    fn seq(items: impl IntoIterator<Item = Content>) -> Content {
        Content::sequence(items)
    }

    #[test]
    fn two_paragraphs_become_two_blocks() {
        let content = seq([
            TextElem::packed("First"),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Second"),
        ]);
        let blocks = extract_blocks(&content);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn heading_is_own_block() {
        let content = seq([
            Content::new(HeadingElem::new(TextElem::packed("Title"), 1.into())),
            Content::new(ParbreakElem::new()),
            TextElem::packed("Body"),
        ]);
        let blocks = extract_blocks(&content);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].is::<HeadingElem>());
    }

    #[test]
    fn trailing_content_without_parbreak_becomes_block() {
        let content = seq([
            TextElem::packed("Only paragraph"),
        ]);
        let blocks = extract_blocks(&content);
        assert_eq!(blocks.len(), 1);
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::two_paragraphs
```

Expected: compile error — `extract_blocks` not defined.

- [ ] **Step 3: Implement extract_blocks**

Write `src/diff.rs`:

```rust
use typst::foundations::{Content, SequenceElem, StyleChain};
use typst::math::EquationElem;
use typst::model::{HeadingElem, ParbreakElem};
use typst::text::{RawElem, SpaceElem};

/// Segment a Content tree into block-level units.
///
/// Paragraph blocks are sequences of inline elements between ParbreakElems.
/// HeadingElems, RawElems, and display EquationElems are their own blocks.
/// Unknown block-level nodes (figures, lists, etc.) flush the current paragraph
/// and become single-item atomic blocks.
pub fn extract_blocks(content: &Content) -> Vec<Content> {
    let children: Vec<Content> = if let Some(seq) = content.to_packed::<SequenceElem>() {
        seq.children.clone()
    } else {
        vec![content.clone()]
    };

    let mut blocks: Vec<Content> = Vec::new();
    let mut para: Vec<Content> = Vec::new();

    let flush = |para: &mut Vec<Content>, blocks: &mut Vec<Content>| {
        let nonempty = para.iter().any(|c| !c.is::<SpaceElem>() && !c.is::<ParbreakElem>());
        if nonempty {
            blocks.push(Content::sequence(para.drain(..)));
        } else {
            para.clear();
        }
    };

    for child in children {
        if child.is::<ParbreakElem>() {
            flush(&mut para, &mut blocks);
        } else if child.is::<HeadingElem>() || child.is::<RawElem>() || is_display_equation(&child) {
            flush(&mut para, &mut blocks);
            blocks.push(child);
        } else if is_known_inline(&child) {
            para.push(child);
        } else {
            // Unknown node: treat as atomic block if para is empty, else flush first
            if para.is_empty() {
                blocks.push(child);
            } else {
                flush(&mut para, &mut blocks);
                blocks.push(child);
            }
        }
    }
    flush(&mut para, &mut blocks);
    blocks
}

fn is_display_equation(c: &Content) -> bool {
    c.to_packed::<EquationElem>()
        .is_some_and(|eq| eq.block.get(StyleChain::default()))
}

fn is_known_inline(c: &Content) -> bool {
    use typst::model::{EmphElem, LinkElem, StrongElem};
    use typst::text::{SmartQuoteElem, TextElem};
    c.is::<TextElem>()
        || c.is::<SpaceElem>()
        || c.is::<StrongElem>()
        || c.is::<EmphElem>()
        || c.is::<LinkElem>()
        || c.is::<SmartQuoteElem>()
        || (c.is::<EquationElem>() && !is_display_equation(c))
}
```

Add `mod diff; pub use diff::extract_blocks;` to `src/main.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test diff::tests
```

Expected: all three block extraction tests pass. Fix `HeadingElem::new` signature
if needed — check with `cargo build` first and adjust argument order per compiler
error. In 0.14.x: `HeadingElem::new(body: Content, level: NonZeroUsize)`.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs src/main.rs
git commit -m "feat: block extraction from Content tree"
```

---

## Task 5: Word extraction

**Files:**
- Modify: `src/diff.rs`

Segments a block's inline content into a flat `Vec<Token>` for word-level diffing.
`TextElem` nodes are split on whitespace; everything else (Strong, Emph, inline
equations, etc.) becomes one atomic token. Tokens carry their original `Content`
for faithful reconstruction, and a `text` key for equality comparison.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/diff.rs`:

```rust
    #[test]
    fn text_elem_splits_into_words() {
        let content = TextElem::packed("hello world foo");
        let tokens = extract_words(&content);
        let texts: Vec<&str> = tokens.iter().map(|t| t.text.as_str()).collect();
        assert!(texts.contains(&"hello"));
        assert!(texts.contains(&"world"));
        assert!(texts.contains(&"foo"));
    }

    #[test]
    fn strong_elem_is_atomic_token() {
        use typst::model::StrongElem;
        let strong = Content::new(StrongElem::new(TextElem::packed("bold")));
        let para = seq([TextElem::packed("before "), strong, TextElem::packed(" after")]);
        let tokens = extract_words(&para);
        assert!(tokens.iter().any(|t| t.text == "bold" || t.content.is::<StrongElem>()));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::text_elem_splits
```

Expected: compile error — `extract_words`, `Token` not defined.

- [ ] **Step 3: Implement Token and extract_words**

Add to `src/diff.rs` (before the `tests` module):

```rust
use std::hash::{Hash, Hasher};

/// A single diffable unit: either a word/space from a TextElem, or an atomic
/// inline node that cannot be split further.
#[derive(Clone)]
pub struct Token {
    /// Text key used for equality comparison during diff.
    pub text: String,
    /// The original Content node for reconstruction.
    pub content: Content,
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}
impl Eq for Token {}
impl Hash for Token {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

/// Extract a flat list of tokens from a block's inline content.
pub fn extract_words(content: &Content) -> Vec<Token> {
    let mut tokens = Vec::new();
    collect_tokens(content, &mut tokens);
    tokens
}

fn collect_tokens(content: &Content, out: &mut Vec<Token>) {
    use typst::text::TextElem;

    if let Some(seq) = content.to_packed::<SequenceElem>() {
        for child in &seq.children {
            collect_tokens(child, out);
        }
    } else if let Some(text_elem) = content.to_packed::<TextElem>() {
        // Split on whitespace boundaries into word/space runs
        let s = text_elem.text.as_str();
        let mut start = 0;
        let mut in_space = s.chars().next().map(|c| c.is_whitespace()).unwrap_or(false);
        for (i, ch) in s.char_indices() {
            if ch.is_whitespace() != in_space {
                let slice = &s[start..i];
                if !slice.is_empty() {
                    out.push(Token {
                        text: slice.to_string(),
                        content: TextElem::packed(slice),
                    });
                }
                start = i;
                in_space = ch.is_whitespace();
            }
        }
        let tail = &s[start..];
        if !tail.is_empty() {
            out.push(Token {
                text: tail.to_string(),
                content: TextElem::packed(tail),
            });
        }
    } else if content.is::<SpaceElem>() {
        out.push(Token { text: " ".to_string(), content: content.clone() });
    } else {
        // Atomic inline node: use plain_text() as the key
        out.push(Token {
            text: content.plain_text().to_string(),
            content: content.clone(),
        });
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test diff::tests
```

Expected: all tests pass including the two new word extraction tests.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat: word-level token extraction from inline Content"
```

---

## Task 6: Block-level LCS diff

**Files:**
- Modify: `src/diff.rs`

Run an LCS diff on two sequences of blocks and produce `BlockOp`s.
Initially produces only `Equal`, `Delete`, and `Insert` — no pairing yet.

- [ ] **Step 1: Write the failing test**

Add to `tests` module in `src/diff.rs`:

```rust
    #[test]
    fn identical_blocks_all_equal() {
        let a = vec![TextElem::packed("Hello"), TextElem::packed("World")];
        let b = a.clone();
        let ops = diff_blocks_raw(&a, &b);
        assert!(ops.iter().all(|op| matches!(op, BlockOp::Equal(_, _))));
    }

    #[test]
    fn added_block_detected() {
        let old = vec![TextElem::packed("Only old")];
        let new = vec![TextElem::packed("Only old"), TextElem::packed("New block")];
        let ops = diff_blocks_raw(&old, &new);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Insert(_))));
    }

    #[test]
    fn deleted_block_detected() {
        let old = vec![TextElem::packed("A"), TextElem::packed("B")];
        let new = vec![TextElem::packed("A")];
        let ops = diff_blocks_raw(&old, &new);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Delete(_))));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::identical_blocks
```

Expected: compile error — `BlockOp`, `diff_blocks_raw` not defined.

- [ ] **Step 3: Implement BlockOp and diff_blocks_raw**

Add to `src/diff.rs`:

```rust
use std::hash::Hash;
use similar::{Algorithm, capture_diff_slices, DiffOp};

/// Wrapper to provide Eq + Hash for Content (which is PartialEq + Hash).
#[derive(Clone)]
struct HashableContent(Content);

impl PartialEq for HashableContent {
    fn eq(&self, other: &Self) -> bool { self.0 == other.0 }
}
impl Eq for HashableContent {}
impl Hash for HashableContent {
    fn hash<H: Hasher>(&self, state: &mut H) { self.0.hash(state) }
}

pub enum BlockOp {
    Equal(Content, Content),  // (old, new) — identical
    Delete(Content),
    Insert(Content),
    Replace(Content, Content), // (old, new) — similar, to be word-diffed
}

/// Raw block diff — no similarity pairing. Replace is not produced here.
pub fn diff_blocks_raw(old: &[Content], new: &[Content]) -> Vec<BlockOp> {
    let old_h: Vec<HashableContent> = old.iter().cloned().map(HashableContent).collect();
    let new_h: Vec<HashableContent> = new.iter().cloned().map(HashableContent).collect();

    let ops = capture_diff_slices(Algorithm::Myers, &old_h, &new_h);
    let mut result = Vec::new();

    for op in ops {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for i in 0..len {
                    result.push(BlockOp::Equal(old[old_index + i].clone(), new[new_index + i].clone()));
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for i in 0..old_len {
                    result.push(BlockOp::Delete(old[old_index + i].clone()));
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for i in 0..new_len {
                    result.push(BlockOp::Insert(new[new_index + i].clone()));
                }
            }
        }
    }
    result
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test diff::tests
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat: block-level LCS diff producing Equal/Delete/Insert ops"
```

---

## Task 7: Block similarity matching

**Files:**
- Modify: `src/diff.rs`

Scan `diff_blocks_raw` output for adjacent Delete+Insert groups ("edit zones").
Within each edit zone, pair old and new blocks by minimum edit distance on
`plain_text()`. A pair is accepted if similarity ≥ 0.3; accepted pairs become
`BlockOp::Replace` (for word-level diff); rejected pairs remain as Delete+Insert.

- [ ] **Step 1: Write the failing test**

Add to `tests` module in `src/diff.rs`:

```rust
    #[test]
    fn similar_blocks_become_replace() {
        let old = vec![TextElem::packed("The quick brown fox jumps.")];
        let new = vec![TextElem::packed("The quick brown fox leaps.")];
        let raw = diff_blocks_raw(&old, &new);
        let ops = match_edit_zones(raw);
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Replace(_, _))));
    }

    #[test]
    fn dissimilar_blocks_stay_delete_insert() {
        let old = vec![TextElem::packed("Completely unrelated old content.")];
        let new = vec![TextElem::packed("xyz")];
        let raw = diff_blocks_raw(&old, &new);
        let ops = match_edit_zones(raw);
        // Both old content is long and new is very short: similarity < 0.3
        // At least one delete and one insert
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Delete(_))));
        assert!(ops.iter().any(|op| matches!(op, BlockOp::Insert(_))));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::similar_blocks
```

Expected: compile error — `match_edit_zones` not defined.

- [ ] **Step 3: Implement match_edit_zones**

Add to `src/diff.rs`:

```rust
/// Pair adjacent Delete+Insert groups by similarity, converting matched pairs
/// to Replace ops.
pub fn match_edit_zones(ops: Vec<BlockOp>) -> Vec<BlockOp> {
    let mut result: Vec<BlockOp> = Vec::new();
    let mut i = 0;

    while i < ops.len() {
        // Collect a contiguous run of Deletes followed by Inserts
        let del_start = i;
        while i < ops.len() && matches!(ops[i], BlockOp::Delete(_)) {
            i += 1;
        }
        let ins_start = i;
        while i < ops.len() && matches!(ops[i], BlockOp::Insert(_)) {
            i += 1;
        }

        let deletes: Vec<Content> = ops[del_start..ins_start]
            .iter()
            .map(|op| match op { BlockOp::Delete(c) => c.clone(), _ => unreachable!() })
            .collect();
        let inserts: Vec<Content> = ops[ins_start..i]
            .iter()
            .map(|op| match op { BlockOp::Insert(c) => c.clone(), _ => unreachable!() })
            .collect();

        if deletes.is_empty() && inserts.is_empty() {
            // Neither — emit whatever was before the loop iteration
            result.push(ops[del_start].clone());  // shouldn't happen, but safe
            i = del_start + 1;
            continue;
        }

        if deletes.is_empty() {
            result.extend(inserts.into_iter().map(BlockOp::Insert));
        } else if inserts.is_empty() {
            result.extend(deletes.into_iter().map(BlockOp::Delete));
        } else {
            // Greedily pair each delete with its most similar insert
            let mut used_inserts = vec![false; inserts.len()];
            let mut paired: Vec<(Content, Content)> = Vec::new();
            let mut unpaired_deletes: Vec<Content> = Vec::new();

            for del in &deletes {
                let del_text = del.plain_text();
                let best = inserts.iter().enumerate()
                    .filter(|(j, _)| !used_inserts[*j])
                    .map(|(j, ins)| {
                        let ins_text = ins.plain_text();
                        (j, similarity(del_text.as_str(), ins_text.as_str()))
                    })
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

                if let Some((j, sim)) = best {
                    if sim >= 0.3 {
                        used_inserts[j] = true;
                        paired.push((del.clone(), inserts[j].clone()));
                        continue;
                    }
                }
                unpaired_deletes.push(del.clone());
            }

            let unpaired_inserts: Vec<Content> = inserts.into_iter().enumerate()
                .filter(|(j, _)| !used_inserts[*j])
                .map(|(_, c)| c)
                .collect();

            result.extend(unpaired_deletes.into_iter().map(BlockOp::Delete));
            result.extend(unpaired_inserts.into_iter().map(BlockOp::Insert));
            result.extend(paired.into_iter().map(|(o, n)| BlockOp::Replace(o, n)));
        }
    }
    result
}

fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let max_len = a.len().max(b.len());
    let dist = edit_distance(a, b);
    1.0 - dist as f64 / max_len as f64
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m { dp[i][0] = i; }
    for j in 0..=n { dp[0][j] = j; }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i-1] == b[j-1] {
                dp[i-1][j-1]
            } else {
                1 + dp[i-1][j].min(dp[i][j-1]).min(dp[i-1][j-1])
            };
        }
    }
    dp[m][n]
}
```

Also need to add `#[derive(Clone)]` to `BlockOp`:

```rust
#[derive(Clone)]
pub enum BlockOp { ... }
```

- [ ] **Step 4: Fix the loop edge case**

The loop has a subtle bug when neither `deletes` nor `inserts` collect — replace
the early-continue block with correct flow. The issue: the `while` loops only
advance `i` for Delete/Insert runs, so non-Delete/non-Insert ops (Equal/Replace)
will be handled by neither inner while. Fix by restructuring as a fold over
contiguous Delete/Insert groups:

```rust
pub fn match_edit_zones(ops: Vec<BlockOp>) -> Vec<BlockOp> {
    let mut result: Vec<BlockOp> = Vec::new();
    let mut i = 0;
    let n = ops.len();

    while i < n {
        match &ops[i] {
            BlockOp::Equal(_, _) | BlockOp::Replace(_, _) => {
                result.push(ops[i].clone());
                i += 1;
            }
            BlockOp::Delete(_) | BlockOp::Insert(_) => {
                // Collect contiguous delete+insert run
                let del_start = i;
                while i < n && matches!(&ops[i], BlockOp::Delete(_)) { i += 1; }
                let ins_start = i;
                while i < n && matches!(&ops[i], BlockOp::Insert(_)) { i += 1; }

                let deletes: Vec<Content> = ops[del_start..ins_start].iter()
                    .map(|op| match op { BlockOp::Delete(c) => c.clone(), _ => unreachable!() })
                    .collect();
                let inserts: Vec<Content> = ops[ins_start..i].iter()
                    .map(|op| match op { BlockOp::Insert(c) => c.clone(), _ => unreachable!() })
                    .collect();

                pair_edit_zone(deletes, inserts, &mut result);
            }
        }
    }
    result
}

fn pair_edit_zone(deletes: Vec<Content>, inserts: Vec<Content>, out: &mut Vec<BlockOp>) {
    if deletes.is_empty() {
        out.extend(inserts.into_iter().map(BlockOp::Insert));
        return;
    }
    if inserts.is_empty() {
        out.extend(deletes.into_iter().map(BlockOp::Delete));
        return;
    }

    let mut used_inserts = vec![false; inserts.len()];
    let mut pairs: Vec<(Content, Content)> = Vec::new();
    let mut unpaired_del: Vec<Content> = Vec::new();

    for del in &deletes {
        let del_text = del.plain_text();
        let best = inserts.iter().enumerate()
            .filter(|(j, _)| !used_inserts[*j])
            .map(|(j, ins)| (j, similarity(del_text.as_str(), ins.plain_text().as_str())))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        match best {
            Some((j, sim)) if sim >= 0.3 => {
                used_inserts[j] = true;
                pairs.push((del.clone(), inserts[j].clone()));
            }
            _ => unpaired_del.push(del.clone()),
        }
    }

    let unpaired_ins: Vec<Content> = inserts.into_iter().enumerate()
        .filter(|(j, _)| !used_inserts[*j])
        .map(|(_, c)| c)
        .collect();

    out.extend(unpaired_del.into_iter().map(BlockOp::Delete));
    out.extend(unpaired_ins.into_iter().map(BlockOp::Insert));
    out.extend(pairs.into_iter().map(|(o, n)| BlockOp::Replace(o, n)));
}
```

Remove the old `match_edit_zones` implementation and replace with this one.

- [ ] **Step 5: Run tests**

```bash
cargo test diff::tests
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/diff.rs
git commit -m "feat: block similarity matching — pairs adjacent delete+insert by edit distance"
```

---

## Task 8: Word-level diff

**Files:**
- Modify: `src/diff.rs`

Diff two `Vec<Token>` sequences and produce `WordOp`s. Adjacent same-tag tokens
are coalesced.

- [ ] **Step 1: Write the failing test**

Add to `tests` module in `src/diff.rs`:

```rust
    #[test]
    fn changed_word_produces_delete_and_insert() {
        let old = extract_words(&TextElem::packed("The quick brown fox jumps."));
        let new = extract_words(&TextElem::packed("The quick brown fox leaps."));
        let ops = diff_words(&old, &new);
        let has_delete = ops.iter().any(|op| matches!(op, WordOp::Delete(_)));
        let has_insert = ops.iter().any(|op| matches!(op, WordOp::Insert(_)));
        assert!(has_delete, "expected a delete op");
        assert!(has_insert, "expected an insert op");
    }

    #[test]
    fn identical_words_all_equal() {
        let tokens = extract_words(&TextElem::packed("Hello world."));
        let ops = diff_words(&tokens, &tokens.clone());
        assert!(ops.iter().all(|op| matches!(op, WordOp::Equal(_))));
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::changed_word
```

Expected: compile error — `WordOp`, `diff_words` not defined.

- [ ] **Step 3: Implement WordOp and diff_words**

Add to `src/diff.rs`:

```rust
#[derive(Clone, Debug)]
pub enum WordOp {
    Equal(Vec<Token>),
    Delete(Vec<Token>),
    Insert(Vec<Token>),
}

/// Diff two token sequences, coalescing adjacent same-tag ops.
pub fn diff_words(old: &[Token], new: &[Token]) -> Vec<WordOp> {
    let raw_ops = capture_diff_slices(Algorithm::Myers, old, new);
    let mut result: Vec<WordOp> = Vec::new();

    for op in raw_ops {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                let tokens = old[old_index..old_index + len].to_vec();
                coalesce(&mut result, WordOp::Equal(tokens));
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                let tokens = old[old_index..old_index + old_len].to_vec();
                coalesce(&mut result, WordOp::Delete(tokens));
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                let tokens = new[new_index..new_index + new_len].to_vec();
                coalesce(&mut result, WordOp::Insert(tokens));
            }
        }
    }
    result
}

fn coalesce(ops: &mut Vec<WordOp>, next: WordOp) {
    match (ops.last_mut(), &next) {
        (Some(WordOp::Equal(v)), WordOp::Equal(w)) => v.extend_from_slice(w),
        (Some(WordOp::Delete(v)), WordOp::Delete(w)) => v.extend_from_slice(w),
        (Some(WordOp::Insert(v)), WordOp::Insert(w)) => v.extend_from_slice(w),
        _ => ops.push(next),
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test diff::tests
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat: word-level LCS diff with coalescing"
```

---

## Task 9: Full diff pipeline

**Files:**
- Modify: `src/diff.rs`

Combine `extract_blocks` → `diff_blocks_raw` → `match_edit_zones` → `diff_words`
into a single `diff_content` function returning a `DiffResult`.

- [ ] **Step 1: Write the failing test**

Add to `tests` module in `src/diff.rs`:

```rust
    #[test]
    fn diff_content_detects_word_change() {
        let old = seq([
            TextElem::packed("The fox jumps."),
        ]);
        let new = seq([
            TextElem::packed("The fox leaps."),
        ]);
        let result = diff_content(&old, &new);
        let has_word_change = result.block_ops.iter().any(|op| match op {
            DiffResultOp::Modified(word_ops) => word_ops.iter().any(|w| {
                matches!(w, WordOp::Delete(_)) || matches!(w, WordOp::Insert(_))
            }),
            _ => false,
        });
        assert!(has_word_change);
    }
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test diff::tests::diff_content
```

Expected: compile error — `diff_content`, `DiffResult`, `DiffResultOp` not defined.

- [ ] **Step 3: Implement DiffResult and diff_content**

Add to `src/diff.rs`:

```rust
pub enum DiffResultOp {
    Equal(Content),              // unchanged block (from new document)
    Deleted(Content),            // whole block removed
    Inserted(Content),           // whole block added
    Modified(Vec<WordOp>),       // word-level diff of a matched pair
}

pub struct DiffResult {
    pub block_ops: Vec<DiffResultOp>,
}

pub fn diff_content(old: &Content, new: &Content) -> DiffResult {
    let old_blocks = extract_blocks(old);
    let new_blocks = extract_blocks(new);
    let raw = diff_blocks_raw(&old_blocks, &new_blocks);
    let matched = match_edit_zones(raw);

    let block_ops = matched.into_iter().map(|op| match op {
        BlockOp::Equal(_, new_block) => DiffResultOp::Equal(new_block),
        BlockOp::Delete(old_block) => DiffResultOp::Deleted(old_block),
        BlockOp::Insert(new_block) => DiffResultOp::Inserted(new_block),
        BlockOp::Replace(old_block, new_block) => {
            let old_tokens = extract_words(&old_block);
            let new_tokens = extract_words(&new_block);
            DiffResultOp::Modified(diff_words(&old_tokens, &new_tokens))
        }
    }).collect();

    DiffResult { block_ops }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test diff::tests
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/diff.rs
git commit -m "feat: full diff pipeline — diff_content combining blocks and words"
```

---

## Task 10: Annotation

**Files:**
- Create: `src/annotate.rs`
- Modify: `src/main.rs` (add `mod annotate;`)

Converts a `DiffResult` into an annotated `Content` tree by wrapping
deleted/inserted material in colored + struck text.

- [ ] **Step 1: Write the failing test**

Create `src/annotate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::TextElem;
    use typst::foundations::Content;
    use crate::diff::{DiffResult, DiffResultOp, WordOp, Token};

    fn word_token(s: &str) -> Token {
        Token { text: s.to_string(), content: TextElem::packed(s) }
    }

    #[test]
    fn inserted_block_wrapped_green() {
        let result = DiffResult {
            block_ops: vec![
                DiffResultOp::Inserted(TextElem::packed("New paragraph")),
            ],
        };
        let content = build_annotated_content(&result);
        // Should be non-empty content
        assert!(!content.is_empty());
    }

    #[test]
    fn modified_block_contains_delete_and_insert() {
        let result = DiffResult {
            block_ops: vec![
                DiffResultOp::Modified(vec![
                    WordOp::Equal(vec![word_token("The ")]),
                    WordOp::Delete(vec![word_token("old")]),
                    WordOp::Insert(vec![word_token("new")]),
                    WordOp::Equal(vec![word_token(" text.")]),
                ]),
            ],
        };
        let content = build_annotated_content(&result);
        assert!(!content.is_empty());
        // Verify it contains StrikeElem (for deletion) somewhere in tree
        use typst::text::StrikeElem;
        let mut found_strike = false;
        let _ = content.traverse::<_, ()>(&mut |c| {
            if c.is::<StrikeElem>() { found_strike = true; }
            std::ops::ControlFlow::Continue(())
        });
        assert!(found_strike, "expected StrikeElem for deleted word");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test annotate::tests
```

Expected: compile error — `build_annotated_content` not defined.

- [ ] **Step 3: Implement build_annotated_content**

Write `src/annotate.rs`:

```rust
use typst::foundations::Content;
use typst::text::{StrikeElem, TextElem};
use typst::visualize::Color;

use crate::diff::{DiffResult, DiffResultOp, WordOp};

const GREEN: Color = Color::Rgb(typst::visualize::ColorSpace::Oklab,
    // Use from_u8 instead since Rgb constructor is not const
    // This const is a placeholder — initialized in the function
    Color::from_u8(0, 180, 0, 255)  // not valid const expr; see impl below
);
```

The `Color::from_u8` constructor is not `const`, so define colors as lazy
statics or inline. Write the actual implementation as:

```rust
use typst::foundations::Content;
use typst::text::{StrikeElem, TextElem};
use typst::visualize::Color;

use crate::diff::{DiffResult, DiffResultOp, WordOp};

fn green() -> Color { Color::from_u8(0, 180, 0, 255) }
fn red()   -> Color { Color::from_u8(220, 0, 0, 255) }

pub fn build_annotated_content(result: &DiffResult) -> Content {
    let mut blocks: Vec<Content> = Vec::new();

    for op in &result.block_ops {
        match op {
            DiffResultOp::Equal(c) => blocks.push(c.clone()),

            DiffResultOp::Inserted(c) => {
                blocks.push(c.clone().styled(TextElem::fill.set(green().into())));
            }

            DiffResultOp::Deleted(c) => {
                let colored = c.clone().styled(TextElem::fill.set(red().into()));
                blocks.push(Content::new(StrikeElem::new(colored)));
            }

            DiffResultOp::Modified(word_ops) => {
                let mut inline: Vec<Content> = Vec::new();
                for wop in word_ops {
                    match wop {
                        WordOp::Equal(tokens) => {
                            for t in tokens { inline.push(t.content.clone()); }
                        }
                        WordOp::Insert(tokens) => {
                            let joined = Content::sequence(
                                tokens.iter().map(|t| t.content.clone())
                            );
                            inline.push(joined.styled(TextElem::fill.set(green().into())));
                        }
                        WordOp::Delete(tokens) => {
                            let joined = Content::sequence(
                                tokens.iter().map(|t| t.content.clone())
                            );
                            let colored = joined.styled(TextElem::fill.set(red().into()));
                            inline.push(Content::new(StrikeElem::new(colored)));
                        }
                    }
                }
                blocks.push(Content::sequence(inline));
            }
        }

        // Add paragraph break between blocks
        use typst::model::ParbreakElem;
        blocks.push(Content::new(ParbreakElem::new()));
    }

    Content::sequence(blocks)
}
```

Add `mod annotate; pub use annotate::build_annotated_content;` to `src/main.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test annotate::tests
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/annotate.rs src/main.rs
git commit -m "feat: annotate diff result into colored+struck Content tree"
```

---

## Task 11: Render to PDF

**Files:**
- Create: `src/render.rs`
- Modify: `src/main.rs` (add `mod render;`)

Takes an annotated `Content` and a `World` (for fonts/assets) and produces PDF
bytes. Uses one layout iteration — sufficient for documents without counters.

- [ ] **Step 1: Write the failing test**

Create `src/render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use typst::text::TextElem;
    use crate::world::SystemWorld;

    #[test]
    fn renders_simple_content_to_pdf() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.typ"), "").unwrap();
        let world = SystemWorld::new(dir.path().join("main.typ")).unwrap();
        let content = TextElem::packed("Hello, diff world.");
        let pdf = render_to_pdf(&content, &world).unwrap();
        // PDF files start with "%PDF"
        assert!(pdf.starts_with(b"%PDF"), "expected PDF output");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test render::tests
```

Expected: compile error — `render_to_pdf` not defined.

- [ ] **Step 3: Implement render_to_pdf**

Write `src/render.rs`:

```rust
use anyhow::Result;
use typst::comemo::Track;
use typst::engine::{Engine, Route, Sink, Traced};
use typst::foundations::{Content, StyleChain};
use typst::introspection::Introspector;
use typst::layout::PagedDocument;
use typst::World;
use typst::ROUTINES;
use typst_pdf::{PdfOptions, pdf};

pub fn render_to_pdf(content: &Content, world: &dyn World) -> Result<Vec<u8>> {
    let library = world.library();
    let styles = StyleChain::new(&library.styles);

    let introspector = Introspector::default();
    let constraint = typst::comemo::Constraint::new();
    let mut sink = Sink::new();
    let traced = Traced::default();

    let mut engine = Engine {
        routines: &ROUTINES,
        world: world.track(),
        introspector: introspector.track_with(&constraint),
        traced: traced.track(),
        sink: sink.track_mut(),
        route: Route::default(),
    };

    let document = typst_layout::layout_document(&mut engine, content, styles)
        .map_err(|errs| anyhow::anyhow!("layout error: {} diagnostic(s)", errs.len()))?;

    pdf(&document, &PdfOptions::default())
        .map_err(|errs| anyhow::anyhow!("pdf export error: {} diagnostic(s)", errs.len()))
}
```

Add `mod render; pub use render::render_to_pdf;` to `src/main.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test render::tests
```

Expected: test passes and PDF bytes start with `%PDF`.

- [ ] **Step 5: Commit**

```bash
git add src/render.rs src/main.rs
git commit -m "feat: render annotated Content to PDF via typst_layout + typst_pdf"
```

---

## Task 12: CLI wiring

**Files:**
- Modify: `src/main.rs`

Wire all modules together under a `clap`-based CLI.

- [ ] **Step 1: Write the complete main.rs**

```rust
mod annotate;
mod diff;
mod eval;
mod render;
mod world;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use annotate::build_annotated_content;
use diff::diff_content;
use eval::eval_to_content;
use render::render_to_pdf;
use world::SystemWorld;

#[derive(Parser)]
#[command(name = "typst-diff", about = "Diff two Typst documents and produce a PDF")]
struct Args {
    /// Path to the old document entry point (e.g. old/main.typ)
    old: PathBuf,
    /// Path to the new document entry point (e.g. new/main.typ)
    new: PathBuf,
    /// Output PDF path
    #[arg(short, long, default_value = "diff.pdf")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    eprintln!("Loading old document: {}", args.old.display());
    let old_world = SystemWorld::new(&args.old)
        .with_context(|| format!("failed to load old document {:?}", args.old))?;

    eprintln!("Loading new document: {}", args.new.display());
    let new_world = SystemWorld::new(&args.new)
        .with_context(|| format!("failed to load new document {:?}", args.new))?;

    eprintln!("Evaluating old document...");
    let old_content = eval_to_content(&old_world)
        .context("failed to evaluate old document")?;

    eprintln!("Evaluating new document...");
    let new_content = eval_to_content(&new_world)
        .context("failed to evaluate new document")?;

    eprintln!("Diffing...");
    let diff_result = diff_content(&old_content, &new_content);

    eprintln!("Annotating...");
    let annotated = build_annotated_content(&diff_result);

    eprintln!("Rendering to PDF...");
    let pdf_bytes = render_to_pdf(&annotated, &new_world)
        .context("failed to render PDF")?;

    std::fs::write(&args.output, &pdf_bytes)
        .with_context(|| format!("failed to write {:?}", args.output))?;

    eprintln!("Written to {}", args.output.display());
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build
```

Expected: compiles cleanly (warnings OK).

- [ ] **Step 3: Smoke test with fixture files**

Create `tests/fixtures/simple_old.typ`:
```typst
= Introduction

The quick brown fox jumps over the lazy dog.

= Conclusion

This is the end of the document.
```

Create `tests/fixtures/simple_new.typ`:
```typst
= Introduction

The quick brown fox leaps over the lazy dog.

= Conclusion

This is the final section of the document.
```

Run:
```bash
cargo run -- tests/fixtures/simple_old.typ tests/fixtures/simple_new.typ -o /tmp/test_diff.pdf
```

Expected: exits 0, writes `/tmp/test_diff.pdf`. Open the PDF — `jumps` should
appear in red strikethrough, `leaps` in green; `end` in red strikethrough,
`final section` in green.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs tests/fixtures/simple_old.typ tests/fixtures/simple_new.typ
git commit -m "feat: CLI wiring — typst-diff old.typ new.typ [-o diff.pdf]"
```

---

## Task 13: Integration test

**Files:**
- Create: `tests/integration.rs`
- Create: `tests/fixtures/multifile_old/main.typ`, `tests/fixtures/multifile_old/chapter.typ`
- Create: `tests/fixtures/multifile_new/main.typ`, `tests/fixtures/multifile_new/chapter.typ`

End-to-end test verifying the binary produces valid PDF output for single-file
and multi-file (with `#include`) documents.

- [ ] **Step 1: Create multi-file fixtures**

`tests/fixtures/multifile_old/main.typ`:
```typst
#include "chapter.typ"
```

`tests/fixtures/multifile_old/chapter.typ`:
```typst
= Chapter One

The old chapter content here.
```

`tests/fixtures/multifile_new/main.typ`:
```typst
#include "chapter.typ"
```

`tests/fixtures/multifile_new/chapter.typ`:
```typst
= Chapter One

The new chapter content here.
```

- [ ] **Step 2: Write integration tests**

Create `tests/integration.rs`:

```rust
use std::path::PathBuf;
use typst::text::TextElem;

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
    let result = typst_diff::diff::diff::diff_content(&old, &new);
    let annotated = typst_diff::build_annotated_content(&result);
    let pdf = typst_diff::render_to_pdf(&annotated, &new_world).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}
```

Add to `src/main.rs` so the crate is usable as a library in tests:

```rust
pub mod annotate;
pub mod diff;
pub mod eval;
pub mod render;
pub mod world;

pub use annotate::build_annotated_content;
pub use eval::eval_to_content;
pub use render::render_to_pdf;
```

(The `main` function remains; `pub mod` declarations allow `tests/integration.rs`
to use the crate's internals.)

- [ ] **Step 3: Fix import in integration test**

The path `typst_diff::diff::diff::diff_content` is a typo — correct to
`typst_diff::diff::diff_content`.

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

Expected: all unit tests and both integration tests pass. Font loading may take
a few seconds on the first run.

- [ ] **Step 5: Final commit**

```bash
git add tests/integration.rs tests/fixtures/
git commit -m "test: integration tests for single-file and multi-file diff"
```

---

## Self-Review Notes

- **Spec coverage:** All sections of the design spec are covered:
  - Two-level diff: Tasks 4–9
  - Annotation: Task 10
  - World + eval: Tasks 2–3
  - PDF render: Task 11
  - CLI: Task 12
  - Block similarity (0.3 threshold): Task 7
- **Similarity threshold 0.3** is hardcoded — spec says "0.3", plan matches.
- **One layout iteration** in render.rs — spec says "sufficient for docs without
  counters". This is intentional for v1.
- **`SmartQuoteElem`** in `is_known_inline`: verify this type is accessible at
  `typst::text::SmartQuoteElem` before Task 4 compiles; if not, remove it from
  the whitelist (unknown inlines fall through to the atomic-block branch safely).
- **`HeadingElem::new` signature**: in 0.14.x may require a `NonZeroUsize` for
  level — the test in Task 4 Step 3 notes this; adjust to match the actual
  constructor if tests fail to compile.
