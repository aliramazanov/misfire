use super::{Vocabulary, text};
use crate::config::{self, RustConfig};
use crate::index::{Assertion, Disabled, ExpectedFailure, Severity, Strength, TestFile, TestFn};
use globset::GlobSet;
use tree_sitter::Node;

struct Ctx<'a> {
    cfg: &'a RustConfig,
    specific_macros: Vocabulary,
    broad_macros: Vocabulary,
    compiled_out_macros: Vocabulary,
    errors: Vec<std::ops::Range<usize>>,
    assertion_globs: GlobSet,
    method_globs: GlobSet,
}

pub fn index(src: &str, cfg: &RustConfig) -> TestFile {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("rust grammar");

    let Some(tree) = parser.parse(src, None) else {
        return TestFile {
            tests: Vec::new(),
            helpers: Vec::new(),
            allows: Vec::new(),
            build_constraint: None,
            suite_runner: None,
            parsed: false,
        };
    };

    let ctx = Ctx {
        cfg,
        specific_macros: Vocabulary::new(&cfg.specific_macros),
        broad_macros: Vocabulary::new(&cfg.broad_macros),
        compiled_out_macros: Vocabulary::new(&cfg.compiled_out_macros),
        errors: crate::index::error_ranges(tree.root_node()),
        assertion_globs: config::compile(&cfg.assertion_patterns)
            .unwrap_or_else(|_| GlobSet::empty()),
        method_globs: config::compile(&cfg.assertion_methods).unwrap_or_else(|_| GlobSet::empty()),
    };

    let mut tests = Vec::new();
    let mut helpers = Vec::new();
    let root = tree.root_node();

    collect(root, src, &ctx, &mut tests, &mut helpers);

    let mut file = TestFile {
        tests,
        helpers,
        allows: Vec::new(),
        build_constraint: None,
        suite_runner: None,
        parsed: true,
    };
    crate::index::attach_suppressions(src, &mut file);
    file
}

fn collect(node: Node, src: &str, ctx: &Ctx, out: &mut Vec<TestFn>, helpers: &mut Vec<TestFn>) {
    collect_at(node, src, ctx, 0, None, out, helpers);
}

fn collect_at(
    node: Node,
    src: &str,
    ctx: &Ctx,
    depth: usize,
    gate: Option<&str>,
    out: &mut Vec<TestFn>,
    helpers: &mut Vec<TestFn>,
) {
    if depth > crate::index::MAX_DEPTH {
        return;
    }

    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "function_item"
            && let Some((mut f, is_test)) =
                index_function(child, &preceding_attributes(child), src, ctx)
        {
            if let Some(g) = gate {
                f.disabled.get_or_insert(crate::index::Disabled {
                    line: f.line,
                    marker: g.to_string(),
                });
            }
            if is_test {
                out.push(f);
            } else {
                helpers.push(f);
            }
        }

        let inherited = if child.kind() == "mod_item" {
            feature_gate(&preceding_attributes(child), src)
        } else {
            None
        };

        let next = inherited.as_deref().or(gate);

        collect_at(child, src, ctx, depth + 1, next, out, helpers);
    }
}

fn feature_gate(attrs: &[Node], src: &str) -> Option<String> {
    attrs.iter().find_map(|a| {
        let body = text(*a, src);
        (body.starts_with("#[cfg(") && body.contains("feature")).then(|| body.trim().to_string())
    })
}

fn preceding_attributes(func: Node) -> Vec<Node> {
    let mut attrs = Vec::new();
    let mut cur = func.prev_sibling();

    while let Some(n) = cur {
        match n.kind() {
            "attribute_item" => attrs.push(n),
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        cur = n.prev_sibling();
    }

    attrs.reverse();

    attrs
}

fn index_function(node: Node, attrs: &[Node], src: &str, ctx: &Ctx) -> Option<(TestFn, bool)> {
    let names: Vec<String> = attrs
        .iter()
        .filter_map(|a| attribute_name(*a, src))
        .collect();

    let is_test = names.iter().any(|n| ctx.cfg.test_attributes.contains(n));
    let own_gate = feature_gate(attrs, src);

    let name_node = node.child_by_field_name("name")?;
    let mut test = TestFn {
        name: text(name_node, src).to_string(),
        line: node.start_position().row + 1,
        assertions: Vec::new(),
        disabled: None,
        expected_failure: None,
        subtests: Vec::new(),
        helper_calls: Vec::new(),
        delegating_calls: Vec::new(),
        end_line: node.end_position().row + 1,
        allows: Vec::new(),
        disabled_subtest: None,
        trusted: !crate::index::spans_an_error(node, &ctx.errors),
    };

    for (attr, name) in attrs.iter().zip(names.iter()) {
        match name.as_str() {
            "ignore" => {
                test.disabled.get_or_insert(Disabled {
                    line: attr.start_position().row + 1,
                    marker: "#[ignore]".into(),
                });
            }
            "should_panic" => {
                test.expected_failure.get_or_insert(ExpectedFailure {
                    line: attr.start_position().row + 1,
                    marker: "#[should_panic]".into(),
                    expected: expected_argument(*attr, src),
                });
            }
            _ => {}
        }
    }

    if let Some(gate) = own_gate {
        test.disabled.get_or_insert(Disabled {
            line: test.line,
            marker: gate,
        });
    }

    if let Some(body) = node.child_by_field_name("body") {
        walk_body(body, src, ctx, &mut test);
    }

    Some((test, is_test))
}

fn attribute_name(item: Node, src: &str) -> Option<String> {
    let mut cursor = item.walk();

    let attr = item
        .children(&mut cursor)
        .find(|c| c.kind() == "attribute")?;

    let inner = attr.child(0)?;

    match inner.kind() {
        "identifier" | "scoped_identifier" => Some(text(inner, src).to_string()),
        _ => None,
    }
}

fn expected_argument(item: Node, src: &str) -> Option<String> {
    let raw = text(item, src);
    let start = raw.find("expected")?;
    let rest = &raw[start..];
    let open = rest.find('"')?;
    let after = &rest[open + 1..];
    let close = after.find('"')?;
    Some(after[..close].to_string())
}

fn walk_body(node: Node, src: &str, ctx: &Ctx, test: &mut TestFn) {
    walk_body_at(node, src, ctx, 0, test);
}

fn walk_body_at(node: Node, src: &str, ctx: &Ctx, depth: usize, test: &mut TestFn) {
    if depth > crate::index::MAX_DEPTH {
        test.trusted = false;
        return;
    }

    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "function_item" {
            continue;
        }

        match child.kind() {
            "macro_definition" => {
                test.delegating_calls.push(
                    child
                        .child_by_field_name("name")
                        .map_or_else(|| "macro_rules".into(), |n| text(n, src).to_string()),
                );
            }
            "macro_invocation" => classify_macro(child, src, ctx, test),
            "call_expression" => {
                if let Some(f) = child.child_by_field_name("function")
                    && f.kind() == "field_expression"
                    && let Some(field) = f.child_by_field_name("field")
                {
                    let method = text(field, src);
                    if ctx.method_globs.is_match(method) {
                        test.assertions.push(Assertion {
                            line: child.start_position().row + 1,
                            call: format!(".{method}()"),
                            strength: Strength::Specific,
                            severity: Severity::Fatal,
                            checks_failure: false,
                            guard: false,
                        });
                    } else {
                        test.helper_calls.push(method.to_string());
                    }
                }
                if let Some(f) = child.child_by_field_name("function")
                    && f.kind() == "identifier"
                {
                    let name = text(f, src);
                    if ctx.assertion_globs.is_match(name) {
                        test.assertions.push(Assertion {
                            line: child.start_position().row + 1,
                            call: name.to_string(),
                            strength: Strength::Specific,
                            severity: Severity::Fatal,
                            checks_failure: false,
                            guard: false,
                        });
                    } else {
                        test.helper_calls.push(name.to_string());
                    }
                }
            }
            _ => {}
        }

        walk_body_at(child, src, ctx, depth + 1, test);
    }
}

fn classify_macro(call: Node, src: &str, ctx: &Ctx, test: &mut TestFn) {
    let Some(name_node) = call.child_by_field_name("macro") else {
        return;
    };

    let name = text(name_node, src);

    let strength = if ctx.specific_macros.contains(name) {
        Strength::Specific
    } else if ctx.broad_macros.contains(name) {
        Strength::Broad
    } else if ctx.assertion_globs.is_match(name) {
        Strength::Specific
    } else {
        return;
    };

    let severity = if ctx.compiled_out_macros.contains(name) {
        Severity::NonFatal
    } else {
        Severity::Fatal
    };

    test.assertions.push(Assertion {
        line: call.start_position().row + 1,
        call: format!("{name}!"),
        strength,
        severity,
        checks_failure: false,
        guard: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_user() {
        let u = create("ali");
        assert_eq!(u.name, "ali");
        assert!(u.active);
    }

    #[test]
    #[ignore]
    fn slow_one() {
        assert_eq!(1, 1);
    }

    #[test]
    #[should_panic(expected = "insufficient funds")]
    fn refund_rejects() {
        refund(500);
    }

    #[test]
    #[should_panic]
    fn refund_rejects_vaguely() {
        refund(500);
    }

    #[tokio::test]
    async fn async_one() {
        assert_ne!(fetch().await, 0);
    }

    #[test]
    fn release_blind() {
        debug_assert_eq!(compute(), 4);
    }

    fn not_a_test() {
        assert_eq!(1, 1);
    }
}
"#;

    fn file() -> TestFile {
        index(SAMPLE, &RustConfig::default())
    }

    #[test]
    fn finds_tests_nested_in_a_mod() {
        let names: Vec<_> = file().tests.iter().map(|t| t.name.clone()).collect();
        assert_eq!(
            names,
            [
                "creates_user",
                "slow_one",
                "refund_rejects",
                "refund_rejects_vaguely",
                "async_one",
                "release_blind"
            ]
        );
    }

    #[test]
    fn counts_assert_macros() {
        let t = file().get("creates_user").unwrap().clone();
        assert_eq!(t.assertion_count(), 2);
        assert_eq!(
            t.specific_count(),
            1,
            "assert! is broad, assert_eq! is specific"
        );
    }

    #[test]
    fn detects_ignore_attribute() {
        let t = file().get("slow_one").unwrap().clone();
        assert_eq!(t.disabled.unwrap().marker, "#[ignore]");
    }

    #[test]
    fn captures_should_panic_expected_message() {
        let t = file().get("refund_rejects").unwrap().clone();
        let ef = t.expected_failure.unwrap();
        assert_eq!(ef.expected.as_deref(), Some("insufficient funds"));
    }

    #[test]
    fn bare_should_panic_has_no_expected_message() {
        let t = file().get("refund_rejects_vaguely").unwrap().clone();
        let ef = t.expected_failure.unwrap();
        assert_eq!(ef.expected, None);
    }

    #[test]
    fn recognises_tokio_test_attribute() {
        assert!(file().get("async_one").is_some());
    }

    #[test]
    fn debug_assert_is_non_fatal_because_release_drops_it() {
        let t = file().get("release_blind").unwrap().clone();
        assert_eq!(t.fatal_count(), 0);
        assert_eq!(t.assertion_count(), 1);
    }

    #[test]
    fn plain_functions_are_not_tests() {
        assert!(file().get("not_a_test").is_none());
    }

    #[test]
    fn unparseable_source_yields_no_tests_rather_than_panicking() {
        assert!(
            index("fn fn fn {{{ #[test]", &RustConfig::default())
                .tests
                .is_empty()
        );
    }
}
