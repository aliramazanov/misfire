use misfire::config::Config;
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Confidence, Finding, Rule};

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

const GO_TWO: &str = "package p\nimport \"testing\"\n\
    func TestA(t *testing.T){ assert.Equal(t,1,1) }\n\
    func TestB(t *testing.T){ assert.Equal(t,2,2); assert.Equal(t,3,3) }\n";

const GO_ONE: &str =
    "package p\nimport \"testing\"\nfunc TestA(t *testing.T){ assert.Equal(t,1,1) }\n";

#[test]
fn deleting_a_whole_test_function_is_reported() {
    let f = run(Lang::Go, GO_TWO, GO_ONE);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
    assert_eq!(f[0].confidence, Confidence::High);
    assert!(f[0].detail.contains("TestB"), "{}", f[0].detail);
    assert!(f[0].detail.contains("2 assertions"), "{}", f[0].detail);
}

#[test]
fn deleting_a_rust_test_is_reported() {
    let before = "#[test]\nfn a(){ assert_eq!(1,1); }\n#[test]\nfn b(){ assert_eq!(2,2); }\n";
    let after = "#[test]\nfn a(){ assert_eq!(1,1); }\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn removing_the_test_attribute_stops_the_test_running() {
    let before = "#[test]\nfn a(){ assert_eq!(1,1); }\n#[test]\nfn b(){ assert_eq!(2,2); }\n";
    let after = "#[test]\nfn a(){ assert_eq!(1,1); }\nfn b(){ assert_eq!(2,2); }\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(f.len(), 1, "the body survives but nothing runs it: {f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn renaming_a_go_test_out_of_the_test_prefix_stops_it_running() {
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n\
        func checkB(t *testing.T){ assert.Equal(t,2,2); assert.Equal(t,3,3) }\n";
    let f = run(Lang::Go, GO_TWO, after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn a_pure_rename_is_recognised_not_accused() {
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n\
        func TestBetterName(t *testing.T){ assert.Equal(t,2,2); assert.Equal(t,3,3) }\n";
    assert!(
        run(Lang::Go, GO_TWO, after).is_empty(),
        "the body is identical, so the test was renamed, not removed"
    );
}

#[test]
fn a_rename_only_absorbs_one_removal() {
    let before = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ setup(); assert.Equal(t,1,1); teardown() }\n\
        func TestB(t *testing.T){ setup(); assert.Equal(t,1,1); teardown() }\n";
    let after = "package p\nimport \"testing\"\n\
        func TestRenamed(t *testing.T){ setup(); assert.Equal(t,1,1); teardown() }\n";
    let f = run(Lang::Go, before, after);
    assert_eq!(
        f.len(),
        1,
        "one arrival can only account for one departure: {f:#?}"
    );
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn removing_a_test_that_asserted_nothing_is_not_a_loss() {
    let before = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n\
        func TestEmpty(t *testing.T){ Refund(1) }\n";
    assert!(
        run(Lang::Go, before, GO_ONE).is_empty(),
        "deleting a test that verified nothing removes no coverage"
    );
}

#[test]
fn removing_an_already_disabled_test_is_not_a_loss() {
    let before = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n\
        func TestSkipped(t *testing.T){ t.Skip(\"x\"); assert.Equal(t,2,2) }\n";
    assert!(
        run(Lang::Go, before, GO_ONE).is_empty(),
        "it was already not running"
    );
}

#[test]
fn a_nested_helper_is_not_counted_twice() {
    let cfg = Config::default();
    let src = "#[test]\nfn x(){\n fn helper(){ assert_eq!(1,1); }\n helper();\n}\n";
    let f = Lang::Rust.index(src, &cfg);
    let t = f.get("x").unwrap();
    assert_eq!(
        t.assertion_count(),
        0,
        "the assertion belongs to the helper"
    );
    assert_eq!(f.effective_assertions(t), 1, "and is counted exactly once");
}

#[test]
fn narrowing_a_should_panic_message_is_a_weakening() {
    let before = "#[test]\n#[should_panic(expected = \"insufficient funds\")]\nfn a(){ boom(); }\n";
    let after = "#[test]\n#[should_panic(expected = \"i\")]\nfn a(){ boom(); }\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::ExpectedFailureRemoved);
    assert!(
        f[0].detail.contains("matches more failures"),
        "{}",
        f[0].detail
    );
}

#[test]
fn rewriting_a_should_panic_message_to_something_else_is_not_assumed_weaker() {
    let before = "#[test]\n#[should_panic(expected = \"insufficient funds\")]\nfn a(){ boom(); }\n";
    let after = "#[test]\n#[should_panic(expected = \"account is frozen\")]\nfn a(){ boom(); }\n";
    assert!(
        run(Lang::Rust, before, after).is_empty(),
        "a different message is a change, not necessarily a weakening"
    );
}

#[test]
fn adding_an_assertion_is_never_a_finding() {
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1); assert.Equal(t,9,9) }\n\
        func TestB(t *testing.T){ assert.Equal(t,2,2); assert.Equal(t,3,3) }\n";
    assert!(run(Lang::Go, GO_TWO, after).is_empty());
}

#[test]
fn adding_a_whole_test_is_never_a_finding() {
    let after = format!("{GO_TWO}func TestC(t *testing.T){{ assert.Equal(t,4,4) }}\n");
    assert!(run(Lang::Go, GO_TWO, &after).is_empty());
}

#[test]
fn a_removed_test_can_be_allowed_from_the_file_that_lost_it() {
    let after = "package p\nimport \"testing\"\n\
        // misfire:allow MF108 TestB moved to the e2e suite\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n";
    assert!(
        run(Lang::Go, GO_TWO, after).is_empty(),
        "a deleted test has no body left to annotate, so the file is the anchor"
    );
}

#[test]
fn a_file_level_allow_is_specific_to_its_rule() {
    let after = "package p\nimport \"testing\"\n\
        // misfire:allow MF101 unrelated\n\
        func TestA(t *testing.T){ assert.Equal(t,1,1) }\n";
    let f = run(Lang::Go, GO_TWO, after);
    assert_eq!(f.len(), 1, "MF101 allow must not silence MF108: {f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn an_unrelated_arrival_cannot_hide_a_deletion() {
    let before = "#[test]\nfn payment_is_charged(){ assert_eq!(a, b); }\n\
                  #[test]\nfn keeps_running(){ assert_eq!(c, d); }\n";
    let after = "#[test]\nfn totally_unrelated(){ assert_eq!(x, y); }\n\
                 #[test]\nfn keeps_running(){ assert_eq!(c, d); }\n";
    let f = run(Lang::Rust, before, after);
    assert!(
        f.iter().any(|x| x.rule == Rule::TestRemoved),
        "one generic assertion is not evidence of a rename: {f:#?}"
    );
    assert!(
        f.iter().all(|x| x.confidence == Confidence::Medium),
        "but the file did gain a test, so it stays short of High"
    );
}

#[test]
fn a_distinctive_body_still_reads_as_a_rename() {
    let before = "#[test]\nfn old_name(){ setup(); assert_eq!(a, b); teardown(); }\n";
    let after = "#[test]\nfn new_name(){ setup(); assert_eq!(a, b); teardown(); }\n";
    assert!(
        run(Lang::Rust, before, after).is_empty(),
        "three matching calls is evidence enough"
    );
}

#[test]
fn gating_a_test_behind_a_feature_stops_it_running() {
    let before = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    let after = "#[cfg(test)]\nmod tests {\n    #[test]\n    #[cfg(feature = \"slow\")]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestDisabled);
    assert!(f[0].detail.contains("feature"), "{}", f[0].detail);
}

#[test]
fn gating_the_module_stops_every_test_in_it() {
    let before = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() { assert_eq!(1, 1); }\n    #[test]\n    fn b() { assert_eq!(2, 2); }\n}\n";
    let after = "#[cfg(test)]\n#[cfg(feature = \"slow\")]\nmod tests {\n    #[test]\n    fn a() { assert_eq!(1, 1); }\n    #[test]\n    fn b() { assert_eq!(2, 2); }\n}\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(f.len(), 2, "both tests stopped running: {f:#?}");
    assert!(f.iter().all(|x| x.rule == Rule::TestDisabled));
}

#[test]
fn the_ordinary_cfg_test_module_is_not_a_gate() {
    let src = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    assert!(
        run(Lang::Rust, src, src).is_empty(),
        "this is how every Rust unit test is written"
    );
}

#[test]
fn a_platform_condition_is_not_treated_as_a_gate() {
    let before = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    let after = "#[cfg(test)]\nmod tests {\n    #[test]\n    #[cfg(unix)]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    assert!(run(Lang::Rust, before, after).is_empty());
}

#[test]
fn a_test_that_was_always_gated_is_not_re_reported() {
    let src = "#[cfg(test)]\nmod tests {\n    #[test]\n    #[cfg(feature = \"slow\")]\n    fn a() { assert_eq!(1, 1); }\n}\n";
    assert!(run(Lang::Rust, src, src).is_empty());
}

#[test]
fn each_way_of_disabling_a_test_gets_its_own_sentence() {
    let go_before =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let go_after = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { t.Skip(\"x\"); assert.Equal(t,1,1) }\n";
    let skipped = run(Lang::Go, go_before, go_after);
    assert!(
        skipped[0].detail.contains("now calls t.Skip"),
        "{}",
        skipped[0].detail
    );

    let rs_before = "#[test]\nfn a() { assert_eq!(1,1); }\n";
    let ignored = run(
        Lang::Rust,
        rs_before,
        "#[test]\n#[ignore]\nfn a() { assert_eq!(1,1); }\n",
    );
    assert!(
        ignored[0].detail.contains("carries #[ignore]"),
        "an attribute is not called: {}",
        ignored[0].detail
    );

    let gated = run(
        Lang::Rust,
        rs_before,
        "#[test]\n#[cfg(feature = \"slow\")]\nfn a() { assert_eq!(1,1); }\n",
    );
    assert!(
        gated[0].detail.contains("gated behind")
            && gated[0].detail.contains("does not run by default"),
        "a build-configuration gate is not a skip: {}",
        gated[0].detail
    );
}
