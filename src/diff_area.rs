#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffAreaKind {
    BodyBlock,
    SemanticPageRegion,
    RenderedPageRegion,
    StructuredContainerRegion,
}

impl DiffAreaKind {
    pub(crate) fn trace_name(self) -> &'static str {
        match self {
            Self::BodyBlock => "body_block",
            Self::SemanticPageRegion => "semantic_page_region",
            Self::RenderedPageRegion => "rendered_page_region",
            Self::StructuredContainerRegion => "structured_container_region",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_area_kind_has_stable_trace_names() {
        assert_eq!(DiffAreaKind::BodyBlock.trace_name(), "body_block");
        assert_eq!(
            DiffAreaKind::StructuredContainerRegion.trace_name(),
            "structured_container_region"
        );
    }
}
