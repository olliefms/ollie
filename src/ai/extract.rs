// src/ai/extract.rs
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;

pub enum Extractable {
    Text(String),
    /// Scanned PDF: raw bytes + whatever garbled text was extracted (may be empty)
    ScannedPdf(Bytes, String),
    /// True image bytes to send to the vision model
    ImageBytes(Bytes),
    Unsupported,
}

pub fn extract_content(data: &Bytes, mime_type: &str) -> Extractable {
    if mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/xml"
        || mime_type.contains("javascript")
    {
        return Extractable::Text(String::from_utf8_lossy(data).into_owned());
    }

    if mime_type == "application/pdf" {
        let text = extract_pdf_text(data);
        if word_count(&text) >= 50 {
            return Extractable::Text(text);
        }
        return Extractable::ScannedPdf(data.clone(), text);
    }

    if mime_type.starts_with("image/") {
        return Extractable::ImageBytes(data.clone());
    }

    Extractable::Unsupported
}

pub fn bytes_to_base64(data: &Bytes) -> String {
    general_purpose::STANDARD.encode(data)
}

fn extract_pdf_text(data: &[u8]) -> String {
    pdf_extract::extract_text_from_mem(data).unwrap_or_default()
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_plain_text() {
        let data = Bytes::from("hello world this is text");
        assert!(matches!(extract_content(&data, "text/plain"), Extractable::Text(t) if t == "hello world this is text"));
    }

    #[test]
    fn test_extract_json() {
        let data = Bytes::from(r#"{"key": "value"}"#);
        assert!(matches!(extract_content(&data, "application/json"), Extractable::Text(_)));
    }

    #[test]
    fn test_extract_image_returns_bytes() {
        let data = Bytes::from(vec![0xFF, 0xD8, 0xFF]);
        assert!(matches!(extract_content(&data, "image/jpeg"), Extractable::ImageBytes(_)));
    }

    #[test]
    fn test_extract_binary_returns_unsupported() {
        let data = Bytes::from(vec![0x00, 0x01, 0x02]);
        assert!(matches!(extract_content(&data, "application/octet-stream"), Extractable::Unsupported));
    }

    #[test]
    fn test_word_count() {
        assert_eq!(word_count("hello world foo"), 3);
        assert_eq!(word_count(""), 0);
    }


    #[test]
    fn test_bytes_to_base64_roundtrips() {
        let data = Bytes::from("test data");
        let b64 = bytes_to_base64(&data);
        let decoded = general_purpose::STANDARD.decode(&b64).unwrap();
        assert_eq!(decoded, b"test data");
    }
}
