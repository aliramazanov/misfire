use misfire::analyze::analyze;
use misfire::config::Config;
use misfire::git::Repo;
use misfire::rules::Options;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn seed(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main", "."]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
}

fn commit(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", msg]);
}

fn findings(dir: &Path) -> Vec<misfire::Finding> {
    let repo = Repo::discover(dir).unwrap();
    let base = repo.merge_base("main~1").unwrap();
    analyze(&repo, &base, &Config::default(), &Options::default())
        .unwrap()
        .findings
}

const HELPERS: &str = "package pkg\n\nimport \"testing\"\n\n\
    func checkOmit(t *testing.T, found, unexpected string) {\n\
    \tif found == unexpected {\n\t\tt.Errorf(\"bad\")\n\t}\n}\n";

#[test]
fn a_helper_defined_in_a_sibling_file_counts_as_asserting() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "helpers_test.go", HELPERS);
    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestExisting(t *testing.T) {\n\tcheckOmit(t, a, b)\n}\n",
    );
    commit(d, "base");

    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestExisting(t *testing.T) {\n\tcheckOmit(t, a, b)\n}\n\n\
         func TestNew(t *testing.T) {\n\tcheckOmit(t, c, d)\n}\n",
    );
    commit(d, "add a test that asserts through the sibling helper");

    assert!(
        findings(d).is_empty(),
        "the helper lives next door, so the new test does assert"
    );
}

#[test]
fn a_new_test_that_calls_nothing_assertive_is_still_reported() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "helpers_test.go", HELPERS);
    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestExisting(t *testing.T) {\n\tcheckOmit(t, a, b)\n}\n",
    );
    commit(d, "base");

    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestExisting(t *testing.T) {\n\tcheckOmit(t, a, b)\n}\n\n\
         func TestNew(t *testing.T) {\n\tRefund(500)\n}\n",
    );
    commit(d, "add an empty test");

    let f = findings(d);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, misfire::Rule::TestWithoutAssertions);
}

#[test]
fn losing_assertions_that_lived_in_a_sibling_helper_is_caught() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "helpers_test.go", HELPERS);
    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {\n\tcheckOmit(t, a, b)\n\tcheckOmit(t, c, d)\n}\n",
    );
    commit(d, "base");

    write(
        d,
        "bash_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {\n\tcheckOmit(t, a, b)\n}\n",
    );
    commit(d, "drop one");

    let f = findings(d);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert_eq!(f[0].rule, misfire::Rule::AssertionRemoved);
}

#[test]
fn a_deleted_test_file_is_reported_only_beside_a_non_test_change() {
    use misfire::rules::Options;

    let build = |also_change_source: bool| {
        let td = tempfile::tempdir().unwrap();
        let d = td.path();
        seed(d);
        write(
            d,
            "pkg/a_test.go",
            "package pkg\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n}\n",
        );
        write(
            d,
            "pkg/a.go",
            "package pkg\n\nfunc One() int { return 1 }\n",
        );
        commit(d, "base");

        std::fs::remove_file(d.join("pkg/a_test.go")).unwrap();
        if also_change_source {
            write(
                d,
                "pkg/a.go",
                "package pkg\n\nfunc One() int { return 2 }\n",
            );
        }
        commit(d, "delete the test");
        td
    };

    let opts = Options {
        rules: misfire::RuleSet::all().with(misfire::Rule::TestFileDeleted),
        ..Options::default()
    };
    let run = |td: &tempfile::TempDir| {
        let repo = Repo::discover(td.path()).unwrap();
        let base = repo.merge_base("main~1").unwrap();
        analyze(&repo, &base, &Config::default(), &opts)
            .unwrap()
            .findings
    };

    let alone = build(false);
    assert!(
        run(&alone)
            .iter()
            .all(|f| f.rule != misfire::Rule::TestFileDeleted),
        "a test file deleted on its own is usually just obsolete"
    );

    let beside = build(true);
    assert!(
        run(&beside)
            .iter()
            .any(|f| f.rule == misfire::Rule::TestFileDeleted),
        "deleted beside a source change is worth surfacing"
    );
}

#[test]
fn vendoring_in_a_tagged_file_is_not_a_weakening() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(
        d,
        "a_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n}\n",
    );
    commit(d, "base");

    write(
        d,
        "vendored_test.go",
        "//go:build !js\n\npackage pkg\n\nimport \"testing\"\n\nfunc TestVendored(t *testing.T) {\n\tassert.Equal(t, 2, Two())\n}\n",
    );
    commit(d, "vendor a tagged file in");

    assert!(
        findings(d)
            .iter()
            .all(|f| f.rule != misfire::Rule::FileExcluded),
        "nothing stopped running: {:#?}",
        findings(d)
    );
}

#[test]
fn tagging_a_file_that_was_running_is_a_weakening() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(
        d,
        "a_test.go",
        "package pkg\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n}\n",
    );
    commit(d, "base");

    write(
        d,
        "a_test.go",
        "//go:build slow\n\npackage pkg\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n}\n",
    );
    commit(d, "hide it behind a tag");

    let f = findings(d);
    assert!(
        f.iter().any(|x| x.rule == misfire::Rule::FileExcluded),
        "{f:#?}"
    );
}
