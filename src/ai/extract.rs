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
        // Guard the vision model: only forward bytes that are actually a
        // decodable raster image. Non-image bytes (e.g. a mislabeled upload)
        // crash Ollama's CLIP tokenizer with a SIGSEGV rather than erroring
        // cleanly — see is_supported_image. (#281)
        if is_supported_image(data) {
            return Extractable::ImageBytes(data.clone());
        }
        return Extractable::Unsupported;
    }

    Extractable::Unsupported
}

pub fn bytes_to_base64(data: &Bytes) -> String {
    general_purpose::STANDARD.encode(data)
}

/// Sniff common raster-image magic bytes. The Ollama vision model
/// (`moondream`/`llava`) segfaults its CLIP/multimodal tokenizer when handed
/// non-image input, so every byte payload bound for the vision model must
/// pass through this check first. Recognizes PNG, JPEG, GIF, BMP, and WebP —
/// notably NOT `application/pdf` (`%PDF` magic), which is exactly the input
/// that crashed the runner in #281.
pub(crate) fn is_supported_image(data: &[u8]) -> bool {
    let png = data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let jpeg = data.starts_with(&[0xFF, 0xD8, 0xFF]);
    let gif = data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a");
    let bmp = data.starts_with(b"BM");
    let webp = data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP";
    png || jpeg || gif || bmp || webp
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
    fn test_extract_image_mime_with_non_image_bytes_is_unsupported() {
        // A mislabeled upload (image/* MIME but the bytes are a PDF) must not
        // reach the vision model — it crashes the Ollama runner. (#281)
        let data = Bytes::from(&b"%PDF-1.7\n..."[..]);
        assert!(matches!(extract_content(&data, "image/png"), Extractable::Unsupported));
    }

    #[test]
    fn test_is_supported_image_recognizes_formats() {
        assert!(is_supported_image(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
        assert!(is_supported_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(is_supported_image(b"GIF89a..."));
        assert!(is_supported_image(b"BM......"));
        assert!(is_supported_image(b"RIFF\0\0\0\0WEBP"));
    }

    #[test]
    fn test_is_supported_image_rejects_pdf_and_junk() {
        assert!(!is_supported_image(b"%PDF-1.7"));
        assert!(!is_supported_image(&[0x00, 0x01, 0x02, 0x03]));
        assert!(!is_supported_image(b""));
        assert!(!is_supported_image(b"RIFF1234WAVE")); // RIFF but not WebP
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
