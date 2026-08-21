use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("not inside a git repository")]
    NotARepository,
    #[error("git could not be run; is it installed and on PATH?")]
    GitUnavailable(#[source] std::io::Error),
    #[error("git {command} failed: {stderr}")]
    GitFailed { command: String, stderr: String },
    #[error("git produced output misfire could not read: {0}")]
    MalformedGitOutput(&'static str),
    #[error("base ref {base:?} does not exist and could not be fetched")]
    BaseNotFound { base: String, shallow: bool },
    #[error("{base:?} and HEAD share no common ancestor")]
    NoMergeBase { base: String, shallow: bool },
    #[error("reading {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}")]
    ConfigSyntax {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("invalid pattern {pattern:?} in misfire.toml")]
    InvalidPattern {
        pattern: String,
        #[source]
        source: Box<globset::Error>,
    },
    #[error("unknown rule {rule:?} in misfire.toml")]
    UnknownRule { rule: String },
    #[error("parsing baseline {path}")]
    BaselineSyntax {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    #[must_use]
    pub fn is_shallow_checkout_problem(&self) -> bool {
        matches!(
            self,
            Self::BaseNotFound { shallow: true, .. } | Self::NoMergeBase { shallow: true, .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;
