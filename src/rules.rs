use crate::index::{TestFile, TestFn};
use crate::{Confidence, Finding, Rule, RuleSet};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub rules: RuleSet,

    pub confidence: Vec<(Rule, Confidence)>,
}

impl Options {
    #[must_use]
    pub fn all_rules() -> Self {
        Self {
            rules: RuleSet::all(),
            confidence: Vec::new(),
        }
    }

    fn level_for(&self, rule: Rule, built_in: Confidence) -> Confidence {
        self.confidence
            .iter()
            .find(|(r, _)| *r == rule)
            .map_or(built_in, |(_, level)| *level)
    }

    fn relevel(&self, findings: &mut [Finding]) {
        if self.confidence.is_empty() {
            return;
        }

        for f in findings {
            let adjusted = self.level_for(f.rule, f.confidence);
            if adjusted != f.confidence {
                tracing::debug!(
                    rule = f.rule.id(),
                    from = ?f.confidence,
                    to = ?adjusted,
                    "confidence overridden by misfire.toml"
                );
                f.confidence = adjusted;
            }
        }
    }
}

impl Default for RuleSet {
    fn default() -> Self {
        Self::all().without(Rule::TestFileDeleted)
    }
}

fn about(
    path: &str,
    test: &TestFn,
    line: usize,
    rule: Rule,
    confidence: Confidence,
    detail: String,
) -> Finding {
    Finding {
        file: path.to_string(),
        line,
        rule,
        test: Some(test.name.clone()),
        confidence,
        detail,
    }
}

pub fn compare(
    path: &str,
    before: &TestFile,
    after: &TestFile,
    opts: &Options,
    out: &mut Vec<Finding>,
) {
    if !before.parsed || !after.parsed {
        return;
    }

    if opts.rules.contains(Rule::SuiteNotRun) {
        suite_not_run(path, before, after, out);
    }

    let by_name_before = ByName::build(&before.tests);
    let by_name_after = ByName::build(&after.tests);

    let gone = removed_tests(&before.tests, &by_name_after);
    let arrived = unmatched(&after.tests, &by_name_before);

    let gained_tests = arrived
        .iter()
        .any(|t| t.trusted && after.effective_assertions(t) > 0);
    let renamed = match_renames(&gone, &arrived);

    let cmp = Comparison {
        path,
        before,
        after,
        opts,
        gained_tests,
    };

    for (old, new) in pair_by_occurrence(&before.tests, &by_name_after) {
        cmp.surviving(old, new, out);
    }

    for old in &gone {
        if renamed.contains(&old.name) {
            tracing::debug!(
                test = %old.name,
                "not reported: an arriving test has the same body, so this is a rename"
            );
            continue;
        }
        cmp.departed(old, out);
    }

    for new in &arrived {
        cmp.arrived(new, out);
    }
}

struct Comparison<'a> {
    path: &'a str,
    before: &'a TestFile,
    after: &'a TestFile,
    opts: &'a Options,

    gained_tests: bool,
}

impl Comparison<'_> {
    fn surviving(&self, old: &TestFn, new: &TestFn, out: &mut Vec<Finding>) {
        if !old.trusted || !new.trusted {
            tracing::debug!(test = %new.name, "skipped: the grammar could not read it");
            return;
        }

        tracing::trace!(
            test = %new.name,
            assertions_before = self.before.effective_assertions(old),
            assertions_after = self.after.effective_assertions(new),
            fatal_before = old.fatal_count(),
            fatal_after = new.fatal_count(),
            specific_before = old.specific_count(),
            specific_after = new.specific_count(),
            disabled_before = old.disabled.is_some(),
            disabled_after = new.disabled.is_some(),
            "comparing"
        );

        let mut local = Vec::new();
        let on = |rule| self.opts.rules.contains(rule);

        if on(Rule::AssertionRemoved) {
            assertions_removed(
                self.path,
                self.before,
                self.after,
                old,
                new,
                self.gained_tests,
                &mut local,
            );
        }

        if on(Rule::TestDisabled) {
            test_disabled(self.path, old, new, &mut local);
        }

        if on(Rule::AssertionLoosened) {
            assertion_loosened(self.path, old, new, &mut local);
        }

        if on(Rule::SeverityDowngraded) {
            severity_downgraded(self.path, old, new, &mut local);
        }

        if on(Rule::ExpectedFailureRemoved) {
            expected_failure_removed(self.path, old, new, &mut local);
        }

        dedupe_consequences(new, &mut local);

        keep_unsuppressed(
            &new.name,
            local,
            |rule| new.allows_rule(rule),
            self.opts,
            out,
        );
    }

    fn departed(&self, old: &TestFn, out: &mut Vec<Finding>) {
        if !old.trusted || !self.opts.rules.contains(Rule::TestRemoved) {
            return;
        }

        let mut local = Vec::new();
        test_removed(self.path, self.before, old, self.gained_tests, &mut local);

        keep_unsuppressed(
            &old.name,
            local,
            |rule| old.allows_rule(rule) || self.after.allows_rule(rule),
            self.opts,
            out,
        );
    }

    fn arrived(&self, new: &TestFn, out: &mut Vec<Finding>) {
        if !new.trusted || !self.opts.rules.contains(Rule::TestWithoutAssertions) {
            return;
        }

        let mut local = Vec::new();
        test_without_assertions(self.path, self.after, new, &mut local);
        keep_unsuppressed(
            &new.name,
            local,
            |rule| new.allows_rule(rule),
            self.opts,
            out,
        );
    }
}

fn keep_unsuppressed(
    test: &str,
    mut local: Vec<Finding>,
    allowed: impl Fn(&str) -> bool,
    opts: &Options,
    out: &mut Vec<Finding>,
) {
    opts.relevel(&mut local);
    if local.is_empty() {
        tracing::trace!(test, "no rule fired");
        return;
    }

    for f in local {
        if allowed(f.rule.id()) {
            tracing::debug!(
                test,
                rule = f.rule.id(),
                "suppressed by a misfire:allow comment"
            );
        } else {
            out.push(f);
        }
    }
}

fn suite_not_run(path: &str, before: &TestFile, after: &TestFile, out: &mut Vec<Finding>) {
    let Some(now) = &after.suite_runner else {
        return;
    };

    if now.runs {
        return;
    }

    let was_running = before.suite_runner.as_ref().is_none_or(|was| was.runs);

    if !was_running || after.allows_rule(Rule::SuiteNotRun.id()) {
        return;
    }

    out.push(Finding {
        file: path.to_string(),
        line: now.line,
        rule: Rule::SuiteNotRun,
        test: Some("TestMain".to_string()),
        confidence: Confidence::High,
        detail: format!(
            "TestMain never calls m.Run(), so none of the {} test{} in this package execute",
            after.tests.len(),
            plural(after.tests.len())
        ),
    });
}

pub fn file_excluded(
    path: &str,
    before: &TestFile,
    after: &TestFile,
    opts: &Options,
    out: &mut Vec<Finding>,
) {
    if !opts.rules.contains(Rule::FileExcluded) {
        return;
    }

    let Some(tag) = &after.build_constraint else {
        return;
    };

    if before.build_constraint.is_some() || after.tests.is_empty() {
        return;
    }

    if after.allows_rule(Rule::FileExcluded.id()) {
        return;
    }

    out.push(Finding {
        file: path.to_string(),
        line: 1,
        rule: Rule::FileExcluded,
        test: None,
        confidence: Confidence::High,
        detail: format!(
            "build constraint {tag:?} was added, so the {} test{} here no longer run by default",
            after.tests.len(),
            plural(after.tests.len())
        ),
    });
}

pub fn file_deleted(path: &str, before: &TestFile, opts: &Options, out: &mut Vec<Finding>) {
    if !opts.rules.contains(Rule::TestFileDeleted) || before.tests.is_empty() {
        return;
    }

    out.push(Finding {
        file: path.to_string(),
        line: 1,
        rule: Rule::TestFileDeleted,
        confidence: Confidence::Low,
        test: None,
        detail: format!(
            "test file deleted, removing {} test{}",
            before.tests.len(),
            plural(before.tests.len())
        ),
    });
}

fn assertions_removed(
    path: &str,
    before: &TestFile,
    after: &TestFile,
    old: &TestFn,
    new: &TestFn,
    gained_tests: bool,
    out: &mut Vec<Finding>,
) {
    let lost = before
        .effective_assertions(old)
        .saturating_sub(after.effective_assertions(new));

    if lost == 0 {
        return;
    }

    let verifying_lost = old.verifying_count().saturating_sub(new.verifying_count());
    let guards_only = verifying_lost == 0;

    tracing::debug!(
        test = %new.name,
        lost,
        verifying_lost,
        guards_only,
        "assertions removed"
    );

    let extracted = new
        .helper_calls
        .iter()
        .any(|c| !old.helper_calls.contains(c));
    let doubt = extracted || gained_tests || guards_only;

    let confidence = if doubt {
        Confidence::Medium
    } else {
        Confidence::High
    };

    out.push(about(
        path,
        new,
        new.line,
        Rule::AssertionRemoved,
        confidence,
        format!(
            "{lost} assertion{} removed from {}{}",
            plural(lost),
            new.name,
            if guards_only {
                ", though only setup guards were lost"
            } else if extracted {
                ", though a new helper call appeared"
            } else if gained_tests {
                ", though the file gained a test"
            } else {
                ""
            }
        ),
    ));
}

fn test_disabled(path: &str, old: &TestFn, new: &TestFn, out: &mut Vec<Finding>) {
    if old.disabled.is_some() {
        return;
    }

    let Some(d) = &new.disabled else { return };

    let subject = match &new.disabled_subtest {
        Some(subtest) => format!("subtest {subtest:?} of {}", new.name),
        None => new.name.clone(),
    };

    let detail = describe_disabling(&subject, &d.marker);

    out.push(about(
        path,
        new,
        d.line,
        Rule::TestDisabled,
        Confidence::High,
        detail,
    ));
}

fn describe_disabling(subject: &str, marker: &str) -> String {
    if let Some(cfg) = marker.strip_prefix("#[cfg(") {
        let condition = cfg.trim_end_matches(")]").trim_end_matches(')');
        return format!("{subject} is now gated behind {condition}, so it does not run by default");
    }

    if marker.starts_with('#') {
        return format!("{subject} now carries {marker}");
    }

    format!("{subject} now calls {marker}")
}

fn test_removed(
    path: &str,
    before: &TestFile,
    old: &TestFn,
    gained_tests: bool,
    out: &mut Vec<Finding>,
) {
    if old.disabled.is_some() {
        return;
    }

    let checks = before.effective_assertions(old);

    if checks == 0 && old.expected_failure.is_none() {
        return;
    }

    let confidence = if gained_tests {
        Confidence::Medium
    } else {
        Confidence::High
    };

    out.push(about(
        path,
        old,
        old.line,
        Rule::TestRemoved,
        confidence,
        format!(
            "{} no longer runs, taking {} assertion{} with it{}",
            old.name,
            checks,
            plural(checks),
            if gained_tests {
                ", though the file gained a test and it may be a rename"
            } else {
                ""
            }
        ),
    ));
}

fn test_without_assertions(path: &str, after: &TestFile, new: &TestFn, out: &mut Vec<Finding>) {
    if !new.delegating_calls.is_empty() {
        tracing::debug!(
            test = %new.name,
            delegates_to = ?new.delegating_calls,
            "not reported: the test hands its testing handle to a helper"
        );
    }

    if after.effective_assertions(new) > 0
        || new.disabled.is_some()
        || new.expected_failure.is_some()
    {
        return;
    }

    if !new.delegating_calls.is_empty() {
        return;
    }

    out.push(about(
        path,
        new,
        new.line,
        Rule::TestWithoutAssertions,
        Confidence::High,
        format!("{} added with 0 assertions", new.name),
    ));
}

fn assertion_loosened(path: &str, old: &TestFn, new: &TestFn, out: &mut Vec<Finding>) {
    if new.assertion_count() != old.assertion_count() {
        return;
    }

    let lost = old.specific_count().saturating_sub(new.specific_count());

    if lost == 0 {
        return;
    }

    out.push(about(
        path,
        new,
        new.line,
        Rule::AssertionLoosened,
        Confidence::Medium,
        format!(
            "{lost} assertion{} in {} replaced with a broader check",
            plural(lost),
            new.name
        ),
    ));
}

fn severity_downgraded(path: &str, old: &TestFn, new: &TestFn, out: &mut Vec<Finding>) {
    if new.assertion_count() != old.assertion_count() {
        return;
    }

    let lost = old.fatal_count().saturating_sub(new.fatal_count());

    if lost == 0 {
        return;
    }

    out.push(about(
        path,
        new,
        new.line,
        Rule::SeverityDowngraded,
        Confidence::High,
        format!(
            "{lost} assertion{} in {} no longer {} the test on failure, so what follows runs on bad state",
            plural(lost),
            new.name,
            if lost == 1 { "stops" } else { "stop" }
        ),
    ));
}

fn expected_failure_removed(path: &str, old: &TestFn, new: &TestFn, out: &mut Vec<Finding>) {
    let Some(before) = &old.expected_failure else {
        return;
    };

    let Some(after) = &new.expected_failure else {
        out.push(about(
            path,
            new,
            new.line,
            Rule::ExpectedFailureRemoved,
            Confidence::High,
            format!("{} no longer checks that the failure happens", new.name),
        ));
        return;
    };

    let (Some(was), now) = (before.expected.as_deref(), after.expected.as_deref()) else {
        return;
    };

    let Some(now) = now else {
        out.push(about(
            path,
            new,
            after.line,
            Rule::ExpectedFailureRemoved,
            Confidence::High,
            format!(
                "{} dropped the expected message, so it now passes on any failure",
                new.name
            ),
        ));
        return;
    };

    if now.len() < was.len() && was.contains(now) {
        out.push(about(
            path,
            new,
            after.line,
            Rule::ExpectedFailureRemoved,
            Confidence::High,
            format!(
                "{} narrowed its expected message to {now:?}, which matches more failures than {was:?}",
                new.name
            ),
        ));
    }
}

struct ByName<'a> {
    slots: HashMap<&'a str, Vec<&'a TestFn>>,
}

impl<'a> ByName<'a> {
    fn build(list: &'a [TestFn]) -> Self {
        let mut slots: HashMap<&str, Vec<&TestFn>> = HashMap::with_capacity(list.len());
        for t in list {
            slots.entry(t.name.as_str()).or_default().push(t);
        }
        Self { slots }
    }

    fn nth(&self, name: &str, n: usize) -> Option<&'a TestFn> {
        self.slots.get(name).and_then(|v| v.get(n)).copied()
    }
}

fn pair_by_occurrence<'a>(
    before: &'a [TestFn],
    after: &'a ByName<'a>,
) -> Vec<(&'a TestFn, &'a TestFn)> {
    let mut pairs = Vec::with_capacity(before.len());
    let mut seen: HashMap<&str, usize> = HashMap::new();

    for old in before {
        let n = seen.entry(old.name.as_str()).or_insert(0);
        if let Some(new) = after.nth(&old.name, *n) {
            pairs.push((old, new));
        }
        *n += 1;
    }

    pairs
}

fn match_renames(gone: &[&TestFn], arrived: &[&TestFn]) -> HashSet<String> {
    let mut available: HashMap<Vec<String>, usize> = HashMap::new();

    for t in arrived {
        let sig = signature(t);
        if sig.len() >= MIN_RENAME_SIGNATURE {
            *available.entry(sig).or_insert(0) += 1;
        }
    }

    let mut renamed = HashSet::new();

    for old in gone {
        let sig = signature(old);
        if sig.len() < MIN_RENAME_SIGNATURE {
            continue;
        }

        if let Some(remaining) = available.get_mut(&sig)
            && *remaining > 0
        {
            *remaining -= 1;
            renamed.insert(old.name.clone());
        }
    }

    renamed
}

const MIN_RENAME_SIGNATURE: usize = 2;

fn signature(t: &TestFn) -> Vec<String> {
    let mut sig: Vec<String> = t.assertions.iter().map(|a| a.call.clone()).collect();
    sig.extend(t.delegating_calls.iter().cloned());
    sig.extend(t.helper_calls.iter().cloned());
    sig.sort();
    sig
}

fn removed_tests<'a>(before: &'a [TestFn], after: &ByName<'a>) -> Vec<&'a TestFn> {
    unmatched(before, after)
}

fn unmatched<'a>(list: &'a [TestFn], other: &ByName<'_>) -> Vec<&'a TestFn> {
    let mut missing = Vec::new();
    let mut seen: HashMap<&str, usize> = HashMap::new();

    for t in list {
        let n = seen.entry(t.name.as_str()).or_insert(0);
        if other.nth(&t.name, *n).is_none() {
            missing.push(t);
        }
        *n += 1;
    }
    missing
}

fn dedupe_consequences(new: &TestFn, local: &mut Vec<Finding>) {
    let wholesale =
        new.assertion_count() == 0 && local.iter().any(|f| f.rule == Rule::AssertionRemoved);

    if wholesale {
        local.retain(|f| {
            f.rule != Rule::ExpectedFailureRemoved || f.detail.contains("expected message")
        });
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
