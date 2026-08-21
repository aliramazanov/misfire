use misfire::config::Config;
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Confidence, Finding, Rule};

fn run(before: &str, after: &str) -> Vec<Finding> {
    let cfg = Config::default();
    let mut out = Vec::new();
    rules::compare(
        "t",
        &Lang::Go.index(before, &cfg),
        &Lang::Go.index(after, &cfg),
        &misfire::rules::Options::default(),
        &mut out,
    );
    out
}

const TESTS: &str = "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\n\
    func TestA(t *testing.T) { assert.Equal(t, 1, One()) }\n\
    func TestB(t *testing.T) { assert.Equal(t, 2, Two()) }\n";

fn with_main(body: &str) -> String {
    format!("{TESTS}\nfunc TestMain(m *testing.M) {{ {body} }}\n")
}

#[test]
fn gutting_test_main_silences_the_whole_package() {
    let f = run(&with_main("os.Exit(m.Run())"), &with_main("setupOnly()"));
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::SuiteNotRun);
    assert_eq!(f[0].confidence, Confidence::High);
    assert!(
        f[0].detail.contains("none of the 2 tests"),
        "{}",
        f[0].detail
    );
}

#[test]
fn adding_a_test_main_that_never_runs_is_caught() {
    let f = run(TESTS, &with_main("setupOnly()"));
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::SuiteNotRun);
}

#[test]
fn a_test_main_that_runs_the_suite_is_fine() {
    assert!(run(TESTS, &with_main("os.Exit(m.Run())")).is_empty());
    assert!(run(&with_main("m.Run()"), &with_main("os.Exit(m.Run())")).is_empty());
}

#[test]
fn a_test_main_that_never_ran_the_suite_is_not_re_reported() {
    assert!(
        run(&with_main("setupOnly()"), &with_main("setupOnly()")).is_empty(),
        "misfire reports what the change did, not what was already there"
    );
}

#[test]
fn a_run_call_nested_in_a_defer_or_block_still_counts() {
    let nested = with_main("code := m.Run()\n\tos.Exit(code)");
    assert!(run(TESTS, &nested).is_empty());

    let conditional = with_main("if setup() {\n\t\tos.Exit(m.Run())\n\t}");
    assert!(run(TESTS, &conditional).is_empty());
}

#[test]
fn a_renamed_receiver_is_still_understood() {
    let f = run(
        &format!("{TESTS}\nfunc TestMain(mm *testing.M) {{ os.Exit(mm.Run()) }}\n"),
        &format!("{TESTS}\nfunc TestMain(mm *testing.M) {{ setupOnly() }}\n"),
    );
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::SuiteNotRun);
}

#[test]
fn the_rule_can_be_allowed_from_the_file() {
    let allowed = format!(
        "{TESTS}\n// misfire:allow MF109 the suite runs elsewhere\nfunc TestMain(m *testing.M) {{ setupOnly() }}\n"
    );
    assert!(run(&with_main("os.Exit(m.Run())"), &allowed).is_empty());
}

#[test]
fn skipping_a_subtest_says_so_rather_than_condemning_the_parent() {
    let before = "package p\nimport \"testing\"\n\
        func TestX(t *testing.T) {\n\
        \tt.Run(\"one\", func(t *testing.T) { assert.Equal(t, 1, One()) })\n\
        \tt.Run(\"two\", func(t *testing.T) { assert.Equal(t, 2, Two()) })\n}\n";
    let after = "package p\nimport \"testing\"\n\
        func TestX(t *testing.T) {\n\
        \tt.Run(\"one\", func(t *testing.T) { t.Skip(\"flaky\"); assert.Equal(t, 1, One()) })\n\
        \tt.Run(\"two\", func(t *testing.T) { assert.Equal(t, 2, Two()) })\n}\n";

    let f = run(before, after);
    let disabled = f
        .iter()
        .find(|x| x.rule == Rule::TestDisabled)
        .unwrap_or_else(|| panic!("expected a disabled finding: {f:#?}"));
    assert!(
        disabled.detail.contains("subtest \"one\""),
        "must name the subtest rather than imply the whole test stopped: {}",
        disabled.detail
    );
}

#[test]
fn skipping_the_whole_test_still_says_the_whole_test() {
    let before =
        "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { assert.Equal(t, 1, One()) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { t.Skip(\"off\"); assert.Equal(t, 1, One()) }\n";
    let f = run(before, after);
    assert_eq!(f[0].rule, Rule::TestDisabled);
    assert!(!f[0].detail.contains("subtest"), "{}", f[0].detail);
    assert!(
        f[0].detail.starts_with("TestX now calls"),
        "{}",
        f[0].detail
    );
}

fn exclusion(before: &str, after: &str) -> Vec<Finding> {
    let cfg = Config::default();
    let mut out = Vec::new();
    misfire::rules::file_excluded(
        "t",
        &Lang::Go.index(before, &cfg),
        &Lang::Go.index(after, &cfg),
        &misfire::rules::Options::default(),
        &mut out,
    );
    out
}

#[test]
fn a_build_constraint_hides_every_test_in_the_file() {
    let before = "package p\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) { assert.Equal(t, 1, One()) }\n";
    let after = format!("//go:build slow\n\n{before}");
    let f = exclusion(before, &after);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::FileExcluded);
    assert!(f[0].detail.contains("slow"), "{}", f[0].detail);
}

#[test]
fn a_file_that_was_always_tagged_is_not_re_reported() {
    let tagged = "//go:build slow\n\npackage p\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) { assert.Equal(t, 1, One()) }\n";
    assert!(exclusion(tagged, tagged).is_empty());
}

#[test]
fn a_constraint_below_the_package_clause_is_not_one() {
    let before = "package p\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) { assert.Equal(t, 1, One()) }\n";
    let after = "package p\n\n//go:build slow\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) { assert.Equal(t, 1, One()) }\n";
    assert!(
        exclusion(before, after).is_empty(),
        "go only honours the constraint above the package clause"
    );
}

#[test]
fn an_example_is_a_test_only_while_it_declares_its_output() {
    let with_output =
        "package p\n\nimport \"fmt\"\n\nfunc ExampleN() {\n\tfmt.Println(N())\n\t// Output: 1\n}\n";
    let without = "package p\n\nimport \"fmt\"\n\nfunc ExampleN() {\n\tfmt.Println(N())\n}\n";

    let cfg = Config::default();
    assert_eq!(Lang::Go.index(with_output, &cfg).tests.len(), 1);
    assert!(
        Lang::Go.index(without, &cfg).tests.is_empty(),
        "without the comment go reports [no tests to run]"
    );

    let f = run(with_output, without);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::TestRemoved);
}

#[test]
fn a_fuzz_target_asserts_through_its_closure() {
    let cfg = Config::default();
    let src = "package p\n\nimport \"testing\"\n\n\
        func FuzzN(f *testing.F) {\n\
        \tf.Fuzz(func(t *testing.T, s string) {\n\
        \t\tif N(s) != s { t.Errorf(\"bad\") }\n\
        \t})\n}\n";
    let file = Lang::Go.index(src, &cfg);
    let target = file.get("FuzzN").expect("fuzz targets run as tests");
    assert_eq!(
        target.assertion_count(),
        1,
        "the closure has its own testing handle"
    );
}

#[test]
fn losing_only_a_setup_guard_is_not_a_high_confidence_accusation() {
    let before = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) {\n\
        \tf, err := open()\n\
        \tif err != nil { t.Fatal(\"could not open\") }\n\
        \tassert.Equal(t, 1, f.N())\n}\n";
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) {\n\
        \tf := mustOpen(t)\n\
        \tassert.Equal(t, 1, f.N())\n}\n";
    let f = run(before, after);
    assert!(
        f.iter().all(|x| x.confidence != Confidence::High),
        "the verification is untouched, only the fixture guard moved: {f:#?}"
    );
}

#[test]
fn losing_a_real_assertion_is_still_high_confidence() {
    let before = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) {\n\
        \tif err != nil { t.Fatal(\"setup\") }\n\
        \tassert.Equal(t, 1, One())\n\tassert.Equal(t, 2, Two())\n}\n";
    let after = "package p\nimport \"testing\"\n\
        func TestA(t *testing.T) {\n\
        \tif err != nil { t.Fatal(\"setup\") }\n\
        \tassert.Equal(t, 1, One())\n}\n";
    let f = run(before, after);
    assert_eq!(f[0].confidence, Confidence::High, "{f:#?}");
}

#[test]
fn example_naming_follows_gos_rules() {
    let cfg = Config::default();
    let example = |name: &str| {
        let src = format!(
            "package p\n\nimport \"fmt\"\n\nfunc {name}() {{\n\tfmt.Println(N())\n\t// Output: 1\n}}\n"
        );
        Lang::Go.index(&src, &cfg).tests.len()
    };

    assert_eq!(example("Example"), 1);
    assert_eq!(example("ExampleN"), 1);
    assert_eq!(
        example("ExampleN_withSuffix"),
        1,
        "a lowercase suffix is legal"
    );
    assert_eq!(example("Example_suffix"), 1, "so is a package-level suffix");
    assert_eq!(example("Examples"), 0, "a plural is a plain function");
    assert_eq!(example("ExampleHelper"), 1);
}

#[test]
fn an_example_delegating_to_a_helper_is_not_assertion_free() {
    let cfg = Config::default();
    let src = "package p\n\nimport \"fmt\"\n\n\
        func ExampleN() {\n\tshowIt()\n\tfmt.Println(N())\n\t// Output: 1\n}\n";
    let file = Lang::Go.index(src, &cfg);
    let ex = file.get("ExampleN").unwrap();
    assert!(
        ex.helper_calls.contains(&"showIt".to_string()),
        "the body is read, not just the output comment: {:?}",
        ex.helper_calls
    );
}

#[test]
fn an_example_the_grammar_could_not_read_is_not_trusted() {
    let cfg = Config::default();

    let src =
        "package p\n\nimport \"fmt\"\n\nfunc ExampleN() {\n\tfmt.Println(N()\n\t// Output: 1\n}\n";
    let file = Lang::Go.index(src, &cfg);
    if let Some(ex) = file.get("ExampleN") {
        assert!(
            !ex.trusted,
            "an example is not exempt from the trust check other tests get"
        );
    }
}
