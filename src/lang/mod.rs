pub mod go;

pub mod rust;

use crate::config::Config;
use crate::index::TestFile;
use std::path::Path;
use tree_sitter::Node;

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

pub(crate) struct Vocabulary {
    words: std::collections::HashSet<String>,
}

impl Vocabulary {
    pub(crate) fn new(words: &[String]) -> Self {
        Self {
            words: words.iter().cloned().collect(),
        }
    }

    pub(crate) fn contains(&self, value: &str) -> bool {
        self.words.contains(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Go,

    Rust,
}

impl Lang {
    #[must_use]
    pub fn for_path(path: &str) -> Option<Self> {
        if path.ends_with("_test.go") {
            return Some(Self::Go);
        }
        let extension = Path::new(path).extension()?;
        extension.eq_ignore_ascii_case("rs").then_some(Self::Rust)
    }

    #[must_use]
    pub fn index(self, src: &str, cfg: &Config) -> TestFile {
        match self {
            Self::Go => go::index(src, &cfg.go),
            Self::Rust => rust::index(src, &cfg.rust),
        }
    }
}
