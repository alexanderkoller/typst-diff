//! `typst_diff` — produce an annotated diff PDF from two Typst documents.
//!
//! # Pipeline
//!
//! ```text
//! old.typ ──► SystemWorld ──► eval_to_realized_content ──► old: AnnotatedContent
//! new.typ ──► SystemWorld ──► eval_to_realized_content ──► new: AnnotatedContent
//!                                       │
//!                          diff::diff_annotated(old, new) ──► DiffResult
//!                                       │
//!               annotate::build_annotated_content_from_tree(result) ──► Content
//!                                       │
//!                render::render_to_pdf(content, new_world) ──► Vec<u8>
//! ```
//!
//! The new document's [`world::SystemWorld`] is reused for annotation and rendering;
//! the old world is discarded after evaluation.

pub mod annotate;
pub mod annotated;
mod attributed_block_stream;
pub mod build_info;
mod container_ops;
mod content_key;
mod content_tree;
mod context_recording;
pub mod debug;
pub mod decision;
pub mod diag;
pub mod diff;
mod diff_area;
mod diff_surface;
mod edit_script;
pub mod eval;
mod normalize;
mod patch_surface;
pub mod render;
mod style_context;
pub mod trace;
pub mod world;

pub use annotated::AnnotatedContent;
pub use eval::{eval_to_content, eval_to_realized_content};
pub use render::render_to_pdf;
