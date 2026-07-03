use crate::diff_area::DiffAreaKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffSurfaceKind {
    WordTokens,
    RawLines,
    EquationTokens,
    NonTokenDisplay,
    OpaqueVisual,
    RenderedRegionText,
    RenderedRegionSegment,
}

impl DiffSurfaceKind {
    pub(crate) fn trace_name(self) -> &'static str {
        match self {
            Self::WordTokens => "word_tokens",
            Self::RawLines => "raw_lines",
            Self::EquationTokens => "equation_tokens",
            Self::NonTokenDisplay => "non_token_display",
            Self::OpaqueVisual => "opaque_visual",
            Self::RenderedRegionText => "rendered_region_text",
            Self::RenderedRegionSegment => "rendered_region_segment",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiffSurfaceEdit<T> {
    kind: DiffSurfaceKind,
    content: T,
}

impl<T> DiffSurfaceEdit<T> {
    pub(crate) fn new(kind: DiffSurfaceKind, content: T) -> Self {
        Self { kind, content }
    }

    pub(crate) fn kind(&self) -> DiffSurfaceKind {
        self.kind
    }

    pub(crate) fn into_content(self) -> T {
        self.content
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiffSelection<T> {
    area: DiffAreaKind,
    surface: DiffSurfaceEdit<T>,
}

impl<T> DiffSelection<T> {
    pub(crate) fn new(area: DiffAreaKind, surface: DiffSurfaceKind, content: T) -> Self {
        Self {
            area,
            surface: DiffSurfaceEdit::new(surface, content),
        }
    }

    pub(crate) fn area(&self) -> DiffAreaKind {
        self.area
    }

    pub(crate) fn surface_kind(&self) -> DiffSurfaceKind {
        self.surface.kind()
    }

    pub(crate) fn into_content(self) -> T {
        self.surface.into_content()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_surface_kind_has_stable_trace_names() {
        assert_eq!(DiffSurfaceKind::WordTokens.trace_name(), "word_tokens");
        assert_eq!(DiffSurfaceKind::RawLines.trace_name(), "raw_lines");
        assert_eq!(
            DiffSurfaceKind::RenderedRegionSegment.trace_name(),
            "rendered_region_segment"
        );
    }

    #[test]
    fn diff_selection_carries_area_and_surface() {
        let selection = DiffSelection::new(
            DiffAreaKind::BodyBlock,
            DiffSurfaceKind::WordTokens,
            "content",
        );

        assert_eq!(selection.area(), DiffAreaKind::BodyBlock);
        assert_eq!(selection.surface_kind(), DiffSurfaceKind::WordTokens);
        assert_eq!(selection.into_content(), "content");
    }
}
