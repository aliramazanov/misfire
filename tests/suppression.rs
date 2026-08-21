use misfire::config::Config;
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Finding, Rule};

fn run(lang: Lang, before: &str, after: &str) -> Vec<Finding> {
    let cfg = Config::default();
    let mut out = Vec::new();
    rules::compare(
        "t",
        &lang.index(before, &cfg),
        &lang.index(after, &cfg),
        &misfire::rules::Options::default(),
        &mut out,
    );
    out
}

const GO_BEFORE: &str = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
"#;

#[test]
fn allow_inside_the_test_suppresses_that_rule() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	// misfire:allow MF101 covered by the integration suite now
	assert.Equal(t, 1, One())
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert!(f.is_empty(), "{f:#?}");
}

#[test]
fn allow_does_not_leak_to_a_sibling_test() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	// misfire:allow MF101 deliberate
	assert.Equal(t, 1, One())
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert_eq!(f.len(), 1, "TestB must still be reported: {f:#?}");
    assert!(f[0].detail.contains("TestB"));
}

#[test]
fn allow_above_the_signature_binds_to_the_next_test() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
// misfire:allow MF101 moved into TestA
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert!(f.is_empty(), "{f:#?}");
}

#[test]
fn allow_is_specific_to_the_named_rule() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	// misfire:allow MF104 unrelated rule
	assert.Equal(t, 1, One())
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert_eq!(f.len(), 1, "MF104 allow must not suppress MF101: {f:#?}");
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}

#[test]
fn several_rules_can_be_allowed_at_once() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	// misfire:allow MF101 MF102 both deliberate
	t.Skip("moved to e2e")
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert!(f.is_empty(), "{f:#?}");
}

#[test]
fn rust_allow_works_the_same_way() {
    let before = "#[test]\nfn a() { assert_eq!(1, 1); assert_eq!(2, 2); }\n";
    let after =
        "#[test]\nfn a() {\n    // misfire:allow MF101 intentional\n    assert_eq!(1, 1);\n}\n";
    assert!(run(Lang::Rust, before, after).is_empty());
}

#[test]
fn a_bare_mention_without_a_rule_id_suppresses_nothing() {
    let after = r#"
package pkg
import "testing"
func TestA(t *testing.T) {
	// misfire:allow because I said so
	assert.Equal(t, 1, One())
}
func TestB(t *testing.T) {
	assert.Equal(t, 1, One())
	assert.Equal(t, 2, Two())
}
"#;
    let f = run(Lang::Go, GO_BEFORE, after);
    assert_eq!(f.len(), 1, "a suppression must name its rule: {f:#?}");
}
