use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

use crate::Finding;

pub const FILE_NAME: &str = ".misfire-baseline.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,

    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Entry {
    pub file: String,

    pub rule: String,
    #[serde(default)]
    pub test: String,
}

impl Baseline {
    #[must_use]
    pub fn from_findings(findings: &[Finding]) -> Self {
        let mut entries: Vec<Entry> = findings
            .iter()
            .map(|f| Entry {
                file: f.file.clone(),
                rule: f.rule.id().to_string(),
                test: f.test.clone().unwrap_or_default(),
            })
            .collect();

        entries.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.rule.cmp(&b.rule))
                .then(a.test.cmp(&b.test))
        });

        entries.dedup();

        Self {
            version: 1,
            entries,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;

        serde_json::from_str(&raw).map_err(|source| Error::BaselineSyntax {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let body = serde_json::to_string_pretty(self).unwrap_or_default();

        std::fs::write(path, format!("{body}\n")).map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    #[must_use]
    pub fn keys(&self) -> HashSet<String> {
        self.entries
            .iter()
            .map(|e| format!("{}\u{1f}{}\u{1f}{}", e.file, e.rule, e.test))
            .collect()
    }
}

#[must_use]
pub fn filter(findings: Vec<Finding>, baseline: &Baseline) -> Vec<Finding> {
    let known = baseline.keys();

    findings
        .into_iter()
        .filter(|f| !known.contains(&f.key()))
        .collect()
}
