use crate::{Confidence, Finding};
use serde_json::{Value, json};

#[must_use]
pub fn build(findings: &[Finding]) -> Value {
    let rules: Vec<Value> = crate::Rule::ALL
        .iter()
        .map(|r| {
            json!({
                "id": r.id(),
                "name": r.name(),
                "shortDescription": { "text": r.description() },
            })
        })
        .collect();

    let results: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "ruleId": f.rule.id(),
                "level": level(f.confidence),
                "message": { "text": f.detail },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": uri_reference(&f.file) },
                        "region": { "startLine": f.line }
                    }
                }],
            })
        })
        .collect();

    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "misfire",
                    "informationUri": "https://github.com/aliramazanov/misfire",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules,
                }
            },
            "results": results,
        }],
    })
}

fn uri_reference(path: &str) -> String {
    const UNRESERVED_EXTRA: [u8; 5] = *b"-._~/";

    let mut out = String::with_capacity(path.len());

    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED_EXTRA.contains(&byte) {
            out.push(byte as char);
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }

    out
}

fn level(c: Confidence) -> &'static str {
    match c {
        Confidence::High => "error",
        Confidence::Medium => "warning",
        Confidence::Low => "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_paths_are_untouched() {
        assert_eq!(uri_reference("src/lib.rs"), "src/lib.rs");
        assert_eq!(uri_reference("a_test.go"), "a_test.go");
        assert_eq!(uri_reference("a-b.c~d/e_f.rs"), "a-b.c~d/e_f.rs");
    }

    #[test]
    fn spaces_and_specials_are_percent_encoded() {
        assert_eq!(uri_reference("od d/b_test.go"), "od%20d/b_test.go");
        assert_eq!(uri_reference("a#b?c.rs"), "a%23b%3Fc.rs");
        assert_eq!(uri_reference("100%.rs"), "100%25.rs");
    }

    #[test]
    fn non_ascii_is_encoded_as_utf8_bytes() {
        assert_eq!(uri_reference("café.rs"), "caf%C3%A9.rs");
    }

    #[test]
    fn a_newline_in_a_path_cannot_break_the_document() {
        let encoded = uri_reference("evil\n.rs");
        assert!(!encoded.contains('\n'), "{encoded}");
        assert_eq!(encoded, "evil%0A.rs");
    }
}
