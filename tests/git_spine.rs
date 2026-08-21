use misfire::git::{ChangeKind, Repo};
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
        "git {:?} failed: {}",
        args,
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

fn find<'a>(files: &'a [misfire::git::ChangedFile], path: &str) -> &'a misfire::git::ChangedFile {
    files.iter().find(|f| f.path == path).unwrap_or_else(|| {
        panic!(
            "{path} not in {:?}",
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        )
    })
}

fn go_suite(fn_prefix: &str, asserter: &str, callee: &str) -> String {
    use std::fmt::Write;
    let mut out = String::from("package pkg\n\nimport (\n\t\"testing\"\n)\n\n");
    for i in 1..=12 {
        let _ = write!(
            out,
            "func Test{fn_prefix}{i}(t *testing.T) {{\n\tgot := {callee}({i})\n\t{asserter}(t, {}, got)\n}}\n\n",
            i * 2
        );
    }
    out
}

#[test]
fn detects_rename_that_was_also_heavily_edited() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(
        d,
        "pkg/user_test.go",
        &go_suite("Case", "require.Equal", "Compute"),
    );
    commit(d, "base");

    git(d, &["mv", "pkg/user_test.go", "pkg/account_test.go"]);
    write(
        d,
        "pkg/account_test.go",
        &go_suite("Case", "assert.Equal", "Calculate"),
    );
    commit(d, "rename and rework");

    let repo = Repo::discover(d).unwrap();
    let base = repo.merge_base("main~1").unwrap();
    let files = repo.changed_files(&base).unwrap();

    let f = find(&files, "pkg/account_test.go");
    assert_eq!(
        f.kind,
        ChangeKind::Renamed {
            from: "pkg/user_test.go".into()
        },
        "a renamed-and-reworked test file must not read as delete+add"
    );
    assert!(
        !files.iter().any(|f| f.kind == ChangeKind::Deleted),
        "the rename must not also surface as a deletion"
    );
}

#[test]
fn resolves_repo_root_from_a_subdirectory() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "a/b/c/x_test.go", "package c\n");
    commit(d, "base");
    write(d, "a/b/c/x_test.go", "package c\n// edit\n");
    commit(d, "edit");

    let repo = Repo::discover(&d.join("a/b/c")).unwrap();
    let base = repo.merge_base("main~1").unwrap();
    let files = repo.changed_files(&base).unwrap();

    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].path, "a/b/c/x_test.go",
        "paths must be repo-root relative regardless of cwd"
    );
}

#[test]
fn handles_pure_deletion_and_paths_with_spaces() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(
        d,
        "gone_test.go",
        "package p\n\nimport \"testing\"\n\nfunc TestGone(t *testing.T) {\n\tif Compute(1) != 2 {\n\t\tt.Fatal(\"bad\")\n\t}\n}\n",
    );
    commit(d, "base");
    std::fs::remove_file(d.join("gone_test.go")).unwrap();
    write(
        d,
        "my tests/a b_test.go",
        "package q\n\nimport \"net/http\"\n\nfunc Serve(w http.ResponseWriter) {\n\tw.WriteHeader(204)\n}\n",
    );
    commit(d, "delete and add");

    let repo = Repo::discover(d).unwrap();
    let base = repo.merge_base("main~1").unwrap();
    let files = repo.changed_files(&base).unwrap();

    let gone = find(&files, "gone_test.go");
    assert_eq!(gone.kind, ChangeKind::Deleted);
    assert_eq!(gone.new_path(), None);
    find(&files, "my tests/a b_test.go");
}

#[test]
fn reads_both_sides_of_a_modified_file() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "x_test.go", "before\n");
    commit(d, "base");
    write(d, "x_test.go", "after\n");
    commit(d, "edit");

    let repo = Repo::discover(d).unwrap();
    let base = repo.merge_base("main~1").unwrap();
    let old = repo.blob(&base, "x_test.go").unwrap().unwrap();
    let new = repo.blob("HEAD", "x_test.go").unwrap().unwrap();

    assert_eq!(String::from_utf8_lossy(&old), "before\n");
    assert_eq!(String::from_utf8_lossy(&new), "after\n");
}

#[test]
fn missing_blob_is_none_not_an_error() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "x_test.go", "x\n");
    commit(d, "base");

    let repo = Repo::discover(d).unwrap();
    assert!(repo.blob("HEAD", "never_existed.go").unwrap().is_none());
}

#[test]
fn unreachable_base_fails_loudly_rather_than_reporting_nothing() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    seed(d);
    write(d, "x_test.go", "x\n");
    commit(d, "base");

    let repo = Repo::discover(d).unwrap();
    let err = repo.merge_base("no-such-ref").unwrap_err();

    assert!(
        matches!(err, misfire::error::Error::BaseNotFound { .. }),
        "callers must match on the kind, not parse a string: {err:?}"
    );
    assert!(
        !err.is_shallow_checkout_problem(),
        "this repository is not shallow, so checkout advice would be misleading"
    );
}

#[test]
fn repairs_a_shallow_clone_whose_base_was_never_fetched() {
    let origin_td = tempfile::tempdir().unwrap();
    let origin = origin_td.path();
    seed(origin);
    write(origin, "x_test.go", "package p\nfunc TestA(){}\n");
    commit(origin, "base");
    git(origin, &["checkout", "-q", "-b", "feature"]);
    write(
        origin,
        "x_test.go",
        "package p\nfunc TestA(){}\n// changed\n",
    );
    commit(origin, "feature work");

    let clone_td = tempfile::tempdir().unwrap();
    let clone = clone_td.path().join("c");
    let out = Command::new("git")
        .args([
            "clone",
            "-q",
            "--depth",
            "1",
            &format!("file://{}", origin.display()),
            clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let repo = Repo::discover(&clone).unwrap();
    let base = repo
        .merge_base("main")
        .expect("misfire must deepen a shallow clone instead of giving up");
    let files = repo.changed_files(&base).unwrap();
    find(&files, "x_test.go");
}

#[test]
fn discovering_outside_a_repository_is_a_typed_error() {
    let td = tempfile::tempdir().unwrap();
    let err = Repo::discover(td.path()).unwrap_err();
    assert!(matches!(err, misfire::error::Error::NotARepository));
}
