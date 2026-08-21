pub mod analyze;

pub mod baseline;

pub mod config;
pub mod error;

pub mod git;
pub mod index;

pub mod lang;

pub mod observe;

pub mod report;

pub mod rules;

pub mod sarif;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Rule {
    AssertionRemoved,

    TestDisabled,

    TestWithoutAssertions,

    AssertionLoosened,

    SeverityDowngraded,

    ExpectedFailureRemoved,

    TestFileDeleted,

    TestRemoved,

    SuiteNotRun,

    FileExcluded,
}

impl Rule {
    pub const ALL: &'static [Self] = &[
        Self::AssertionRemoved,
        Self::TestDisabled,
        Self::TestWithoutAssertions,
        Self::AssertionLoosened,
        Self::SeverityDowngraded,
        Self::ExpectedFailureRemoved,
        Self::TestFileDeleted,
        Self::TestRemoved,
        Self::SuiteNotRun,
        Self::FileExcluded,
    ];

    const fn facts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::AssertionRemoved => (
                "MF101",
                "assertion-removed",
                "A test survived the change with fewer assertions than before",
            ),
            Self::TestDisabled => (
                "MF102",
                "test-disabled",
                "A test gained a skip or ignore marker",
            ),
            Self::TestWithoutAssertions => (
                "MF103",
                "test-without-assertions",
                "A new test was added with no assertions",
            ),
            Self::AssertionLoosened => (
                "MF104",
                "assertion-loosened",
                "An assertion was replaced with a broader check",
            ),
            Self::SeverityDowngraded => (
                "MF105",
                "severity-downgraded",
                "A failing assertion no longer stops the test",
            ),
            Self::ExpectedFailureRemoved => (
                "MF106",
                "expected-failure-removed",
                "An expected-failure check was dropped or widened",
            ),
            Self::TestFileDeleted => (
                "MF107",
                "test-file-deleted",
                "A test file was deleted while its siblings changed",
            ),
            Self::TestRemoved => (
                "MF108",
                "test-removed",
                "A test that used to run no longer runs",
            ),
            Self::SuiteNotRun => (
                "MF109",
                "suite-not-run",
                "TestMain no longer runs the suite, so no test in the package executes",
            ),
            Self::FileExcluded => (
                "MF110",
                "file-excluded",
                "A build constraint now excludes this file, so its tests no longer run",
            ),
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        self.facts().0
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.facts().1
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        self.facts().2
    }

    fn slot(self) -> u32 {
        self.id()[2..].parse::<u32>().unwrap_or(101) - 101
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleSet {
    mask: u32,
}

impl RuleSet {
    #[must_use]
    pub fn all() -> Self {
        let mut mask = 0;

        for rule in Rule::ALL {
            mask |= 1 << rule.slot();
        }

        Self { mask }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self { mask: 0 }
    }

    #[must_use]
    pub fn contains(self, rule: Rule) -> bool {
        self.mask & (1 << rule.slot()) != 0
    }

    #[must_use]
    pub fn with(self, rule: Rule) -> Self {
        Self {
            mask: self.mask | (1 << rule.slot()),
        }
    }

    #[must_use]
    pub fn without(self, rule: Rule) -> Self {
        Self {
            mask: self.mask & !(1 << rule.slot()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,

    Medium,

    High,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub file: String,

    pub line: usize,

    pub rule: Rule,

    pub confidence: Confidence,

    pub detail: String,

    pub test: Option<String>,
}

impl Finding {
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.file,
            self.rule.id(),
            self.test.as_deref().unwrap_or("")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_has_a_unique_id_and_name() {
        let mut ids: Vec<&str> = Rule::ALL.iter().map(|r| r.id()).collect();
        let count = ids.len();

        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "rule ids must be unique");

        let mut names: Vec<&str> = Rule::ALL.iter().map(|r| r.name()).collect();

        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), count, "rule names must be unique");
    }

    #[test]
    fn every_rule_id_is_well_formed() {
        for rule in Rule::ALL {
            let id = rule.id();
            assert_eq!(id.len(), 5, "{id} must be MF plus three digits");
            assert!(id.starts_with("MF"), "{id}");
            assert!(id[2..].chars().all(|c| c.is_ascii_digit()), "{id}");
            assert!(!rule.description().is_empty());
        }
    }

    #[test]
    fn every_rule_has_a_distinct_slot() {
        let mut slots: Vec<u32> = Rule::ALL.iter().map(|r| r.slot()).collect();
        let count = slots.len();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), count, "two rules would share a bit");
        assert!(slots.iter().all(|s| *s < 32), "the mask has room");
    }

    #[test]
    fn a_rule_set_holds_exactly_what_was_put_in_it() {
        let all = RuleSet::all();
        assert!(Rule::ALL.iter().all(|r| all.contains(*r)));

        let without = all.without(Rule::AssertionLoosened);
        assert!(!without.contains(Rule::AssertionLoosened));
        assert!(without.contains(Rule::AssertionRemoved));

        let only_one = RuleSet::none().with(Rule::TestRemoved);
        assert!(only_one.contains(Rule::TestRemoved));
        assert_eq!(
            Rule::ALL.iter().filter(|r| only_one.contains(**r)).count(),
            1
        );
    }

    #[test]
    fn confidence_orders_from_low_to_high() {
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
    }
}
