//! Annotated realized content tree.
//!
//! [`AnnotatedContent`] pairs a realized [`Content`] node with semantic
//! information recovered from the pre-realization tree. The realized side is
//! preserved exactly as Typst produced it; annotations are built once and
//! never mutated.

use typst::foundations::Content;
use typst::syntax::Span;
use crate::content_slots::SlotStep;

/// A realized Content node together with its semantic identity.
pub struct AnnotatedContent {
    /// The realized content as Typst produced it (what gets rendered).
    pub realized: Content,
    /// Semantic information derived once at eval time.
    pub annotation: Annotation,
    /// Annotated children, mirroring the descent points of the realized tree.
    /// Empty for leaves and for nodes we don't descend into.
    pub children: Vec<AnnotatedContent>,
}

impl AnnotatedContent {
    /// Convenience: return the plain text of the realized content.
    pub fn plain_text(&self) -> typst::diag::EcoString {
        self.realized.plain_text()
    }

    /// Is the realized content empty?
    pub fn is_empty(&self) -> bool {
        self.realized.is_empty()
    }
}

pub struct Annotation {
    /// Pre-realization element type if this node is a tracked structural element.
    /// `None` for plain text, spaces, anonymous wrappers.
    pub semantic_kind: Option<SemanticKind>,
    /// Semantic slots — named positions within `children` that the diff recurses into.
    pub slots: Vec<SemanticSlot>,
    /// Footnote body if this realized node is a footnote marker site.
    pub footnote: Option<FootnoteInfo>,
    /// Source span for diagnostics (not used as a lookup key).
    pub span: Span,
}

impl Default for Annotation {
    fn default() -> Self {
        Annotation {
            semantic_kind: None,
            slots: vec![],
            footnote: None,
            span: Span::detached(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticKind {
    Paragraph,
    Heading,
    RawBlock,
    List,
    Enum,
    Terms,
    Table,
    Grid,
    Stack,
    Figure,
    Footnote,
    Quote,
    Equation,
    Wrapper(WrapperKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WrapperKind {
    Align, Pad, Place, Columns, Box, Block, Rect, Circle, Ellipse,
}

/// A named semantic position within an [`AnnotatedContent`] node.
///
/// `child_index` points into the parent's `children` vec.
/// `label` identifies the slot's role (e.g. `ListItem(0)`).
#[derive(Clone, Debug)]
pub struct SemanticSlot {
    pub label: SlotStep,
    pub child_index: usize,
}

pub struct FootnoteInfo {
    pub body: Content,
}
