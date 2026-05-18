pub fn sanitize_filename(name: &str) -> String {
    // Strip CR/LF (header injection), then backslash-escape `\` and `"`
    // because the value is interpolated into a quoted Content-Disposition
    // header. Backslash must be replaced first so the backslashes inserted
    // for `"` aren't re-escaped.
    let stripped: String = name.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    stripped.replace('\\', r"\\").replace('"', r#"\""#)
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
