use typst::foundations::{NativeElement, Style, Styles};
use typst::layout::PageElem;

pub(crate) fn is_page_style(style: &Style) -> bool {
    style
        .element()
        .is_some_and(|element| element == PageElem::ELEM)
}

pub(crate) fn page_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect()
}

pub(crate) fn non_page_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| !is_page_style(style))
        .cloned()
        .map(Style::wrap)
        .collect()
}

pub(crate) fn marginal_styles(styles: &Styles) -> Styles {
    styles
        .iter()
        .filter(|style| is_page_style(style) || (style.outside() && style.liftable()))
        .cloned()
        .map(Style::wrap)
        .collect()
}

pub(crate) fn advance_sticky_page_styles(current: &mut Styles, block_styles: &mut Styles) {
    if !block_styles.is_empty() {
        *current = block_styles.clone();
    }
    *block_styles = current.clone();
}
