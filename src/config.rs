use crate::error::{Error, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE_NAME: &str = "misfire.toml";

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub go: GoConfig,
    #[serde(default)]
    pub rust: RustConfig,
    #[serde(default)]
    pub rules: RulesConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoConfig {
    #[serde(default = "default_fatal_packages")]
    pub fatal_packages: Vec<String>,
    #[serde(default = "default_nonfatal_packages")]
    pub nonfatal_packages: Vec<String>,
    #[serde(default = "default_broad_matchers")]
    pub broad_matchers: Vec<String>,
    #[serde(default = "default_failure_matchers")]
    pub failure_matchers: Vec<String>,
    #[serde(default)]
    pub assertion_patterns: Vec<String>,
    #[serde(default)]
    pub fatal_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustConfig {
    #[serde(default = "default_specific_macros")]
    pub specific_macros: Vec<String>,
    #[serde(default = "default_broad_macros")]
    pub broad_macros: Vec<String>,
    #[serde(default = "default_compiled_out_macros")]
    pub compiled_out_macros: Vec<String>,
    #[serde(default = "default_test_attributes")]
    pub test_attributes: Vec<String>,
    #[serde(default)]
    pub assertion_patterns: Vec<String>,
    #[serde(default = "default_assertion_methods")]
    pub assertion_methods: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RulesConfig {
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub file_deleted: bool,
    #[serde(default)]
    pub confidence: BTreeMap<String, crate::Confidence>,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FILE_NAME);

        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;

        let cfg: Self = toml::from_str(&raw).map_err(|source| Error::ConfigSyntax {
            path: path.clone(),
            source: Box::new(source),
        })?;

        cfg.validate()?;

        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        compile(&self.go.assertion_patterns)?;
        compile(&self.go.fatal_patterns)?;
        compile(&self.rust.assertion_patterns)?;
        compile(&self.rust.assertion_methods)?;

        for id in self
            .rules
            .disabled
            .iter()
            .chain(self.rules.confidence.keys())
        {
            if !crate::Rule::ALL.iter().any(|r| r.id() == id) {
                return Err(Error::UnknownRule { rule: id.clone() });
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn rule_enabled(&self, id: &str) -> bool {
        !self.rules.disabled.iter().any(|d| d == id)
    }

    #[must_use]
    pub fn confidence_overrides(&self) -> Vec<(crate::Rule, crate::Confidence)> {
        crate::Rule::ALL
            .iter()
            .filter_map(|r| self.rules.confidence.get(r.id()).map(|level| (*r, *level)))
            .collect()
    }

    #[must_use]
    pub fn rule_set(&self) -> crate::RuleSet {
        let mut set = crate::RuleSet::all();
        for rule in crate::Rule::ALL {
            if !self.rule_enabled(rule.id()) {
                set = set.without(*rule);
            }
        }
        if !self.rules.file_deleted {
            set = set.without(crate::Rule::TestFileDeleted);
        }
        set
    }
}

pub fn compile(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|source| Error::InvalidPattern {
            pattern: pattern.clone(),
            source: Box::new(source),
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| Error::InvalidPattern {
        pattern: patterns.join(", "),
        source: Box::new(source),
    })
}

fn default_fatal_packages() -> Vec<String> {
    vec!["require".into(), "must".into()]
}

fn default_nonfatal_packages() -> Vec<String> {
    vec!["assert".into(), "should".into()]
}

fn default_broad_matchers() -> Vec<String> {
    [
        "True",
        "False",
        "Nil",
        "NotNil",
        "Empty",
        "NotEmpty",
        "Zero",
        "NotZero",
        "Implements",
        "IsType",
        "Positive",
        "Negative",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

fn default_failure_matchers() -> Vec<String> {
    [
        "Error",
        "ErrorIs",
        "ErrorAs",
        "EqualError",
        "ErrorContains",
        "Panics",
        "PanicsWithValue",
        "PanicsWithError",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

fn default_specific_macros() -> Vec<String> {
    [
        "assert_eq",
        "assert_ne",
        "assert_matches",
        "debug_assert_eq",
        "debug_assert_ne",
        "assert_snapshot",
        "assert_debug_snapshot",
        "assert_json_snapshot",
        "assert_yaml_snapshot",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

fn default_broad_macros() -> Vec<String> {
    vec!["assert".into(), "debug_assert".into()]
}

fn default_compiled_out_macros() -> Vec<String> {
    vec![
        "debug_assert".into(),
        "debug_assert_eq".into(),
        "debug_assert_ne".into(),
    ]
}

fn default_assertion_methods() -> Vec<String> {
    vec!["assert*".into(), "expect*".into()]
}

fn default_test_attributes() -> Vec<String> {
    [
        "test",
        "tokio::test",
        "async_std::test",
        "actix_rt::test",
        "rstest",
        "test_case",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

impl Default for GoConfig {
    fn default() -> Self {
        Self {
            fatal_packages: default_fatal_packages(),
            nonfatal_packages: default_nonfatal_packages(),
            broad_matchers: default_broad_matchers(),
            failure_matchers: default_failure_matchers(),
            assertion_patterns: Vec::new(),
            fatal_patterns: Vec::new(),
        }
    }
}

impl Default for RustConfig {
    fn default() -> Self {
        Self {
            specific_macros: default_specific_macros(),
            broad_macros: default_broad_macros(),
            compiled_out_macros: default_compiled_out_macros(),
            test_attributes: default_test_attributes(),
            assertion_patterns: Vec::new(),
            assertion_methods: default_assertion_methods(),
        }
    }
}
