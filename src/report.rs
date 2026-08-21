use crate::{Confidence, Finding};
use std::fmt::Write;

#[must_use]
pub fn text(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "misfire: no findings\n".to_string();
    }

    let files = distinct_files(findings);
    let width = findings.iter().map(location_width).max().unwrap_or(0);

    let mut out = String::with_capacity(findings.len() * 96);

    let _ = writeln!(
        out,
        "misfire: {} finding{} in {files} file{}\n",
        findings.len(),
        plural(findings.len()),
        plural(files)
    );

    for f in findings {
        let location = format!("{}:{}", printable(&f.file), f.line);

        let _ = writeln!(
            out,
            "  {location:width$}  {}  {}",
            f.rule.id(),
            printable(&f.detail)
        );
    }

    let _ = writeln!(
        out,
        "\n  {} finding{}. Review the test changes before merging.",
        findings.len(),
        plural(findings.len())
    );
    out
}

#[must_use]
pub fn github_annotations(findings: &[Finding]) -> String {
    let mut out = String::with_capacity(findings.len() * 128);

    for f in findings {
        let level = match f.confidence {
            Confidence::High => "error",
            Confidence::Medium | Confidence::Low => "warning",
        };

        let _ = writeln!(
            out,
            "::{level} file={},line={},title=misfire {}::{}",
            escape_property(&f.file),
            f.line,
            f.rule.id(),
            escape_data(&f.detail)
        );
    }

    out
}

fn distinct_files(findings: &[Finding]) -> usize {
    let mut files: Vec<&str> = findings.iter().map(|f| f.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    files.len()
}

fn location_width(f: &Finding) -> usize {
    printable(&f.file).chars().count() + 1 + decimal_width(f.line)
}

fn printable(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.chars().any(char::is_control) {
        return std::borrow::Cow::Borrowed(value);
    }

    let mut out = String::with_capacity(value.len() + 8);

    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }

    std::borrow::Cow::Owned(out)
}

fn decimal_width(mut n: usize) -> usize {
    let mut digits = 1;

    while n >= 10 {
        n /= 10;
        digits += 1;
    }

    digits
}

fn escape_data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_property(value: &str) -> String {
    escape_data(value).replace(':', "%3A").replace(',', "%2C")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_character_in_a_path_cannot_forge_a_finding_line() {
        let f = Finding {
            file: "evil\n  fake_test.go:1  MF101  faked".into(),
            line: 3,
            rule: crate::Rule::AssertionRemoved,
            confidence: crate::Confidence::High,
            detail: "real".into(),
            test: Some("T".into()),
        };

        let out = text(&[f]);
        let finding_lines = out.lines().filter(|l| l.contains("MF101")).count();

        assert_eq!(
            finding_lines, 1,
            "one finding must print as one line:\n{out}"
        );

        assert!(out.contains("evil\\n"), "{out}");
    }

    #[test]
    fn ordinary_paths_are_not_rewritten() {
        assert_eq!(printable("src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn decimal_width_matches_formatting() {
        for n in [0usize, 1, 9, 10, 99, 100, 999, 1000, 123_456] {
            assert_eq!(decimal_width(n), n.to_string().len(), "for {n}");
        }
    }
}
