use std::path::Path;
use std::process::{Command, Output};

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

fn misfire(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_misfire"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("misfire")
}

fn repo_with_a_weakening() -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    git(d, &["init", "-q", "-b", "main", "."]);
    git(d, &["config", "user.email", "t@t.t"]);
    git(d, &["config", "user.name", "t"]);
    std::fs::write(
        d.join("a_test.go"),
        "package p\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n\tassert.Equal(t, 2, Two())\n}\n",
    )
    .unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "base"]);
    std::fs::write(
        d.join("a_test.go"),
        "package p\n\nimport \"testing\"\n\nfunc TestA(t *testing.T) {\n\tassert.Equal(t, 1, One())\n}\n",
    )
    .unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-qm", "weaken"]);
    td
}

#[test]
fn findings_exit_one_and_a_clean_tree_exits_zero() {
    let td = repo_with_a_weakening();
    let found = misfire(td.path(), &["--base", "main~1"]);
    assert_eq!(found.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&found.stdout).contains("MF101"));

    let clean = misfire(td.path(), &["--base", "HEAD"]);
    assert_eq!(clean.status.code(), Some(0));
}

#[test]
fn a_failure_exits_two_so_it_is_distinguishable_from_findings() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "no-such-ref"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "could-not-run must not look like a clean run or a finding"
    );
}

#[test]
fn a_bad_ref_in_a_full_clone_does_not_get_a_lecture_about_ci() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "no-such-ref"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(
        !stderr.contains("fetch-depth"),
        "this clone is complete, so checkout advice would be wrong: {stderr}"
    );
}

#[test]
fn a_shallow_checkout_failure_tells_the_user_how_to_fix_it() {
    let origin = repo_with_a_weakening();
    let holder = tempfile::tempdir().unwrap();
    let clone = holder.path().join("shallow");
    let cloned = Command::new("git")
        .args([
            "clone",
            "-q",
            "--depth",
            "1",
            "--no-local",
            &format!("file://{}", origin.path().display()),
            clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    let out = misfire(&clone, &["--base", "no-such-ref"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("fetch-depth"),
        "a shallow checkout is exactly when the advice is right: {stderr}"
    );
}

#[test]
fn min_confidence_gates_the_exit_code_without_hiding_findings() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "main~1", "--min-confidence", "high"]);
    assert_eq!(out.status.code(), Some(1), "MF101 here is High");

    let json = misfire(td.path(), &["--base", "main~1", "--json"]);
    let body = String::from_utf8_lossy(&json.stdout);
    assert!(body.contains("\"rule\": \"MF101\""), "{body}");
}

#[test]
fn a_broken_config_fails_rather_than_being_ignored() {
    let td = repo_with_a_weakening();
    std::fs::write(td.path().join("misfire.toml"), "[go]\nnope = 1\n").unwrap();
    let out = misfire(td.path(), &["--base", "main~1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nope"));
}

#[test]
fn output_is_byte_identical_across_runs() {
    let td = repo_with_a_weakening();
    let a = misfire(td.path(), &["--base", "main~1", "--json"]).stdout;
    let b = misfire(td.path(), &["--base", "main~1", "--json"]).stdout;
    assert_eq!(a, b, "a gate that flaps gets removed");
}

#[test]
fn help_and_version_work() {
    let td = repo_with_a_weakening();
    assert!(misfire(td.path(), &["--help"]).status.success());
    let v = misfire(td.path(), &["--version"]);
    assert!(v.status.success());
    assert!(String::from_utf8_lossy(&v.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn conflicting_output_formats_are_rejected_rather_than_silently_ignored() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "main~1", "--json", "--sarif"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "one flag must not be dropped on the floor"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot be used with"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_default_run_says_nothing_on_stderr() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "main~1"]);
    assert!(
        out.stderr.is_empty(),
        "narration must be opt-in: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn narration_goes_to_stderr_so_stdout_stays_machine_readable() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "main~1", "--json", "--explain"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<serde_json::Value>(&stdout)
        .unwrap_or_else(|e| panic!("stdout must stay pure JSON: {e}\n{stdout}"));
    assert!(
        !out.stderr.is_empty(),
        "--explain should have narrated something"
    );
}

#[test]
fn explain_reports_the_decisions_that_produced_nothing() {
    let td = repo_with_a_weakening();
    let out = misfire(td.path(), &["--base", "main~1", "--explain"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("comparing") && stderr.contains("assertions_before"),
        "the counts a rule decided on must be visible: {stderr}"
    );
}

#[test]
fn verbosity_escalates_with_repeated_flags() {
    let td = repo_with_a_weakening();
    let quiet = misfire(td.path(), &["--base", "main~1"]).stderr.len();
    let some = misfire(td.path(), &["--base", "main~1", "-v"]).stderr.len();
    let more = misfire(td.path(), &["--base", "main~1", "-vv"])
        .stderr
        .len();
    assert!(quiet < some, "-v must say more than the default");
    assert!(some < more, "-vv must say more than -v");
}

#[test]
fn json_logs_are_one_object_per_line() {
    let td = repo_with_a_weakening();
    let out = misfire(
        td.path(),
        &["--base", "main~1", "--explain", "--log-format", "json"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("not a JSON object: {e}\n{line}"));
    }
}

#[test]
fn misfire_log_overrides_the_verbosity_flags() {
    let td = repo_with_a_weakening();
    let out = Command::new(env!("CARGO_BIN_EXE_misfire"))
        .current_dir(td.path())
        .args(["--base", "main~1"])
        .env("MISFIRE_LOG", "misfire=debug")
        .output()
        .unwrap();
    assert!(
        !out.stderr.is_empty(),
        "the env filter must work without any -v"
    );
}
