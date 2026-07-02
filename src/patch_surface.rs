use typst::foundations::{Content, Styles};

#[derive(Clone, Debug, PartialEq)]
pub enum PatchSurface {
    PreContainer(Content),
    GraftedBlockBody(Content),
    LayoutPreservingSequence(Content),
    OpaqueVisual(Content),
    RenderedEditSurface(Content),
}

impl PatchSurface {
    pub(crate) fn pre_container(content: Content) -> Self {
        Self::PreContainer(content)
    }

    pub(crate) fn grafted_block_body(content: Content) -> Self {
        Self::GraftedBlockBody(content)
    }

    pub(crate) fn layout_preserving_sequence(content: Content) -> Self {
        Self::LayoutPreservingSequence(content)
    }

    pub(crate) fn opaque_visual(content: Content) -> Self {
        Self::OpaqueVisual(content)
    }

    pub(crate) fn rendered_edit_surface(content: Content) -> Self {
        Self::RenderedEditSurface(content)
    }

    pub(crate) fn as_content(&self) -> &Content {
        match self {
            Self::PreContainer(content)
            | Self::GraftedBlockBody(content)
            | Self::LayoutPreservingSequence(content)
            | Self::OpaqueVisual(content)
            | Self::RenderedEditSurface(content) => content,
        }
    }

    pub(crate) fn map_content(self, f: impl FnOnce(Content) -> Content) -> Self {
        match self {
            Self::PreContainer(content) => Self::PreContainer(f(content)),
            Self::GraftedBlockBody(content) => Self::GraftedBlockBody(f(content)),
            Self::LayoutPreservingSequence(content) => Self::LayoutPreservingSequence(f(content)),
            Self::OpaqueVisual(content) => Self::OpaqueVisual(f(content)),
            Self::RenderedEditSurface(content) => Self::RenderedEditSurface(f(content)),
        }
    }

    pub(crate) fn styled_with_map(self, styles: Styles) -> Self {
        self.map_content(|content| content.styled_with_map(styles))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::text::TextElem;

    #[test]
    fn patch_surface_map_content_preserves_variant() {
        let surface = PatchSurface::grafted_block_body(TextElem::packed("old"))
            .map_content(|_| TextElem::packed("new"));

        assert!(matches!(surface, PatchSurface::GraftedBlockBody(_)));
        assert_eq!(surface.as_content().plain_text(), "new");
    }

    #[test]
    fn patch_surface_exposes_content_without_erasing_reason() {
        let surface = PatchSurface::opaque_visual(TextElem::packed("graphic"));

        assert!(matches!(surface, PatchSurface::OpaqueVisual(_)));
        assert_eq!(surface.as_content().plain_text(), "graphic");
    }
}
