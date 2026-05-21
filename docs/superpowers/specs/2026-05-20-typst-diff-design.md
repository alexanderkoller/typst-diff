# typst-diff Design Spec

**Date:** 2026-05-20  
**Status:** Approved

## Overview

A Rust CLI tool that compares two Typst documents and produces a PDF showing
additions (green) and deletions (red strikethrough). Analogous to `latexdiff`
but operates on fully-evaluated Typst Content trees rather than source text,
so all macros and `#include`s are expanded before diffing.

## Usage

```
typst-diff old/main.typ new/main.typ [-o diff.pdf]
```

- Two positional arguments: entry-point `.typ` file for each version
- `-o` / `--output`: output path (default: `diff.pdf` in current directory)
- Errors from either document are reported with file+line and exit code 1

## Pipeline

```
old/main.typ ──► [World A + typst_eval::eval] ──► old_content: Content
new/main.typ ──► [World B + typst_eval::eval] ──► new_content: Content
                                                          │
                                               [two-level Content diff]
                                                          │
                                               [annotate diff_content]
                                                          │
                                 [typst_layout::layout_document (World B)]
                                                          │
                                              [typst_pdf::pdf export]
                                                          │
                                                       diff.pdf
```

## Modules

| Module | Responsibility |
|---|---|
| `main` | CLI wiring, orchestration |
| `world` | `World` trait implementation for filesystem-backed Typst documents |
| `eval` | Thin wrapper around `typst_eval::eval` → `Content` |
| `diff` | Two-level block+word diff on Content trees |
| `annotate` | Build annotated `Content` from diff output |
| `render` | `layout_document` + `typst_pdf` → PDF bytes |

## Content Tree Traversal

### Level 1 — Block extraction

Walk the root `SequenceElem`, splitting into blocks at `ParbreakElem` boundaries:

| Node type | Block treatment |
|---|---|
| Inline sequence between `ParbreakElem`s | Paragraph block — word-diffed when matched |
| `HeadingElem` | Heading block — word-diff the body |
| `ListItem` / `EnumItem` | Item block — word-diff the body |
| `RawElem` | Atomic block — never word-diffed |
| `EquationElem` (display mode) | Atomic block — never word-diffed |
| Other block-producing nodes | Atomic block |

### Level 2 — Word extraction (within matched block pairs)

Recurse into inline content:

| Node type | Token |
|---|---|
| `TextElem` | Split `.text` on whitespace/punctuation → multiple word tokens |
| `SpaceElem` | Single space token |
| Anything else (Strong, Emph, inline equation, link, …) | Single atomic token |

### Block matching heuristic

1. Run LCS with exact `Content` equality to identify unchanged blocks.
2. For adjacent delete/insert groups ("edit zones"), pair blocks by minimum
   edit distance on `Content::plain_text()`. A pair is accepted if the
   similarity ratio (1 - edit_distance / max_len) exceeds 0.3; otherwise
   both blocks are treated as unrelated whole-block add/delete.
3. Paired blocks → word-level diff. Unpaired blocks → whole-block annotation.

## Annotation

**Word-level (within matched block pairs):**

- `Keep` → emit original `Content` node unchanged
- `Delete` → wrap in red `TextElem` fill + `StrikeElem`
- `Insert` → wrap in green `TextElem` fill
- Adjacent same-tag tokens are coalesced into one wrapper

**Block-level (unmatched blocks):**

- Deleted block → `.styled(TextElem::fill.set(red))` + strike on whole block
- Inserted block → `.styled(TextElem::fill.set(green))` on whole block
- Styling is applied to the `Content` node directly, so structural elements
  (headings, lists) retain their type and appearance (just colored)

**Colors:**

- Add: `Color::from_u8(0, 180, 0, 255)` (green)
- Delete: `Color::from_u8(220, 0, 0, 255)` (red)

## Rendering

Uses World B (the new document's world) for layout so fonts and binary assets
resolve correctly:

```rust
let engine = Engine {
    routines: &ROUTINES,
    world: new_world.track(),
    introspector: Introspector::default().track_with(&constraint),
    traced: Traced::default().track(),
    sink: sink.track_mut(),
    route: Route::default(),
};
let document = typst_layout::layout_document(&mut engine, &diff_content, styles)?;
let pdf_bytes = typst_pdf::pdf(&document, &PdfOptions::default())?;
```

One layout iteration is sufficient — the diff document has no counters or
cross-references requiring convergence.

## World Implementation

Each `World` instance is filesystem-backed, resolving paths relative to the
entry file's directory:

- `source()` — reads `.typ` files from disk, caches by `FileId`
- `file()` — reads binary files (images, etc.); only World B needs this (layout)
- `font()` — served via `typst-kit`'s `FontSearcher` (system fonts + embedded)
- `library()` — `Library::default()` constructed once per world
- `book()` — `FontBook` populated by `FontSearcher`

World A (old document) is eval-only; its `file()` and `font()` can return
`FileError::NotFound` since they are never called during pure evaluation.

## Crate Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `typst` | 0.14.2 | Re-exports, `ROUTINES`, `LibraryExt` |
| `typst-eval` | 0.14.2 | `eval()` function |
| `typst-layout` | 0.14.2 | `layout_document()` |
| `typst-pdf` | 0.14.2 | `pdf()` export |
| `typst-kit` | 0.14.2 | Font searching |
| `similar` | latest | LCS sequence diff |
| `clap` | latest | CLI argument parsing |
| `anyhow` | latest | Error handling |

## Feasibility Notes

Confirmed via spike (`/tmp/typst-spike`):

- `typst_eval::eval()` is public; returns `Module` with `.content()` method
- `Content::traverse()` and `to_packed::<TextElem>()` work as expected;
  `TextElem::text` field is accessible
- `StrikeElem::new(content)` + `.styled(TextElem::fill.set(...))` construct
  annotated Content correctly
- All `Engine` struct fields are `pub`; can be constructed externally
- `typst::compile::<PagedDocument>()` works with a minimal `World` impl

## Out of Scope (v1)

- Git version comparison (compare working trees by ref) — future work
- HTML output
- Configurable diff colors
- Moved-paragraph detection (currently treated as delete + insert)
- Diffing inside math equations or code blocks
