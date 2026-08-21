use misfire::config::Config;
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Confidence, Finding, Rule};

fn run(lang: Lang, before: &str, after: &str) -> Vec<Finding> {
    with_config(lang, before, after, &Config::default())
}

fn with_config(lang: Lang, before: &str, after: &str, cfg: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    rules::compare(
        "t",
        &lang.index(before, cfg),
        &lang.index(after, cfg),
        &misfire::rules::Options::default(),
        &mut out,
    );
    out
}

fn has(findings: &[Finding], rule: Rule) -> bool {
    findings.iter().any(|f| f.rule == rule)
}

fn only(findings: &[Finding]) -> &Finding {
    assert_eq!(findings.len(), 1, "expected one finding, got {findings:#?}");
    &findings[0]
}

const GO_BEFORE: &str = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	require.NoError(t, u.Err)
	assert.Equal(t, "ali", u.Name)
	assert.Equal(t, 1, u.ID)
}
"#;

#[test]
fn go_assertions_deleted_outright() {
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	require.NoError(t, u.Err)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::AssertionRemoved);
    assert_eq!(finding.confidence, Confidence::High);
    assert!(
        finding
            .detail
            .contains("2 assertions removed from TestCreateUser"),
        "must name the test: {}",
        finding.detail
    );
}

#[test]
fn go_test_skipped() {
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	t.Skip("flaky")
	u := Create("ali")
	require.NoError(t, u.Err)
	assert.Equal(t, "ali", u.Name)
	assert.Equal(t, 1, u.ID)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert!(has(&f, Rule::TestDisabled));
}

#[test]
fn go_new_test_with_zero_assertions() {
    let after = format!("{GO_BEFORE}\nfunc TestRefund(t *testing.T) {{\n\tRefund(500)\n}}\n");
    let f = run(Lang::Go, GO_BEFORE, &after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::TestWithoutAssertions);
    assert!(finding.detail.contains("TestRefund"));
}

#[test]
fn go_require_downgraded_to_assert_is_caught() {
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	assert.NoError(t, u.Err)
	assert.Equal(t, "ali", u.Name)
	assert.Equal(t, 1, u.ID)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::SeverityDowngraded);
    assert_eq!(finding.confidence, Confidence::High);
}

#[test]
fn go_assertion_loosened_to_broad_matcher() {
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	require.NoError(t, u.Err)
	assert.NotNil(t, u.Name)
	assert.Equal(t, 1, u.ID)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::AssertionLoosened);
}

#[test]
fn go_multiline_assertion_removal_is_still_seen() {
    let before = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	assert.Equal(
		t,
		"ali",
		u.Name,
	)
	assert.Equal(t, 1, u.ID)
}
"#;
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	assert.Equal(t, 1, u.ID)
}
"#;
    let f = run(Lang::Go, before, after);
    assert!(has(&f, Rule::AssertionRemoved), "got {f:#?}");
}

#[test]
fn go_reformatting_alone_produces_nothing() {
    let after = r#"
package pkg

import (
	"testing"
)

func TestCreateUser(t *testing.T) {
	u := Create("ali")

	require.NoError(t, u.Err)

	assert.Equal(t, "ali", u.Name)
	assert.Equal(
		t,
		1,
		u.ID,
	)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert!(f.is_empty(), "reformatting must be silent, got {f:#?}");
}

#[test]
fn go_assertions_moved_to_a_helper_are_downgraded_not_accused() {
    let after = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	require.NoError(t, u.Err)
	checkUser(t, u)
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::AssertionRemoved);
    assert_eq!(
        finding.confidence,
        Confidence::Medium,
        "extraction to a helper is not a High-confidence accusation"
    );
}

#[test]
fn go_renaming_a_test_is_not_a_removal() {
    let after = GO_BEFORE.replace("TestCreateUser", "TestCreatesUser");
    let f = run(Lang::Go, GO_BEFORE, &after);
    assert!(
        !has(&f, Rule::AssertionRemoved),
        "a rename must not read as assertion removal: {f:#?}"
    );
}

const RUST_BEFORE: &str = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn creates_user() {
        let u = create("ali");
        assert_eq!(u.name, "ali");
        assert_eq!(u.id, 1);
    }

    #[test]
    #[should_panic(expected = "insufficient funds")]
    fn refund_rejects() {
        refund(500);
    }
}
"#;

#[test]
fn rust_assertions_deleted_outright() {
    let after = RUST_BEFORE.replace("        assert_eq!(u.id, 1);\n", "");
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::AssertionRemoved);
    assert!(finding.detail.contains("creates_user"));
}

#[test]
fn rust_ignore_attribute_added() {
    let after = RUST_BEFORE.replace(
        "    #[test]\n    fn creates_user()",
        "    #[test]\n    #[ignore]\n    fn creates_user()",
    );
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    assert!(has(&f, Rule::TestDisabled), "got {f:#?}");
}

#[test]
fn rust_should_panic_expected_message_dropped() {
    let after = RUST_BEFORE.replace(
        "#[should_panic(expected = \"insufficient funds\")]",
        "#[should_panic]",
    );
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::ExpectedFailureRemoved);
    assert!(
        finding.detail.contains("passes on any failure"),
        "{}",
        finding.detail
    );
}

#[test]
fn rust_should_panic_removed_entirely() {
    let after = RUST_BEFORE.replace(
        "    #[should_panic(expected = \"insufficient funds\")]\n",
        "",
    );
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    assert!(has(&f, Rule::ExpectedFailureRemoved), "got {f:#?}");
}

#[test]
fn rust_assert_downgraded_to_debug_assert() {
    let before = "#[test]\nfn x() { assert_eq!(a, b); }\n";
    let after = "#[test]\nfn x() { debug_assert_eq!(a, b); }\n";
    let f = run(Lang::Rust, before, after);
    let finding = only(&f);
    assert_eq!(
        finding.rule,
        Rule::SeverityDowngraded,
        "debug_assert is compiled out of release builds"
    );
}

#[test]
fn rust_new_test_with_zero_assertions() {
    let after = format!("{RUST_BEFORE}\n#[test]\nfn does_nothing() {{ let _ = 1; }}\n");
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    let finding = only(&f);
    assert_eq!(finding.rule, Rule::TestWithoutAssertions);
    assert!(finding.detail.contains("does_nothing"));
}

#[test]
fn rust_should_panic_test_without_asserts_is_not_flagged() {
    let after = format!(
        "{RUST_BEFORE}\n#[test]\n#[should_panic(expected = \"boom\")]\nfn panics_properly() {{ blow_up(); }}\n"
    );
    let f = run(Lang::Rust, RUST_BEFORE, &after);
    assert!(
        f.is_empty(),
        "a should_panic test asserts by construction: {f:#?}"
    );
}

#[test]
fn rust_reformatting_alone_produces_nothing() {
    let after = r#"
#[cfg(test)]
mod tests {

    #[test]
    fn creates_user() {
        let u = create("ali");

        assert_eq!(
            u.name,
            "ali"
        );
        assert_eq!(
            u.id,
            1
        );
    }

    #[test]
    #[should_panic(expected = "insufficient funds")]
    fn refund_rejects() {
        refund(500);
    }
}
"#;
    let f = run(Lang::Rust, RUST_BEFORE, after);
    assert!(f.is_empty(), "reformatting must be silent, got {f:#?}");
}

#[test]
fn unchanged_input_produces_nothing() {
    assert!(run(Lang::Go, GO_BEFORE, GO_BEFORE).is_empty());
    assert!(run(Lang::Rust, RUST_BEFORE, RUST_BEFORE).is_empty());
}

#[test]
fn pairing_stays_correct_at_scale() {
    use std::fmt::Write;

    let suite = |count: usize, skip: Option<usize>| {
        let mut s = String::from("package p\nimport \"testing\"\n");
        for i in 0..count {
            if Some(i) == skip {
                continue;
            }
            let _ = writeln!(
                s,
                "func TestCase{i}(t *testing.T) {{ assert.Equal(t, {i}, F({i})) }}"
            );
        }
        s
    };

    let before = suite(500, None);
    assert!(
        run(Lang::Go, &before, &before).is_empty(),
        "500 identical tests must produce nothing"
    );

    let after = suite(500, Some(250));
    let f = run(Lang::Go, &before, &after);
    assert_eq!(f.len(), 1, "exactly the one that vanished: {f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
    assert!(f[0].detail.contains("TestCase250"), "{}", f[0].detail);
}

#[test]
fn duplicate_names_still_pair_in_order_at_scale() {
    use std::fmt::Write;
    let mut before = String::from("package p\nimport \"testing\"\n");
    for _ in 0..200 {
        let _ = writeln!(
            before,
            "func TestDup(t *testing.T) {{ assert.Equal(t, 1, F()); assert.True(t, G()) }}"
        );
    }
    let after = before.replacen(
        "func TestDup(t *testing.T) { assert.Equal(t, 1, F()); assert.True(t, G()) }",
        "func TestDup(t *testing.T) { assert.Equal(t, 1, F()) }",
        1,
    );
    let f = run(Lang::Go, &before, &after);
    assert_eq!(f.len(), 1, "only the first occurrence changed: {f:#?}");
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}
