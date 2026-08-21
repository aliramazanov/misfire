use misfire::config::{Config, GoConfig, RustConfig};
use misfire::lang::Lang;
use misfire::rules;
use misfire::{Finding, Rule};

fn run(lang: Lang, before: &str, after: &str, cfg: &Config) -> Vec<Finding> {
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

const GO_CUSTOM_BEFORE: &str = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
	expectUserToMatch(t, u, "ali")
	testutil.AssertID(t, u, 1)
}
"#;

const GO_CUSTOM_AFTER: &str = r#"
package pkg
import "testing"
func TestCreateUser(t *testing.T) {
	u := Create("ali")
}
"#;

#[test]
fn custom_go_assertions_are_invisible_without_config() {
    let f = run(
        Lang::Go,
        GO_CUSTOM_BEFORE,
        GO_CUSTOM_AFTER,
        &Config::default(),
    );
    assert!(
        f.is_empty(),
        "a project-specific matcher cannot be guessed, so silence is correct here: {f:#?}"
    );
}

#[test]
fn custom_go_assertions_are_caught_once_declared() {
    let cfg = Config {
        go: GoConfig {
            assertion_patterns: vec!["expect*".into(), "testutil.*".into()],
            ..GoConfig::default()
        },
        ..Config::default()
    };
    let f = run(Lang::Go, GO_CUSTOM_BEFORE, GO_CUSTOM_AFTER, &cfg);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
    assert!(
        f[0].detail
            .contains("2 assertions removed from TestCreateUser")
    );
}

#[test]
fn custom_rust_assertion_macro_is_caught_once_declared() {
    let before = "#[test]\nfn x() { assert_user_matches!(u, \"ali\"); }\n";
    let after = "#[test]\nfn x() { let _ = u; }\n";

    assert!(
        run(Lang::Rust, before, after, &Config::default()).is_empty(),
        "unknown macro is not assumed to be an assertion"
    );

    let cfg = Config {
        rust: RustConfig {
            assertion_patterns: vec!["assert_*".into()],
            ..RustConfig::default()
        },
        ..Config::default()
    };
    let f = run(Lang::Rust, before, after, &cfg);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::AssertionRemoved);
}

#[test]
fn a_declared_fatal_helper_downgrade_is_caught() {
    let cfg = Config {
        go: GoConfig {
            assertion_patterns: vec!["mustEqual".into(), "shouldEqual".into()],
            fatal_patterns: vec!["mustEqual".into()],
            ..GoConfig::default()
        },
        ..Config::default()
    };
    let before = "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { mustEqual(t, 1, 2) }\n";
    let after =
        "package p\nimport \"testing\"\nfunc TestX(t *testing.T) { shouldEqual(t, 1, 2) }\n";
    let f = run(Lang::Go, before, after, &cfg);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, Rule::SeverityDowngraded);
}

#[test]
fn parses_a_real_config_file() {
    let cfg: Config = toml::from_str(
        r#"
[go]
assertion_patterns = ["testutil.*"]

[rules]
disabled = ["MF104"]
file_deleted = true
"#,
    )
    .unwrap();

    assert_eq!(cfg.go.assertion_patterns, ["testutil.*"]);
    assert!(cfg.rules.file_deleted);
    assert!(!cfg.rule_enabled("MF104"));
    assert!(cfg.rule_enabled("MF101"));
    assert!(
        cfg.go.fatal_packages.contains(&"require".to_string()),
        "unspecified sections must keep their defaults"
    );
}

#[test]
fn a_typo_in_config_is_an_error_not_a_silent_default() {
    let err = toml::from_str::<Config>("[go]\nassertion_pattern = [\"x\"]\n").unwrap_err();
    assert!(err.to_string().contains("assertion_pattern"), "got: {err}");
}

#[test]
fn a_declared_assertion_has_the_same_severity_in_either_spelling() {
    let cfg = Config {
        go: GoConfig {
            assertion_patterns: vec!["check*".into(), "testutil.*".into()],
            ..GoConfig::default()
        },
        ..Config::default()
    };

    let qualified =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { testutil.Check(t, 1) }\n";
    let plain = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { checkIt(t, 1) }\n";

    let severity = |src: &str| {
        Lang::Go
            .index(src, &cfg)
            .get("TestA")
            .unwrap()
            .assertions
            .first()
            .map(|a| a.severity)
    };
    assert_eq!(severity(qualified), severity(plain));

    assert!(
        run(Lang::Go, qualified, plain, &cfg).is_empty(),
        "swapping one spelling for the other is not a weakening"
    );
}

#[test]
fn a_pattern_declared_fatal_is_fatal_in_either_spelling() {
    let cfg = Config {
        go: GoConfig {
            assertion_patterns: vec!["must*".into(), "testutil.Must*".into()],
            fatal_patterns: vec!["must*".into(), "testutil.Must*".into()],
            ..GoConfig::default()
        },
        ..Config::default()
    };
    let qualified =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { testutil.MustEqual(t, 1) }\n";
    let plain = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { mustEqual(t, 1) }\n";

    for src in [qualified, plain] {
        let f = Lang::Go.index(src, &cfg);
        assert_eq!(f.get("TestA").unwrap().fatal_count(), 1, "for {src}");
    }
}

#[test]
fn a_disabled_rule_never_runs_rather_than_being_filtered_afterwards() {
    use misfire::rules::Options;
    use misfire::{Rule, RuleSet};

    let before = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1); assert.Equal(t,2,2) }\n";
    let after = "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let cfg = Config::default();

    let compare_with = |opts: &Options| {
        let mut out = Vec::new();
        rules::compare(
            "t",
            &Lang::Go.index(before, &cfg),
            &Lang::Go.index(after, &cfg),
            opts,
            &mut out,
        );
        out
    };

    assert_eq!(compare_with(&Options::default()).len(), 1);
    assert!(
        compare_with(&Options {
            rules: RuleSet::all().without(Rule::AssertionRemoved),
            ..Options::default()
        })
        .is_empty(),
        "the engine must honour the selection, not leave it to the caller"
    );
}

#[test]
fn config_produces_the_rule_set_the_engine_uses() {
    let cfg: Config = toml::from_str("[rules]\ndisabled = [\"MF104\", \"MF105\"]\n").unwrap();
    let set = cfg.rule_set();

    assert!(!set.contains(misfire::Rule::AssertionLoosened));
    assert!(!set.contains(misfire::Rule::SeverityDowngraded));
    assert!(set.contains(misfire::Rule::AssertionRemoved));
    assert!(
        !set.contains(misfire::Rule::TestFileDeleted),
        "the file-deleted rule stays out unless asked for"
    );

    let opted_in: Config = toml::from_str("[rules]\nfile_deleted = true\n").unwrap();
    assert!(opted_in.rule_set().contains(misfire::Rule::TestFileDeleted));
}

#[test]
fn a_rule_can_be_demoted_instead_of_switched_off() {
    use misfire::rules::Options;
    use misfire::{Confidence, Rule};

    let before =
        "package p\nimport \"testing\"\nfunc TestA(t *testing.T) { assert.Equal(t,1,1) }\n";
    let after = format!("{before}func TestB(t *testing.T) {{ Refund(1) }}\n");
    let cfg = Config::default();

    let findings = |opts: &Options| {
        let mut out = Vec::new();
        rules::compare(
            "t",
            &Lang::Go.index(before, &cfg),
            &Lang::Go.index(&after, &cfg),
            opts,
            &mut out,
        );
        out
    };

    let built_in = findings(&Options::default());
    assert_eq!(built_in[0].rule, Rule::TestWithoutAssertions);
    assert_eq!(built_in[0].confidence, Confidence::High);

    let demoted = findings(&Options {
        confidence: vec![(Rule::TestWithoutAssertions, Confidence::Medium)],
        ..Options::default()
    });
    assert_eq!(
        demoted.len(),
        1,
        "the finding is kept, only its stakes change"
    );
    assert_eq!(demoted[0].confidence, Confidence::Medium);
}

#[test]
fn confidence_overrides_come_from_the_config_file() {
    let cfg: Config =
        toml::from_str("[rules.confidence]\nMF103 = \"medium\"\nMF101 = \"low\"\n").unwrap();
    let overrides = cfg.confidence_overrides();

    assert!(overrides.contains(&(
        misfire::Rule::TestWithoutAssertions,
        misfire::Confidence::Medium
    )));
    assert!(overrides.contains(&(misfire::Rule::AssertionRemoved, misfire::Confidence::Low)));
}

#[test]
fn an_unknown_rule_in_a_confidence_override_is_rejected() {
    let cfg: Config = toml::from_str("[rules.confidence]\nMF999 = \"low\"\n").unwrap();
    assert!(
        cfg.validate().is_err(),
        "a typo must not silently do nothing"
    );
}
