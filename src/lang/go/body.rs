use super::{Ctx, Position, T_FATAL, T_NONFATAL, T_SKIP, testing_param};
use crate::index::{Assertion, Disabled, ExpectedFailure, Severity, Strength, TestFn};
use crate::lang::text;
use tree_sitter::Node;

const CONDITIONAL: [&str; 5] = [
    "if_statement",
    "for_statement",
    "expression_switch_statement",
    "type_switch_statement",
    "select_statement",
];

pub(super) fn walk_body(node: Node, src: &str, recv: &str, ctx: &Ctx, test: &mut TestFn) {
    let start = Position {
        guarded: false,
        error_guard: false,
        subtest: None,
        depth: 0,
    };
    walk(node, src, recv, ctx, start, test);
}

fn walk(node: Node, src: &str, recv: &str, ctx: &Ctx, pos: Position, test: &mut TestFn) {
    if pos.depth > crate::index::MAX_DEPTH {
        test.trusted = false;
        return;
    }

    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            classify_call(child, src, recv, ctx, pos, test);
        }

        let entered = subtest_name(child, src, recv);
        let closure_recv = closure_testing_param(child, src);
        let recv = closure_recv.as_deref().unwrap_or(recv);
        let inner = Position {
            guarded: pos.guarded || CONDITIONAL.contains(&child.kind()),
            error_guard: pos.error_guard || guards_an_error(child, src),
            subtest: entered.as_deref().or(pos.subtest),
            depth: pos.depth + 1,
        };

        walk(child, src, recv, ctx, inner, test);
    }
}

fn guards_an_error(node: Node, src: &str) -> bool {
    if node.kind() != "if_statement" {
        return false;
    }

    let Some(cond) = node.child_by_field_name("condition") else {
        return false;
    };
    let text = text(cond, src);
    let touches_nil = text.contains("!= nil") || text.contains("== nil");
    let names_an_error = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| {
            let w = w.to_ascii_lowercase();
            w == "err" || w.ends_with("err") || w.starts_with("err") || w == "errs"
        });

    names_an_error && (touches_nil || text.contains("len("))
}

fn closure_testing_param(node: Node, src: &str) -> Option<String> {
    if node.kind() != "func_literal" {
        return None;
    }

    testing_param(node, src).map(|p| p.name)
}

fn subtest_name(node: Node, src: &str, recv: &str) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }

    let func = node.child_by_field_name("function")?;

    if func.kind() != "selector_expression" {
        return None;
    }

    let operand = func.child_by_field_name("operand")?;
    let field = func.child_by_field_name("field")?;

    if text(operand, src) != recv || text(field, src) != "Run" {
        return None;
    }

    first_string_arg(node, src)
}

fn classify_call(call: Node, src: &str, recv: &str, ctx: &Ctx, pos: Position, test: &mut TestFn) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };

    if func.kind() == "identifier" {
        classify_plain_call(call, func, src, recv, ctx, test);
        return;
    }

    classify_method_call(call, func, src, recv, ctx, pos, test);
}

#[allow(clippy::too_many_arguments)]
fn classify_method_call(
    call: Node,
    func: Node,
    src: &str,
    recv: &str,
    ctx: &Ctx,
    pos: Position,
    test: &mut TestFn,
) {
    if func.kind() != "selector_expression" {
        return;
    }

    let (Some(operand), Some(field)) = (
        func.child_by_field_name("operand"),
        func.child_by_field_name("field"),
    ) else {
        return;
    };

    let pkg = text(operand, src);
    let method = text(field, src);
    let full = format!("{pkg}.{method}");
    let line = call.start_position().row + 1;

    if pkg != recv && passes_receiver(call, src, recv) {
        test.delegating_calls.push(full.clone());
    }

    if pkg == recv {
        classify_testing_api(call, src, method, full, line, pos, test);
    } else {
        classify_matcher(pkg, method, full, line, ctx, test);
    }
}

fn classify_testing_api(
    call: Node,
    src: &str,
    method: &str,
    full: String,
    line: usize,
    pos: Position,
    test: &mut TestFn,
) {
    if T_SKIP.contains(&method) {
        if !pos.guarded {
            test.disabled.get_or_insert(Disabled { line, marker: full });
            if let Some(name) = pos.subtest {
                test.disabled_subtest
                    .get_or_insert_with(|| name.to_string());
            }
        }
        return;
    }

    if method == "Run" {
        if let Some(name) = first_string_arg(call, src) {
            test.subtests.push(name);
        }
        return;
    }

    let severity = if T_FATAL.contains(&method) {
        Severity::Fatal
    } else if T_NONFATAL.contains(&method) {
        Severity::NonFatal
    } else {
        return;
    };

    test.assertions.push(Assertion {
        line,
        call: full,
        strength: Strength::Specific,
        severity,
        checks_failure: false,
        guard: pos.error_guard,
    });
}

fn classify_matcher(
    pkg: &str,
    method: &str,
    full: String,
    line: usize,
    ctx: &Ctx,
    test: &mut TestFn,
) {
    let known = ctx.fatal_packages.contains(pkg) || ctx.nonfatal_packages.contains(pkg);
    if !known && !ctx.assertion_globs.is_match(&full) {
        test.helper_calls.push(full);
        return;
    }

    let severity = if ctx.fatal_packages.contains(pkg) || ctx.fatal_globs.is_match(&full) {
        Severity::Fatal
    } else {
        Severity::NonFatal
    };

    let strength = if ctx.broad_matchers.contains(method) {
        Strength::Broad
    } else {
        Strength::Specific
    };

    let checks_failure = ctx.failure_matchers.contains(method);
    if checks_failure {
        test.expected_failure.get_or_insert(ExpectedFailure {
            line,
            marker: full.clone(),
            expected: None,
        });
    }

    test.assertions.push(Assertion {
        line,
        call: full,
        strength,
        severity,
        checks_failure,
        guard: false,
    });
}

fn severity_for(name: &str, ctx: &Ctx) -> Severity {
    if ctx.fatal_globs.is_match(name) {
        Severity::Fatal
    } else {
        Severity::NonFatal
    }
}

fn classify_plain_call(
    call: Node,
    func: Node,
    src: &str,
    recv: &str,
    ctx: &Ctx,
    test: &mut TestFn,
) {
    let name = text(func, src);
    if passes_receiver(call, src, recv) {
        test.delegating_calls.push(name.to_string());
    }

    if ctx.assertion_globs.is_match(name) {
        test.assertions.push(Assertion {
            line: call.start_position().row + 1,
            call: name.to_string(),
            strength: Strength::Specific,
            severity: severity_for(name, ctx),
            checks_failure: false,
            guard: false,
        });
    } else {
        test.helper_calls.push(name.to_string());
    }
}

fn passes_receiver(call: Node, src: &str, recv: &str) -> bool {
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };

    let mut cursor = args.walk();

    args.children(&mut cursor)
        .any(|a| a.kind() == "identifier" && text(a, src) == recv)
}

fn first_string_arg(call: Node, src: &str) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();

    for arg in args.children(&mut cursor) {
        if arg.kind() == "interpreted_string_literal" {
            return Some(text(arg, src).trim_matches('"').to_string());
        }
    }

    None
}
