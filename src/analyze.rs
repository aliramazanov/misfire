use crate::error::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::Finding;
use crate::config::Config;
use crate::git::{ChangeKind, Repo};
use crate::index::{TestFile, TestFn};
use crate::lang::Lang;
use crate::observe::{Metrics, Phase};
use crate::rules::{self, Options};

#[derive(Debug, Default)]
pub struct Outcome {
    pub findings: Vec<Finding>,

    pub metrics: Metrics,

    pub unanalysed: Vec<String>,
}

pub fn analyze(repo: &Repo, base: &str, cfg: &Config, opts: &Options) -> Result<Outcome> {
    let mut metrics = Metrics::default();

    let phase = Phase::start("read-diff");
    let changed = repo.changed_files(base)?;

    metrics.record_phase("read-diff", phase.elapsed());

    phase.done(&format!("{} paths changed", changed.len()));

    metrics.paths_changed = changed.len() as u64;

    let dirs_with_other_changes: HashSet<&str> = changed
        .iter()
        .filter(|f| Lang::for_path(&f.path).is_none())
        .filter_map(|f| parent_of(&f.path))
        .collect();

    let mut findings = Vec::new();
    let mut unanalysed = Vec::new();
    let mut helper_cache: HashMap<(String, String), Vec<TestFn>> = HashMap::new();

    for file in &changed {
        let Some(lang) = Lang::for_path(&file.path) else {
            continue;
        };

        let _file_span = tracing::debug_span!("file", path = %file.path).entered();

        metrics.files_analysed += 1;

        examine(
            repo,
            base,
            file,
            lang,
            cfg,
            opts,
            &dirs_with_other_changes,
            &mut helper_cache,
            &mut metrics,
            &mut findings,
            &mut unanalysed,
        )?;
    }

    unanalysed.sort();
    unanalysed.dedup();

    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.rule.id().cmp(b.rule.id()))
            .then(a.detail.cmp(&b.detail))
    });

    for f in &findings {
        metrics.record_finding(f.rule.id());
    }

    Ok(Outcome {
        findings,
        metrics,
        unanalysed,
    })
}

#[allow(clippy::too_many_arguments)]
fn examine(
    repo: &Repo,
    base: &str,
    file: &crate::git::ChangedFile,
    lang: Lang,
    cfg: &Config,
    opts: &Options,
    dirs_with_other_changes: &HashSet<&str>,
    helper_cache: &mut HashMap<(String, String), Vec<TestFn>>,
    metrics: &mut Metrics,
    findings: &mut Vec<Finding>,
    unanalysed: &mut Vec<String>,
) -> Result<()> {
    let phase = Phase::start("index");

    let mut before = match file.old_path() {
        Some(p) => load(repo, base, p, lang, cfg, metrics)?,
        None => TestFile::default(),
    };

    let mut after = match file.new_path() {
        Some(p) => load(repo, "HEAD", p, lang, cfg, metrics)?,
        None => TestFile::default(),
    };

    metrics.record_phase("index", phase.elapsed());
    phase.done(&format!("{}: {} tests", file.path, after.tests.len()));

    let phase = Phase::start("resolve-helpers");

    if let Some(p) = file.old_path() {
        borrow_siblings(repo, base, p, lang, cfg, helper_cache, metrics, &mut before)?;
    }

    if let Some(p) = file.new_path() {
        borrow_siblings(
            repo,
            "HEAD",
            p,
            lang,
            cfg,
            helper_cache,
            metrics,
            &mut after,
        )?;
    }

    metrics.record_phase("resolve-helpers", phase.elapsed());
    phase.done("done");

    metrics.tests_indexed += after.tests.len() as u64;
    if !before.parsed || !after.parsed {
        metrics.files_unreadable += 1;
        unanalysed.push(file.path.clone());
    }

    metrics.tests_unreadable += after.untrusted().count() as u64;
    for t in after.untrusted() {
        unanalysed.push(format!("{}:{} ({})", file.path, t.line, t.name));
    }

    if file.kind == ChangeKind::Deleted {
        let siblings_changed =
            parent_of(&file.path).is_some_and(|d| dirs_with_other_changes.contains(d));
        if siblings_changed {
            rules::file_deleted(&file.path, &before, opts, findings);
        }

        return Ok(());
    }

    let phase = Phase::start("compare");
    let was = findings.len();

    if file.kind != ChangeKind::Added {
        rules::file_excluded(&file.path, &before, &after, opts, findings);
    }

    rules::compare(&file.path, &before, &after, opts, findings);
    metrics.record_phase("compare", phase.elapsed());

    phase.done(&format!(
        "{}: {} tests, {} findings",
        file.path,
        after.tests.len(),
        findings.len() - was
    ));

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn borrow_siblings(
    repo: &Repo,
    rev: &str,
    path: &str,
    lang: Lang,
    cfg: &Config,
    cache: &mut HashMap<(String, String), Vec<TestFn>>,
    metrics: &mut Metrics,
    file: &mut TestFile,
) -> Result<()> {
    if !has_unresolved_calls(file) {
        return Ok(());
    }

    let dir = parent_of(path).unwrap_or("").to_string();
    let key = (rev.to_string(), dir.clone());

    if !cache.contains_key(&key) {
        metrics.helper_dirs_loaded += 1;

        let mut helpers = Vec::new();

        for sibling in repo.list_dir(rev, &dir)? {
            if sibling == path || Lang::for_path(&sibling) != Some(lang) {
                continue;
            }
            let indexed = load(repo, rev, &sibling, lang, cfg, metrics)?;
            helpers.extend(indexed.helpers);
            helpers.extend(indexed.tests);
        }

        cache.insert(key.clone(), helpers);
    }

    let known: HashSet<&str> = file.helpers.iter().map(|h| h.name.as_str()).collect();

    let extra: Vec<TestFn> = cache[&key]
        .iter()
        .filter(|h| !known.contains(h.name.as_str()))
        .cloned()
        .collect();
    file.helpers.extend(extra);

    Ok(())
}

fn has_unresolved_calls(file: &TestFile) -> bool {
    let known: HashSet<&str> = file.helpers.iter().map(|h| h.name.as_str()).collect();
    file.tests.iter().any(|t| {
        t.helper_calls
            .iter()
            .chain(t.delegating_calls.iter())
            .any(|c| !known.contains(c.as_str()))
    })
}

fn load(
    repo: &Repo,
    rev: &str,
    path: &str,
    lang: Lang,
    cfg: &Config,
    metrics: &mut Metrics,
) -> Result<TestFile> {
    metrics.blobs_read += 1;

    let Some(bytes) = repo.blob(rev, path)? else {
        return Ok(TestFile::default());
    };

    let Ok(src) = String::from_utf8(bytes) else {
        return Ok(TestFile {
            parsed: false,
            ..TestFile::default()
        });
    };

    Ok(lang.index(&src, cfg))
}

fn parent_of(path: &str) -> Option<&str> {
    Path::new(path).parent().and_then(|p| p.to_str())
}
