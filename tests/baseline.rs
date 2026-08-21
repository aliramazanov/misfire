use misfire::baseline::{self, Baseline};
use misfire::{Confidence, Finding, Rule};

fn finding(file: &str, line: usize, rule: Rule, test: &str) -> Finding {
    Finding {
        file: file.into(),
        line,
        rule,
        confidence: Confidence::High,
        detail: "detail".into(),
        test: Some(test.into()),
    }
}

#[test]
fn a_baselined_finding_is_suppressed() {
    let found = vec![finding("a_test.go", 10, Rule::AssertionRemoved, "TestA")];
    let b = Baseline::from_findings(&found);
    assert!(baseline::filter(found, &b).is_empty());
}

#[test]
fn the_key_ignores_line_numbers_so_edits_above_do_not_stale_it() {
    let original = vec![finding("a_test.go", 10, Rule::AssertionRemoved, "TestA")];
    let b = Baseline::from_findings(&original);

    let shifted = vec![finding("a_test.go", 93, Rule::AssertionRemoved, "TestA")];
    assert!(
        baseline::filter(shifted, &b).is_empty(),
        "the same finding at a new line must stay suppressed"
    );
}

#[test]
fn a_different_rule_on_a_baselined_test_still_surfaces() {
    let b = Baseline::from_findings(&[finding("a_test.go", 10, Rule::TestDisabled, "TestA")]);
    let new = vec![finding("a_test.go", 10, Rule::AssertionRemoved, "TestA")];
    assert_eq!(baseline::filter(new, &b).len(), 1);
}

#[test]
fn a_different_test_in_a_baselined_file_still_surfaces() {
    let b = Baseline::from_findings(&[finding("a_test.go", 10, Rule::AssertionRemoved, "TestA")]);
    let new = vec![finding("a_test.go", 40, Rule::AssertionRemoved, "TestB")];
    assert_eq!(baseline::filter(new, &b).len(), 1);
}

#[test]
fn entries_are_sorted_and_deduped_so_the_file_does_not_churn() {
    let found = vec![
        finding("z_test.go", 1, Rule::AssertionRemoved, "TestZ"),
        finding("a_test.go", 1, Rule::AssertionRemoved, "TestA"),
        finding("a_test.go", 9, Rule::AssertionRemoved, "TestA"),
    ];
    let b = Baseline::from_findings(&found);
    assert_eq!(
        b.entries.len(),
        2,
        "same test and rule collapses to one entry"
    );
    assert_eq!(b.entries[0].file, "a_test.go");
}

#[test]
fn round_trips_through_disk() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join(".misfire-baseline.json");
    let b = Baseline::from_findings(&[finding("a_test.go", 10, Rule::AssertionRemoved, "TestA")]);
    b.write(&path).unwrap();

    let loaded = Baseline::load(&path).unwrap();
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.entries, b.entries);
}

#[test]
fn a_corrupt_baseline_is_an_error_not_a_silent_pass() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("bad.json");
    std::fs::write(&path, "{ not json").unwrap();
    assert!(Baseline::load(&path).is_err());
}
