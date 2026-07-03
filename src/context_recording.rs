use std::cell::RefCell;
use std::collections::VecDeque;

use rustc_hash::FxHashMap;
use typst::World;
use typst::diag::SourceResult;
use typst::engine::Engine;
use typst::foundations::{
    Content, ContextElem, Element, NativeElement, NativeRuleMap, NativeShowRule, Packed, ShowFn,
    StyleChain, Target,
};
use typst::syntax::{Span, ast};

thread_local! {
    static CONTEXT_RESULTS: RefCell<VecDeque<(Span, Content)>> = const {
        RefCell::new(VecDeque::new())
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordedAlignment {
    Left,
    Center,
    Right,
    Start,
    End,
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

pub(crate) fn syntax_alignment(world: &dyn World, span: Span) -> Option<RecordedAlignment> {
    let id = span.id()?;
    let source = world.source(id).ok()?;
    let node = source.find(span)?;
    let expr = node.cast::<ast::Expr>()?;
    let expr = match expr {
        ast::Expr::Contextual(contextual) => contextual.body(),
        other => other,
    };
    recorded_alignment_expr(expr)
}

fn recorded_alignment_expr(expr: ast::Expr) -> Option<RecordedAlignment> {
    let mut found = None;
    collect_recorded_alignment_expr(expr, &mut found).then_some(found)?
}

fn collect_recorded_alignment_expr(expr: ast::Expr, found: &mut Option<RecordedAlignment>) -> bool {
    match expr {
        ast::Expr::FuncCall(call) => {
            if is_align_callee(call.callee())
                && let Some(alignment) = align_call_alignment(call)
                && !merge_recorded_alignment(found, alignment)
            {
                return false;
            }
            true
        }
        ast::Expr::CodeBlock(block) => block
            .body()
            .exprs()
            .all(|expr| collect_recorded_alignment_expr(expr, found)),
        ast::Expr::Contextual(contextual) => {
            collect_recorded_alignment_expr(contextual.body(), found)
        }
        ast::Expr::Conditional(conditional) => {
            collect_recorded_alignment_expr(conditional.if_body(), found)
                && conditional
                    .else_body()
                    .is_none_or(|expr| collect_recorded_alignment_expr(expr, found))
        }
        _ => true,
    }
}

fn is_align_callee(expr: ast::Expr) -> bool {
    matches!(expr, ast::Expr::Ident(ident) if ident.as_str() == "align")
}

fn align_call_alignment(call: ast::FuncCall) -> Option<RecordedAlignment> {
    call.args().items().find_map(|arg| match arg {
        ast::Arg::Pos(expr) => alignment_arg(expr),
        ast::Arg::Named(_) | ast::Arg::Spread(_) => None,
    })
}

fn alignment_arg(expr: ast::Expr) -> Option<RecordedAlignment> {
    match expr {
        ast::Expr::Ident(ident) => named_recorded_alignment(ident.as_str()),
        ast::Expr::FieldAccess(access) => {
            if matches!(access.target(), ast::Expr::Ident(ident) if ident.as_str() == "alignment") {
                named_recorded_alignment(access.field().as_str())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn named_recorded_alignment(name: &str) -> Option<RecordedAlignment> {
    match name {
        "left" => Some(RecordedAlignment::Left),
        "center" => Some(RecordedAlignment::Center),
        "right" => Some(RecordedAlignment::Right),
        "start" => Some(RecordedAlignment::Start),
        "end" => Some(RecordedAlignment::End),
        _ => None,
    }
}

fn merge_recorded_alignment(
    found: &mut Option<RecordedAlignment>,
    alignment: RecordedAlignment,
) -> bool {
    match *found {
        Some(previous) => previous == alignment,
        None => {
            *found = Some(alignment);
            true
        }
    }
}
