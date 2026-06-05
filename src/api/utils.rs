pub fn sanitize_filename(name: &str) -> String {
    // Strip CR/LF (header injection), then backslash-escape `\` and `"`
    // because the value is interpolated into a quoted Content-Disposition
    // header. Backslash must be replaced first so the backslashes inserted
    // for `"` aren't re-escaped.
    let stripped: String = name.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    stripped.replace('\\', r"\\").replace('"', r#"\""#)
}

/// Build the 409 body for a blocked permanent delete.
///
/// `entity` is the singular noun being deleted ("driver"); `referrers` pairs a
/// referrer kind ("trips") with how many point at the object. Zero-count pairs
/// are skipped. Callers wrap the result in `AppError::Conflict`.
pub fn referrer_conflict_message(entity: &str, referrers: &[(&str, usize)]) -> String {
    let parts: Vec<String> = referrers
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(kind, n)| format!("{n} {kind}"))
        .collect();
    format!(
        "cannot permanently delete {entity}: referenced by {}",
        parts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_strips_crlf() {
        assert_eq!(sanitize_filename("report.pdf\r\nX-Injected: evil"), "report.pdfX-Injected: evil");
    }

    #[test]
    fn test_sanitize_filename_passthrough() {
        assert_eq!(sanitize_filename("invoice 2026-05-14.pdf"), "invoice 2026-05-14.pdf");
    }

    #[test]
    fn test_sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "");
    }

    #[test]
    fn test_sanitize_filename_escapes_quote() {
        assert_eq!(sanitize_filename(r#"a"b.pdf"#), r#"a\"b.pdf"#);
    }

    #[test]
    fn test_sanitize_filename_escapes_backslash() {
        assert_eq!(sanitize_filename(r"a\b.pdf"), r"a\\b.pdf");
    }

    #[test]
    fn test_sanitize_filename_escapes_both() {
        // Backslash must be escaped FIRST so the inserted `\` for the quote
        // doesn't get double-escaped.
        assert_eq!(sanitize_filename(r#"a\"b.pdf"#), r#"a\\\"b.pdf"#);
    }
}

#[cfg(test)]
mod referrer_message_tests {
    use super::referrer_conflict_message;

    #[test]
    fn formats_single_referrer_kind_with_count() {
        let msg = referrer_conflict_message("driver", &[("trips", 3)]);
        assert_eq!(msg, "cannot permanently delete driver: referenced by 3 trips");
    }

    #[test]
    fn formats_multiple_referrer_kinds() {
        let msg = referrer_conflict_message("truck", &[("trips", 2), ("drivers", 1)]);
        assert_eq!(
            msg,
            "cannot permanently delete truck: referenced by 2 trips, 1 drivers"
        );
    }

    #[test]
    fn skips_zero_count_referrers() {
        let msg = referrer_conflict_message("facility", &[("loads", 0), ("trips", 4)]);
        assert_eq!(msg, "cannot permanently delete facility: referenced by 4 trips");
    }
}
