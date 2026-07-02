use std::cell::RefCell;
use std::collections::VecDeque;

use rustc_hash::FxHashMap;
use typst::diag::SourceResult;
use typst::engine::Engine;
use typst::foundations::{
    Content, ContextElem, Element, NativeElement, NativeRuleMap, NativeShowRule, Packed, ShowFn,
    StyleChain, Target,
};
use typst::syntax::Span;

thread_local! {
    static CONTEXT_RESULTS: RefCell<VecDeque<(Span, Content)>> = const {
        RefCell::new(VecDeque::new())
    };
}

#[repr(C)]
struct NativeRuleMapRepr {
    rules: FxHashMap<(Element, Target), NativeShowRule>,
}

pub(crate) fn install_recording_context_rule(rules: &mut NativeRuleMap) {
    let replacement = NativeShowRule::new(recording_context_rule as ShowFn<ContextElem>);

    // Typst's NativeRuleMap exposes registration but not replacement. The map is
    // a single-field wrapper, so keep this narrow and assert the layout we rely on.
    debug_assert_eq!(
        std::mem::size_of::<NativeRuleMap>(),
        std::mem::size_of::<NativeRuleMapRepr>()
    );
    let raw = unsafe { &mut *(rules as *mut NativeRuleMap).cast::<NativeRuleMapRepr>() };
    raw.rules
        .insert((ContextElem::ELEM, Target::Paged), replacement);
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
