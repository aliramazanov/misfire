use super::{Vocabulary, text};
use crate::config::{self, GoConfig};
use crate::index::{Assertion, Severity, Strength, SuiteRunner, TestFile, TestFn};
use globset::GlobSet;
use tree_sitter::Node;

pub(super) const T_FATAL: [&str; 3] = ["Fatal", "Fatalf", "FailNow"];
pub(super) const T_NONFATAL: [&str; 3] = ["Error", "Errorf", "Fail"];
pub(super) const T_SKIP: [&str; 3] = ["Skip", "Skipf", "SkipNow"];

pub(super) struct Ctx {
    pub(super) fatal_packages: Vocabulary,
    pub(super) nonfatal_packages: Vocabulary,
    pub(super) broad_matchers: Vocabulary,
    pub(super) failure_matchers: Vocabulary,
    pub(super) errors: Vec<std::ops::Range<usize>>,
    pub(super) assertion_globs: GlobSet,
    pub(super) fatal_globs: GlobSet,
}

pub fn index(src: &str, cfg: &GoConfig) -> TestFile {
    let mut parser = tree_sitter::Parser::new();

    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("go grammar");

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
        fatal_packages: Vocabulary::new(&cfg.fatal_packages),
        nonfatal_packages: Vocabulary::new(&cfg.nonfatal_packages),
        broad_matchers: Vocabulary::new(&cfg.broad_matchers),
        failure_matchers: Vocabulary::new(&cfg.failure_matchers),
        errors: crate::index::error_ranges(tree.root_node()),
        assertion_globs: config::compile(&cfg.assertion_patterns)
            .unwrap_or_else(|_| GlobSet::empty()),
        fatal_globs: config::compile(&cfg.fatal_patterns).unwrap_or_else(|_| GlobSet::empty()),
    };

    let mut tests = Vec::new();
    let mut helpers = Vec::new();
    let mut suite_runner = None;
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if child.kind() != "function_declaration" {
            continue;
        }

        if let Some(runner) = suite_entry_point(child, src) {
            suite_runner = Some(runner);
            continue;
        }

        match index_function(child, src, &ctx) {
            Some((f, true)) => tests.push(f),
            Some((f, false)) => helpers.push(f),
            None => {}
        }
    }

    let mut file = TestFile {
        tests,
        helpers,
        allows: Vec::new(),
        build_constraint: build_constraint(src),
        suite_runner,
        parsed: true,
    };

    crate::index::attach_suppressions(src, &mut file);

    file
}

fn index_function(node: Node, src: &str, ctx: &Ctx) -> Option<(TestFn, bool)> {
    let name_node = node.child_by_field_name("name")?;
    let name = text(name_node, src).to_string();
    let body = node.child_by_field_name("body")?;

    if is_example_name(&name) && testing_param(node, src).is_none() {
        let declares_output = example_output(body, src);
        let mut example = new_test(&name, node, ctx);

        if declares_output {
            example.assertions.push(Assertion {
                line: node.start_position().row + 1,
                call: "// Output:".to_string(),
                strength: Strength::Specific,
                severity: Severity::Fatal,
                checks_failure: false,
                guard: false,
            });
        }

        walk_body(body, src, "", ctx, &mut example);
        return Some((example, declares_output));
    }

    let t_param = testing_param(node, src)?;
    let is_test = (name.starts_with("Test") && t_param.strict) || t_param.fuzz;

    let mut test = new_test(&name, node, ctx);

    walk_body(body, src, &t_param.name, ctx, &mut test);

    Some((test, is_test))
}

fn build_constraint(src: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("package ") {
            return None;
        }
        if let Some(rest) = line.strip_prefix("//go:build ") {
            return Some(rest.trim().to_string());
        }
    }

    None
}

fn example_output(node: Node, src: &str) -> bool {
    let body = text(node, src);

    body.lines().any(|l| {
        let l = l.trim().to_ascii_lowercase();
        l.starts_with("// output:") || l.starts_with("// unordered output:")
    })
}

fn suite_entry_point(node: Node, src: &str) -> Option<SuiteRunner> {
    let name = node.child_by_field_name("name")?;

    if text(name, src) != "TestMain" {
        return None;
    }

    let params = node.child_by_field_name("parameters")?;
    let receiver = testing_m_param(params, src)?;
    let body = node.child_by_field_name("body")?;

    Some(SuiteRunner {
        line: node.start_position().row + 1,
        runs: calls_run(body, src, &receiver),
    })
}

fn testing_m_param(params: Node, src: &str) -> Option<String> {
    let mut cursor = params.walk();

    for decl in params.children(&mut cursor) {
        if decl.kind() != "parameter_declaration" {
            continue;
        }

        let ty = decl.child_by_field_name("type")?;

        if text(ty, src).trim() == "*testing.M" {
            let ident = decl.child_by_field_name("name")?;
            return Some(text(ident, src).to_string());
        }
    }

    None
}

fn calls_run(node: Node, src: &str, receiver: &str) -> bool {
    calls_run_at(node, src, receiver, 0)
}

fn calls_run_at(node: Node, src: &str, receiver: &str, depth: usize) -> bool {
    if depth > crate::index::MAX_DEPTH {
        return false;
    }

    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression"
            && let Some(func) = child.child_by_field_name("function")
            && func.kind() == "selector_expression"
            && let (Some(operand), Some(field)) = (
                func.child_by_field_name("operand"),
                func.child_by_field_name("field"),
            )
            && text(operand, src) == receiver
            && text(field, src) == "Run"
        {
            return true;
        }
        if calls_run_at(child, src, receiver, depth + 1) {
            return true;
        }
    }

    false
}

fn is_example_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("Example") else {
        return false;
    };

    match rest.chars().next() {
        None => true,
        Some(c) => c.is_uppercase() || c == '_',
    }
}

fn new_test(name: &str, node: Node, ctx: &Ctx) -> TestFn {
    TestFn {
        name: name.to_string(),
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
    }
}

pub(super) struct TestingParam {
    pub(super) name: String,

    pub(super) strict: bool,

    pub(super) fuzz: bool,
}

pub(super) fn testing_param(node: Node, src: &str) -> Option<TestingParam> {
    let params = node.child_by_field_name("parameters")?;
    let mut declarations = 0;
    let mut found: Option<TestingParam> = None;
    let mut cursor = params.walk();

    for decl in params.children(&mut cursor) {
        if decl.kind() != "parameter_declaration" {
            continue;
        }

        declarations += 1;

        let Some(ty) = decl.child_by_field_name("type") else {
            continue;
        };

        let ty_text = text(ty, src).trim();
        let is_t = ty.kind() == "pointer_type" && ty_text == "*testing.T";
        let is_f = ty.kind() == "pointer_type" && ty_text == "*testing.F";

        if is_t || is_f || ty_text == "testing.TB" {
            let ident = decl.child_by_field_name("name")?;
            found = Some(TestingParam {
                name: text(ident, src).to_string(),
                strict: is_t,
                fuzz: is_f,
            });
        }
    }

    let mut param = found?;

    if declarations != 1 {
        param.strict = false;
        param.fuzz = false;
    }

    Some(param)
}

#[derive(Clone, Copy)]
pub(super) struct Position<'a> {
    pub(super) guarded: bool,

    pub(super) error_guard: bool,

    pub(super) subtest: Option<&'a str>,
    depth: usize,
}

mod body;
use body::walk_body;

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
package pkg

import "testing"

func TestCreateUser(t *testing.T) {
	u, err := Create("ali")
	require.NoError(t, err)
	assert.Equal(t, "ali", u.Name)
	if u.ID != 1 {
		t.Fatalf("bad id %d", u.ID)
	}
}

func TestSkipped(t *testing.T) {
	t.Skip("flaky")
	assert.True(t, false)
}

func TestTable(t *testing.T) {
	for _, c := range cases {
		t.Run("doubles", func(t *testing.T) {
			assert.Equal(t, c.want, Double(c.in))
		})
	}
}

func TestRenamedReceiver(tt *testing.T) {
	tt.Skip("nope")
}

func TestNegative(t *testing.T) {
	assert.ErrorIs(t, err, ErrNotFound)
}

func helperNotATest(t *testing.T) {
	assert.Equal(t, 1, 1)
}
"#;

    fn file() -> TestFile {
        index(SAMPLE, &GoConfig::default())
    }

    #[test]
    fn finds_only_test_functions() {
        let f = file();
        let names: Vec<_> = f.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "TestCreateUser",
                "TestSkipped",
                "TestTable",
                "TestRenamedReceiver",
                "TestNegative"
            ]
        );
    }

    #[test]
    fn counts_assertions_including_bare_t_fatal() {
        let t = file().get("TestCreateUser").unwrap().clone();
        assert_eq!(t.assertion_count(), 3);
        assert_eq!(
            t.fatal_count(),
            2,
            "require.NoError and t.Fatalf both abort"
        );
    }

    #[test]
    fn separates_require_from_assert_severity() {
        let t = file().get("TestCreateUser").unwrap().clone();
        let by_call = |c: &str| t.assertions.iter().find(|a| a.call == c).unwrap().severity;
        assert_eq!(by_call("require.NoError"), Severity::Fatal);
        assert_eq!(by_call("assert.Equal"), Severity::NonFatal);
    }

    #[test]
    fn detects_skip_marker() {
        let t = file().get("TestSkipped").unwrap().clone();
        assert_eq!(t.disabled.unwrap().marker, "t.Skip");
    }

    #[test]
    fn honours_a_renamed_testing_receiver() {
        let t = file().get("TestRenamedReceiver").unwrap().clone();
        assert!(
            t.disabled.is_some(),
            "skip must be found when the param is not named t"
        );
    }

    #[test]
    fn records_subtests_and_their_assertions() {
        let t = file().get("TestTable").unwrap().clone();
        assert_eq!(t.subtests, ["doubles"]);
        assert_eq!(
            t.assertion_count(),
            1,
            "assertions inside t.Run belong to the parent test"
        );
    }

    #[test]
    fn flags_broad_matchers_as_broad() {
        let t = file().get("TestSkipped").unwrap().clone();
        let a = t
            .assertions
            .iter()
            .find(|a| a.call == "assert.True")
            .unwrap();
        assert_eq!(a.strength, Strength::Broad);
    }

    #[test]
    fn recognises_failure_expectation() {
        let t = file().get("TestNegative").unwrap().clone();
        assert_eq!(t.failure_checks(), 1);
        assert!(t.expected_failure.is_some());
    }

    #[test]
    fn no_error_is_an_assertion_but_not_a_failure_expectation() {
        let t = file().get("TestCreateUser").unwrap().clone();
        assert_eq!(
            t.failure_checks(),
            0,
            "require.NoError asserts success, it does not expect a failure"
        );
    }

    #[test]
    fn unparseable_source_yields_no_tests_rather_than_panicking() {
        assert!(
            index("package \u{1F600} func func func {{{", &GoConfig::default())
                .tests
                .is_empty()
        );
    }
}
