use misfire::report::github_annotations;
use misfire::{Confidence, Finding, Rule};

fn finding(file: &str, detail: &str, confidence: Confidence) -> Finding {
    Finding {
        file: file.into(),
        line: 3,
        rule: Rule::TestWithoutAssertions,
        confidence,
        detail: detail.into(),
        test: Some("TestB".into()),
    }
}

#[test]
fn a_newline_in_a_path_cannot_inject_a_workflow_command() {
    let evil = "evil\n::error file=x,line=1::injected\n_test.go";
    let out = github_annotations(&[finding(evil, "detail", Confidence::High)]);

    assert_eq!(
        out.lines().count(),
        1,
        "a path is data, and data must not become a second command: {out}"
    );
    assert!(!out.contains("::injected"), "{out}");
    assert!(out.contains("%0A"), "the newline must be encoded: {out}");
}

#[test]
fn colons_and_commas_in_a_path_are_encoded() {
    let out = github_annotations(&[finding("a:b,c_test.go", "detail", Confidence::High)]);
    assert!(out.contains("a%3Ab%2Cc_test.go"), "{out}");
}

#[test]
fn a_newline_in_the_message_cannot_inject_either() {
    let out = github_annotations(&[finding("a_test.go", "one\n::error::two", Confidence::High)]);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("%0A"), "{out}");
}

#[test]
fn percent_is_escaped_first_so_encodings_are_not_doubled() {
    let out = github_annotations(&[finding("a_test.go", "100%", Confidence::High)]);
    assert!(out.contains("100%25"), "{out}");
}

#[test]
fn confidence_selects_the_annotation_level() {
    let high = github_annotations(&[finding("a_test.go", "d", Confidence::High)]);
    let med = github_annotations(&[finding("a_test.go", "d", Confidence::Medium)]);
    assert!(high.starts_with("::error"), "{high}");
    assert!(med.starts_with("::warning"), "{med}");
}
