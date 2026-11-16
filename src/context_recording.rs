use std::cell::RefCell;
use std::collections::VecDeque;

use typst::diag::SourceResult;
use typst::engine::Engine;
use typst::foundations::{
    Content, ContextElem, NativeRuleMap, Packed, ShowFn, StyleChain, Target,
};
use typst::syntax::Span;

thread_local! {
    static CONTEXT_RESULTS: RefCell<VecDeque<(Span, Content)>> = const {
        RefCell::new(VecDeque::new())
    };
}

pub(crate) fn install_recording_context_rule(rules: &mut NativeRuleMap) {
    rules.replace(Target::Paged, recording_context_rule as ShowFn<ContextElem>);
}

pub(crate) fn clear() {
    CONTEXT_RESULTS.with(|results| results.borrow_mut().clear());
}

pub(crate) fn take(span: Span) -> Option<Content> {
    CONTEXT_RESULTS.with(|results| {
        let mut results = results.borrow_mut();
        let index = results
            .iter()
            .position(|(recorded_span, _)| *recorded_span == span)?;
        results.remove(index).map(|(_, content)| content)
    })
}

pub(crate) fn peek(span: Span) -> Option<Content> {
    CONTEXT_RESULTS.with(|results| {
        results
            .borrow()
            .iter()
            .find(|(recorded_span, _)| *recorded_span == span)
            .map(|(_, content)| content.clone())
    })
}

fn recording_context_rule(
    elem: &Packed<ContextElem>,
    engine: &mut Engine,
    styles: StyleChain,
) -> SourceResult<Content> {
    let result = typst::foundations::CONTEXT_RULE(elem, engine, styles)?;
    CONTEXT_RESULTS.with(|results| {
        results
            .borrow_mut()
            .push_back((elem.span(), result.clone()));
    });
    Ok(result)
}
