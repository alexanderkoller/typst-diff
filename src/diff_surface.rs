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
}
