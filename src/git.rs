use crate::error::{Error, Result};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const RENAME_DETECTION: &str = "--find-renames=40%";

const RENAME_LIMIT: &str = "-l50000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,

    Modified,

    Deleted,

    Renamed { from: String },
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,

    pub kind: ChangeKind,
}

impl ChangedFile {
    #[must_use]
    pub fn old_path(&self) -> Option<&str> {
        match &self.kind {
            ChangeKind::Added => None,
            ChangeKind::Renamed { from } => Some(from),
            _ => Some(&self.path),
        }
    }

    #[must_use]
    pub fn new_path(&self) -> Option<&str> {
        match self.kind {
            ChangeKind::Deleted => None,
            _ => Some(&self.path),
        }
    }
}

#[derive(Debug)]
pub struct Repo {
    root: PathBuf,

    blobs: RefCell<Option<BlobReader>>,
}

#[derive(Debug)]
struct BlobReader {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for BlobReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl BlobReader {
    fn spawn(root: &Path) -> Option<Self> {
        let mut child = Command::new("git")
            .current_dir(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let stdin = child.stdin.take()?;

        let stdout = BufReader::new(child.stdout.take()?);

        Some(Self {
            child,
            stdin,
            stdout,
        })
    }

    fn read(&mut self, spec: &str) -> std::result::Result<Option<Vec<u8>>, ()> {
        writeln!(self.stdin, "{spec}").map_err(|_| ())?;
        self.stdin.flush().map_err(|_| ())?;

        let mut header = String::new();
        if self.stdout.read_line(&mut header).map_err(|_| ())? == 0 {
            return Err(());
        }

        let header = header.trim_end();
        if header.ends_with("missing") || header.ends_with("ambiguous") {
            return Ok(None);
        }

        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or(())?;

        let mut body = vec![0u8; size + 1];

        std::io::Read::read_exact(&mut self.stdout, &mut body).map_err(|_| ())?;

        body.truncate(size);

        Ok(Some(body))
    }
}

impl Repo {
    pub fn discover(from: &Path) -> Result<Self> {
        let out =
            run(from, &["rev-parse", "--show-toplevel"]).map_err(|_| Error::NotARepository)?;

        Ok(Self {
            root: PathBuf::from(out.trim()),
            blobs: RefCell::new(None),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        run(&self.root, args)
    }

    fn is_shallow(&self) -> bool {
        self.git(&["rev-parse", "--is-shallow-repository"])
            .map(|s| s.trim() == "true")
            .unwrap_or(false)
    }

    fn resolve_rev(&self, base: &str) -> Option<String> {
        let mut candidates = vec![base.to_string()];

        if !base.contains('/') {
            candidates.push(format!("origin/{base}"));
        }

        for cand in candidates {
            if let Ok(sha) = self.git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{cand}^{{commit}}"),
            ]) && !sha.trim().is_empty()
            {
                return Some(sha.trim().to_string());
            }
        }

        None
    }

    pub fn merge_base(&self, base: &str) -> Result<String> {
        let sha = if let Some(sha) = self.resolve_rev(base) {
            sha
        } else {
            self.try_fetch(base);
            self.resolve_rev(base).ok_or_else(|| Error::BaseNotFound {
                base: base.to_string(),
                shallow: self.is_shallow(),
            })?
        };

        if let Ok(mb) = self.git(&["merge-base", &sha, "HEAD"]) {
            return Ok(mb.trim().to_string());
        }

        if self.is_shallow() {
            self.deepen();
            if let Ok(mb) = self.git(&["merge-base", &sha, "HEAD"]) {
                return Ok(mb.trim().to_string());
            }
        }

        Err(Error::NoMergeBase {
            base: base.to_string(),
            shallow: self.is_shallow(),
        })
    }

    fn try_fetch(&self, base: &str) {
        let branch = branch_of(base);
        let remote = remote_of(base);
        let refspec = format!("{branch}:refs/remotes/{remote}/{branch}");

        if self
            .git(&["fetch", "--no-tags", "--quiet", remote, &refspec])
            .is_ok()
        {
            return;
        }

        let _ = self.git(&["fetch", "--no-tags", "--quiet", remote, branch]);
    }

    fn deepen(&self) {
        for depth in ["50", "500"] {
            if self
                .git(&[
                    "fetch",
                    "--no-tags",
                    "--quiet",
                    &format!("--deepen={depth}"),
                ])
                .is_err()
            {
                break;
            }

            if !self.is_shallow() {
                return;
            }
        }

        let _ = self.git(&["fetch", "--no-tags", "--quiet", "--unshallow"]);
    }

    pub fn changed_files(&self, from: &str) -> Result<Vec<ChangedFile>> {
        let raw = self.git(&[
            "diff",
            "--raw",
            RENAME_DETECTION,
            RENAME_LIMIT,
            "-z",
            from,
            "HEAD",
        ])?;
        parse_raw(&raw)
    }

    pub fn list_dir(&self, rev: &str, dir: &str) -> Result<Vec<String>> {
        let spec = if dir.is_empty() {
            rev.to_string()
        } else {
            format!("{rev}:{dir}")
        };

        let Ok(raw) = self.git(&["ls-tree", "--name-only", "-z", &spec]) else {
            return Ok(Vec::new());
        };

        Ok(raw
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(|name| {
                if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}/{name}")
                }
            })
            .collect())
    }

    pub fn blob(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{rev}:{path}");

        if batchable(&spec) {
            let mut slot = self.blobs.borrow_mut();

            if slot.is_none() {
                *slot = BlobReader::spawn(&self.root);
            }

            if let Some(reader) = slot.as_mut() {
                match reader.read(&spec) {
                    Ok(found) => return Ok(found),

                    Err(()) => *slot = None,
                }
            }
        }

        let out = Command::new("git")
            .current_dir(&self.root)
            .args(["show", &spec])
            .output()
            .map_err(Error::GitUnavailable)?;

        if !out.status.success() {
            return Ok(None);
        }

        Ok(Some(out.stdout))
    }
}

fn batchable(spec: &str) -> bool {
    !spec.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0)
}

fn branch_of(base: &str) -> &str {
    base.split_once('/').map_or(base, |(_, b)| b)
}

fn remote_of(base: &str) -> &str {
    match base.split_once('/') {
        Some((r, _)) => r,
        None => "origin",
    }
}

fn parse_raw(raw: &str) -> Result<Vec<ChangedFile>> {
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());
    let mut files = Vec::new();

    while let Some(meta) = fields.next() {
        let Some(rest) = meta.strip_prefix(':') else {
            continue;
        };

        let parts: Vec<&str> = rest.split_whitespace().collect();
        let [old_mode, new_mode, _, _, status] = parts[..] else {
            continue;
        };

        let code = status.as_bytes()[0];
        let blobby = |m: &str| m == "000000" || m.starts_with("100");
        let usable = blobby(old_mode) && blobby(new_mode);

        if code == b'R' || code == b'C' {
            let from = fields
                .next()
                .ok_or(Error::MalformedGitOutput("truncated rename entry"))?;
            let to = fields
                .next()
                .ok_or(Error::MalformedGitOutput("truncated rename entry"))?;
            if usable {
                files.push(ChangedFile {
                    path: to.to_string(),
                    kind: ChangeKind::Renamed {
                        from: from.to_string(),
                    },
                });
            }
            continue;
        }

        let path = fields
            .next()
            .ok_or(Error::MalformedGitOutput("truncated path entry"))?;

        if !usable {
            continue;
        }

        let kind = match code {
            b'A' => ChangeKind::Added,
            b'D' => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        };

        files.push(ChangedFile {
            path: path.to_string(),
            kind,
        });
    }

    Ok(files)
}

fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(Error::GitUnavailable)?;

    if !out.status.success() {
        return Err(Error::GitFailed {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(entries: &[(&str, &str, &str)]) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for (mode, status, paths) in entries {
            let _ = write!(out, ":{mode} {mode} aaa bbb {status}\0{paths}\0");
        }
        out
    }

    #[test]
    fn parses_plain_statuses() {
        let files = parse_raw(&raw(&[
            ("100644", "M", "a.go"),
            ("100644", "A", "b.go"),
            ("100644", "D", "c.go"),
        ]))
        .unwrap();

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].kind, ChangeKind::Modified);
        assert_eq!(files[1].kind, ChangeKind::Added);
        assert_eq!(files[2].kind, ChangeKind::Deleted);
        assert_eq!(files[2].path, "c.go");
    }

    #[test]
    fn parses_rename_with_three_fields() {
        let files = parse_raw(&raw(&[("100644", "R096", "old_test.go\0new_test.go")])).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_test.go");
        assert_eq!(
            files[0].kind,
            ChangeKind::Renamed {
                from: "old_test.go".into()
            }
        );
    }

    #[test]
    fn rename_reports_both_sides() {
        let files = parse_raw(&raw(&[("100644", "R100", "a_test.go\0b_test.go")])).unwrap();
        assert_eq!(files[0].old_path(), Some("a_test.go"));
        assert_eq!(files[0].new_path(), Some("b_test.go"));
    }

    #[test]
    fn deleted_file_has_no_new_side() {
        let files = parse_raw(&raw(&[("100644", "D", "gone_test.go")])).unwrap();
        assert_eq!(files[0].new_path(), None);
        assert_eq!(files[0].old_path(), Some("gone_test.go"));
    }

    #[test]
    fn added_file_has_no_old_side() {
        let files = parse_raw(&raw(&[("100644", "A", "new_test.go")])).unwrap();
        assert_eq!(files[0].old_path(), None);
    }

    #[test]
    fn paths_with_spaces_survive_nul_separation() {
        let files = parse_raw(&raw(&[("100644", "M", "my tests/a b_test.go")])).unwrap();
        assert_eq!(files[0].path, "my tests/a b_test.go");
    }

    #[test]
    fn empty_diff_yields_nothing() {
        assert!(parse_raw("").unwrap().is_empty());
    }

    #[test]
    fn symlinks_are_not_source_files() {
        let files = parse_raw(&raw(&[("120000", "A", "link_test.go")])).unwrap();
        assert!(files.is_empty(), "a symlink blob holds a path, not code");
    }

    #[test]
    fn submodule_pointers_are_skipped() {
        let files = parse_raw(&raw(&[("160000", "M", "vendor/thing")])).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn executable_sources_are_still_analysed() {
        let files = parse_raw(&raw(&[("100755", "M", "run_test.go")])).unwrap();
        assert_eq!(files.len(), 1);
    }
}
