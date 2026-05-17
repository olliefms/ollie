pub fn sanitize_filename(name: &str) -> String {
    name.chars().filter(|c| *c != '\r' && *c != '\n').collect()
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
}
