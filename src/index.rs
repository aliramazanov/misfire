#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Specific,

    Broad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Fatal,

    NonFatal,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Assertion {
    pub line: usize,

    pub call: String,

    pub strength: Strength,

    pub severity: Severity,

    pub checks_failure: bool,

    pub guard: bool,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Disabled {
    pub line: usize,

    pub marker: String,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExpectedFailure {
    pub line: usize,

    pub marker: String,

    pub expected: Option<String>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TestFn {
    pub name: String,

    pub line: usize,

    pub assertions: Vec<Assertion>,

    pub disabled: Option<Disabled>,

    pub expected_failure: Option<ExpectedFailure>,

    pub subtests: Vec<String>,

    pub helper_calls: Vec<String>,

    pub delegating_calls: Vec<String>,

    pub end_line: usize,

    pub allows: Vec<String>,

    pub disabled_subtest: Option<String>,

    pub trusted: bool,
}

impl TestFn {
    pub fn allows_rule(&self, rule: &str) -> bool {
        self.allows.iter().any(|a| a == rule)
    }

    #[must_use]
    pub fn assertion_count(&self) -> usize {
        self.assertions.len()
    }

    #[must_use]
    pub fn fatal_count(&self) -> usize {
        self.assertions
            .iter()
            .filter(|a| a.severity == Severity::Fatal)
            .count()
    }

    #[must_use]
    pub fn specific_count(&self) -> usize {
        self.assertions
            .iter()
            .filter(|a| a.strength == Strength::Specific)
            .count()
    }

    #[must_use]
    pub fn verifying_count(&self) -> usize {
        self.assertions.iter().filter(|a| !a.guard).count()
    }

    #[must_use]
    pub fn failure_checks(&self) -> usize {
        self.assertions.iter().filter(|a| a.checks_failure).count()
    }
}

#[derive(Debug, Clone)]
pub struct SuiteRunner {
    pub line: usize,

    pub runs: bool,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TestFile {
    pub tests: Vec<TestFn>,

    pub build_constraint: Option<String>,

    pub suite_runner: Option<SuiteRunner>,

    pub helpers: Vec<TestFn>,

    pub allows: Vec<String>,

    pub parsed: bool,
}

impl Default for TestFile {
    fn default() -> Self {
        Self {
            tests: Vec::new(),
            helpers: Vec::new(),
            allows: Vec::new(),
            build_constraint: None,
            suite_runner: None,
            parsed: true,
        }
    }
}

impl TestFile {
    pub fn allows_rule(&self, rule: &str) -> bool {
        self.allows.iter().any(|a| a == rule)
    }

    pub fn untrusted(&self) -> impl Iterator<Item = &TestFn> {
        self.tests.iter().filter(|t| !t.trusted)
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TestFn> {
        self.tests.iter().find(|t| t.name == name)
    }

    #[must_use]
    pub fn effective_assertions(&self, test: &TestFn) -> usize {
        let mut chain = Vec::new();
        test.assertion_count() + self.via_helpers(&test.helper_calls, 0, &mut chain)
    }

    #[must_use]
    pub fn effective_fatal(&self, test: &TestFn) -> usize {
        test.fatal_count()
    }

    fn via_helpers(&self, calls: &[String], depth: usize, chain: &mut Vec<String>) -> usize {
        if depth > 8 {
            return 0;
        }
        let mut total = 0;
        for name in calls {
            if chain.iter().any(|c| c == name) {
                continue;
            }
            if let Some(h) = self.helpers.iter().find(|h| &h.name == name) {
                chain.push(name.clone());
                total += h.assertion_count() + self.via_helpers(&h.helper_calls, depth + 1, chain);
                chain.pop();
            }
        }
        total
    }
}

pub fn attach_suppressions(src: &str, file: &mut TestFile) {
    for (idx, raw) in src.lines().enumerate() {
        let line = idx + 1;
        let rules = parse_allow(raw);
        if rules.is_empty() {
            continue;
        }
        let enclosing = file
            .tests
            .iter()
            .position(|t| line >= t.line && line <= t.end_line);
        let target = enclosing.or_else(|| {
            file.tests
                .iter()
                .enumerate()
                .filter(|(_, t)| t.line > line)
                .min_by_key(|(_, t)| t.line)
                .map(|(i, _)| i)
        });
        file.allows.extend(rules.iter().cloned());
        if let Some(i) = target {
            file.tests[i].allows.extend(rules);
        }
    }
}

fn parse_allow(line: &str) -> Vec<String> {
    let Some(rest) = line.split("misfire:allow").nth(1) else {
        return Vec::new();
    };
    rest.split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .take_while(|tok| is_rule_id(tok))
        .map(std::string::ToString::to_string)
        .collect()
}

fn is_rule_id(tok: &str) -> bool {
    tok.len() == 5 && tok.starts_with("MF") && tok[2..].chars().all(|c| c.is_ascii_digit())
}

pub fn error_ranges(root: tree_sitter::Node) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    if !root.has_error() {
        return ranges;
    }
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            ranges.push(n.byte_range());
            continue;
        }
        if !n.has_error() {
            continue;
        }
        let mut c = n.walk();
        for child in n.children(&mut c) {
            stack.push(child);
        }
    }
    ranges
}

pub const MAX_DEPTH: usize = 256;

#[must_use]
pub fn spans_an_error(node: tree_sitter::Node, errors: &[std::ops::Range<usize>]) -> bool {
    let span = node.byte_range();
    errors.iter().any(|e| {
        if e.start == e.end {
            e.start >= span.start && e.start <= span.end
        } else {
            e.start < span.end && span.start < e.end
        }
    })
}
