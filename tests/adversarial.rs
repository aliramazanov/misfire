use misfire::config::{Config, GoConfig, RustConfig};
use misfire::index::TestFile;
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Confidence, Finding, Rule};

fn idx(lang: Lang, src: &str) -> TestFile {
    lang.index(src, &Config::default())
}

fn run(lang: Lang, before: &str, after: &str) -> Vec<Finding> {
    with(lang, before, after, &Config::default())
}

fn with(lang: Lang, before: &str, after: &str, cfg: &Config) -> Vec<Finding> {
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

#[test]
fn rust_comment_between_attribute_and_signature() {
    let f = idx(
        Lang::Rust,
        "#[test]\n// explains the case\nfn x() { assert_eq!(1,1); }\n",
    );
    assert_eq!(
        f.tests.len(),
        1,
        "a comment must not detach #[test] from its fn"
    );
}

#[test]
fn rust_doc_comment_between_attributes() {
    let f = idx(
        Lang::Rust,
        "#[test]\n/// docs\n#[ignore]\nfn x() { assert_eq!(1,1); }\n",
    );
    assert_eq!(f.tests.len(), 1);
    assert!(
        f.tests[0].disabled.is_some(),
        "#[ignore] must survive the comment"
    );
}

#[test]
fn rust_test_inside_an_impl_block() {
    let src = "#[cfg(test)]\nmod t {\n struct S;\n impl S {\n  #[test]\n  fn x() { assert_eq!(1,1); }\n }\n}\n";
    assert_eq!(idx(Lang::Rust, src).tests.len(), 1);
}

#[test]
fn rust_test_in_a_mod_nested_inside_a_function() {
    let src = "fn outer() {\n #[cfg(test)]\n mod t { #[test] fn x() { assert_eq!(1,1); } }\n}\n";
    assert_eq!(idx(Lang::Rust, src).tests.len(), 1);
}

#[test]
fn go_conditional_skip_is_a_guard_not_a_disabled_test() {
    let before =
        "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) {\n if testing.Short() { t.Skip(\"slow\") }\n assert.Equal(t,1,1) }\n";
    assert!(
        run(Lang::Go, before, after).is_empty(),
        "if testing.Short() {{ t.Skip() }} is the standard idiom, not a weakening"
    );
}

#[test]
fn go_unconditional_skip_is_still_caught() {
    let before =
        "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { t.Skip(\"broken\")\n assert.Equal(t,1,1) }\n";
    let f = run(Lang::Go, before, after);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].rule, Rule::TestDisabled);
}

#[test]
fn go_testing_tb_helper_is_not_a_test() {
    let f = idx(
        Lang::Go,
        "package p\nimport \"testing\"\nfunc TestHelperThing(t testing.TB) { assert.Equal(t,1,1) }\n",
    );
    assert!(
        f.tests.is_empty(),
        "testing.TB is a helper signature, not a test"
    );
    assert_eq!(
        f.helpers.len(),
        1,
        "but it is still worth indexing as a helper"
    );
}

#[test]
fn go_assertions_behind_a_local_helper_still_count() {
    let src = "package p\nimport \"testing\"\n\
        func expectSuccess(out string, err error, t *testing.T) {\n \
        if err != nil { t.Fatalf(\"bad\") }\n }\n\
        func TestX(t *testing.T) { expectSuccess(o, e, t) }\n";
    let f = idx(Lang::Go, src);
    let t = f.get("TestX").unwrap();
    assert_eq!(
        t.assertion_count(),
        0,
        "no assertion is written in the test itself"
    );
    assert_eq!(
        f.effective_assertions(t),
        1,
        "but the helper it calls makes one"
    );
}

#[test]
fn go_new_test_delegating_to_a_helper_is_not_accused() {
    let before =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = format!("{before}func TestB(t *testing.T) {{ checkOmit(t, output, \"x\") }}\n");
    assert!(
        run(Lang::Go, before, &after).is_empty(),
        "a helper defined in a sibling file still receives t, so the test does assert"
    );
}

#[test]
fn go_new_test_that_asserts_nothing_at_all_is_still_caught() {
    let before =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = format!("{before}func TestB(t *testing.T) {{ Refund(500) }}\n");
    let f = run(Lang::Go, before, &after);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].rule, Rule::TestWithoutAssertions);
}

#[test]
fn duplicate_test_names_pair_by_occurrence() {
    let src = "#[test]\nfn error() { assert!(a); assert_eq!(b, c); }\n\
               #[test]\nfn error() { assert_eq!(d, e); assert_eq!(f, g); }\n";
    assert!(
        run(Lang::Rust, src, src).is_empty(),
        "an unchanged file with duplicate test names must be silent"
    );
}

#[test]
fn duplicate_names_do_not_mask_a_real_removal() {
    let before = "#[test]\nfn error() { assert!(a); assert_eq!(b, c); }\n\
                  #[test]\nfn error() { assert_eq!(d, e); assert_eq!(f, g); }\n";
    let after = "#[test]\nfn error() { assert!(a); assert_eq!(b, c); }\n\
                 #[test]\nfn error() { assert_eq!(d, e); }\n";
    let f = run(Lang::Rust, before, after);
    assert_eq!(
        f.len(),
        1,
        "the second occurrence lost an assertion: {f:#?}"
    );
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}

#[test]
fn suppression_tolerates_comma_separated_ids() {
    let before = "#[test]\nfn a() { assert_eq!(1,1); assert_eq!(2,2); }\n";
    let after = "#[test]\nfn a() {\n // misfire:allow MF101, MF102 reason\n assert_eq!(1,1); }\n";
    assert!(run(Lang::Rust, before, after).is_empty());
}

#[test]
fn a_test_the_grammar_could_not_read_is_not_compared() {
    let broken = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1)\n";
    let good = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1) }\n";
    let f = idx(Lang::Go, broken);
    assert_eq!(f.untrusted().count(), 1, "the damaged test must be marked");
    assert!(
        run(Lang::Go, good, broken).is_empty(),
        "a test we could not read must not be compared as though we had"
    );
}

#[test]
fn one_unreadable_construct_does_not_disable_the_whole_file() {
    let before = "fn helper() -> Result<()> { try!(go()); Ok(()) }\n                  #[test]\nfn a() { assert_eq!(1,1); assert_eq!(2,2); }\n";
    let after = "fn helper() -> Result<()> { try!(go()); Ok(()) }\n                 #[test]\nfn a() { assert_eq!(1,1); }\n";

    let f = idx(Lang::Rust, before);
    assert_eq!(f.tests.len(), 1);
    assert!(
        f.tests[0].trusted,
        "the error is in helper(), not in the test"
    );

    let found = run(Lang::Rust, before, after);
    assert_eq!(
        found.len(),
        1,
        "the removal must still be caught: {found:#?}"
    );
    assert_eq!(found[0].rule, Rule::AssertionRemoved);
}

#[test]
fn splitting_a_test_is_not_a_high_confidence_accusation() {
    let before = "package p\nimport \"testing\"\nfunc TestAll(t *testing.T) { assert.Equal(t,1,1); assert.Equal(t,2,2) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestAll(t *testing.T) { assert.Equal(t,1,1) }\nfunc TestSecond(t *testing.T) { assert.Equal(t,2,2) }\n";
    let f = run(Lang::Go, before, after);
    assert!(
        f.iter().all(|x| x.confidence == Confidence::Medium),
        "a split is a refactor, not a High accusation: {f:#?}"
    );
}

#[test]
fn removing_every_assertion_reports_once() {
    let before = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Error(t, err); assert.Equal(t,1,1) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { }\n";
    let f = run(Lang::Go, before, after);
    assert_eq!(f.len(), 1, "one act should not be reported twice: {f:#?}");
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}

#[test]
fn a_suppression_inside_a_string_literal_does_not_suppress() {
    let before = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1); assert.Equal(t,2,2) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { s := \"misfire:allow MF101\"; assert.Equal(t,1,1) }\n";
    assert!(!run(Lang::Go, before, after).is_empty());
}

#[test]
fn an_invalid_glob_is_rejected_rather_than_ignored() {
    let cfg = Config {
        go: GoConfig {
            assertion_patterns: vec!["[unclosed".into()],
            ..GoConfig::default()
        },
        ..Config::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn an_unknown_rule_id_in_config_is_rejected() {
    let cfg: Config = toml::from_str("[rules]\ndisabled = [\"MF999\"]\n").unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn a_custom_rust_assert_macro_can_be_declared() {
    let before = "#[test]\nfn x() { assert_eq_printed!(a, b); }\n";
    let after = "#[test]\nfn x() { let _ = a; }\n";
    assert!(
        run(Lang::Rust, before, after).is_empty(),
        "an unknown macro is not assumed to assert"
    );

    let cfg = Config {
        rust: RustConfig {
            assertion_patterns: vec!["assert_eq_printed".into()],
            ..RustConfig::default()
        },
        ..Config::default()
    };
    let f = with(Lang::Rust, before, after, &cfg);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}

#[test]
fn empty_and_whitespace_sources_are_harmless() {
    assert!(idx(Lang::Go, "").tests.is_empty());
    assert!(idx(Lang::Rust, "   \n\n  ").tests.is_empty());
}

#[test]
fn language_detection_handles_awkward_paths() {
    use misfire::lang::Lang;
    assert_eq!(Lang::for_path("a_test.go"), Some(Lang::Go));
    assert_eq!(Lang::for_path("a.rs"), Some(Lang::Rust));
    assert_eq!(
        Lang::for_path("A.RS"),
        Some(Lang::Rust),
        "case-insensitive filesystems exist"
    );
    assert_eq!(
        Lang::for_path("a.go"),
        None,
        "a non-test Go file holds no tests"
    );
    assert_eq!(
        Lang::for_path(".rs"),
        None,
        "a dotfile named .rs is not a Rust source file"
    );
    assert_eq!(Lang::for_path("rs"), None);
    assert_eq!(Lang::for_path(""), None);
    assert_eq!(Lang::for_path("src/lib.rs.bak"), None);
    assert_eq!(Lang::for_path("weird/a_test.go"), Some(Lang::Go));
}

#[test]
fn go_only_runs_functions_with_the_exact_test_signature() {
    let extra = "package p\nimport \"testing\"\nfunc TestX(t *testing.T, extra int) { assert.Equal(t,1,1) }\n";
    let f = idx(Lang::Go, extra);
    assert!(f.tests.is_empty(), "extra parameters mean it never runs");
    assert_eq!(
        f.helpers.len(),
        1,
        "but it is still a helper worth resolving"
    );

    let ok = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t,1,1) }\n";
    assert_eq!(idx(Lang::Go, ok).tests.len(), 1);

    let main = "package p\nimport \"testing\"\nfunc TestMain(m *testing.M) { m.Run() }\n";
    assert!(
        idx(Lang::Go, main).tests.is_empty(),
        "TestMain is not a test"
    );
}

#[test]
fn deeply_nested_source_does_not_crash_the_process() {
    for depth in [64usize, 512, 5000] {
        let rust = format!(
            "#[test]\nfn deep() {{\n{}assert_eq!(1, 1);{}\n}}\n",
            "if true {\n".repeat(depth),
            "\n}".repeat(depth)
        );
        let f = idx(Lang::Rust, &rust);
        assert_eq!(f.tests.len(), 1, "rust at depth {depth}");

        let go = format!(
            "package p\nimport \"testing\"\nfunc TestDeep(t *testing.T) {{\n{}assert.Equal(t,1,1){}\n}}\n",
            "if true {\n".repeat(depth),
            "\n}".repeat(depth)
        );
        let g = idx(Lang::Go, &go);
        assert_eq!(g.tests.len(), 1, "go at depth {depth}");
    }
}

#[test]
fn a_test_too_deep_to_read_is_marked_untrusted_not_guessed_at() {
    let depth = deep_enough();
    let rust = format!(
        "#[test]\nfn deep() {{\n{}assert_eq!(1, 1);{}\n}}\n",
        "if true {\n".repeat(depth),
        "\n}".repeat(depth)
    );
    let f = idx(Lang::Rust, &rust);
    assert_eq!(
        f.untrusted().count(),
        1,
        "past the depth bound misfire stops looking, and says so"
    );
}

fn deep_enough() -> usize {
    misfire::index::MAX_DEPTH + 50
}

#[test]
fn a_mock_assertion_method_counts_as_asserting() {
    let before = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { m.On(\"X\") }\n";
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) { m.On(\"X\") }\n\
        func TestB(t *testing.T) {\n\tm := &Mock{}\n\tm.On(\"Method\")\n\tm.AssertExpectations(t)\n}\n";
    assert!(
        run(Lang::Go, before, after).is_empty(),
        "anything handed the testing handle can fail the test"
    );
}

#[test]
fn a_method_call_that_never_sees_t_is_still_not_an_assertion() {
    let before =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) { assert.Equal(t,1,1) }\n\
        func TestB(t *testing.T) {\n\tm := &Mock{}\n\tm.On(\"Method\")\n\tm.Reset()\n}\n";
    let f = run(Lang::Go, before, after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestWithoutAssertions);
}

#[test]
fn an_unreadable_arrival_does_not_soften_a_real_finding() {
    let before = "#[test]\nfn a() { assert_eq!(1,1); assert_eq!(2,2); }\n";
    let after = "#[test]\nfn a() { assert_eq!(1,1); }\n\
                 #[test]\nfn broken() { let s = str![[r#\"x # y\"#]]; assert_eq!(3,3); }\n";
    let f = idx(Lang::Rust, after);
    assert!(
        f.untrusted().count() == 1,
        "the fixture must actually contain an unreadable test"
    );

    let found = run(Lang::Rust, before, after);
    let removal = found
        .iter()
        .find(|x| x.rule == Rule::AssertionRemoved)
        .unwrap_or_else(|| panic!("expected the removal to be reported: {found:#?}"));
    assert_eq!(
        removal.confidence,
        Confidence::High,
        "nothing readable arrived, so there is no refactor to give it the benefit of"
    );
}
