//! `typst_diff` — produce an annotated diff PDF from two Typst documents.
//!
//! # Pipeline
//!
//! ```text
//! old.typ ──► SystemWorld ──► eval_to_realized_content ──► old_content: Content
//! new.typ ──► SystemWorld ──► eval_to_realized_content ──► new_content: Content
//!                                       │
//!                          diff::diff_content(old, new) ──► DiffResult
//!                                       │
//!               annotate::build_annotated_content(result) ──► Content
//!                                       │
//!                render::render_to_pdf(content, new_world) ──► Vec<u8>
//! ```
//!
//! The new document's [`world::SystemWorld`] is reused for annotation and rendering;
//! the old world is discarded after evaluation.

pub mod annotate;
pub mod annotated;
pub mod content_slots;
pub mod diag;
pub mod diff;
pub mod eval;
pub mod render;
pub mod world;

pub use annotate::build_annotated_content;
pub use annotated::AnnotatedContent;
pub use eval::{eval_to_content, eval_to_realized_content};
pub use render::render_to_pdf;
