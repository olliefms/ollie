// src/ai/extract.rs
use base64::{engine::general_purpose, Engine as _};
use bytes::Bytes;

pub enum Extractable {
    Text(String),
    /// Raw bytes to send to the vision model (image or sparse PDF)
    ImageBytes(Bytes),
    /// lopdf extracted enough tokens but the alphanumeric ratio is too low —
    /// likely CID-keyed or custom-encoded fonts. Carries the raw lopdf text for
    /// use as auxiliary context in hybrid vision extraction.
    GibberishPdf(String),
    Unsupported,
}

pub fn extract_content(data: &Bytes, mime_type: &str) -> Extractable {
    if mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/xml"
        || mime_type.contains("javascript")
    {
        let text = String::from_utf8_lossy(data).into_owned();
        tracing::info!(
            outcome = "text",
            mime_type = mime_type,
            size_bytes = data.len(),
            "extract_outcome"
        );
        return Extractable::Text(text);
    }

    if mime_type == "application/pdf" {
        let (text, pdf_meta) = extract_pdf_text_and_meta(data);
        let wc = word_count(&text);
        let alnum_ratio = alnum_ratio(&text);
        if wc >= 50 {
            if is_gibberish_ratio(alnum_ratio) {
                tracing::info!(
                    outcome = "gibberish_pdf",
                    mime_type = mime_type,
                    size_bytes = data.len(),
                    word_count = wc,
                    alnum_ratio = alnum_ratio,
                    pdf_producer = pdf_meta.producer.as_deref().unwrap_or(""),
                    pdf_creator = pdf_meta.creator.as_deref().unwrap_or(""),
                    "extract_outcome"
                );
                return Extractable::GibberishPdf(text);
            }
            tracing::info!(
                outcome = "text",
                mime_type = mime_type,
                size_bytes = data.len(),
                word_count = wc,
                alnum_ratio = alnum_ratio,
                pdf_producer = pdf_meta.producer.as_deref().unwrap_or(""),
                pdf_creator = pdf_meta.creator.as_deref().unwrap_or(""),
                "extract_outcome"
            );
            return Extractable::Text(text);
        }
        tracing::info!(
            outcome = "image_bytes",
            mime_type = mime_type,
            size_bytes = data.len(),
            word_count = wc,
            alnum_ratio = alnum_ratio,
            pdf_producer = pdf_meta.producer.as_deref().unwrap_or(""),
            pdf_creator = pdf_meta.creator.as_deref().unwrap_or(""),
            "extract_outcome"
        );
        return Extractable::ImageBytes(data.clone());
    }

    if mime_type.starts_with("image/") {
        tracing::info!(
            outcome = "image_bytes",
            mime_type = mime_type,
            size_bytes = data.len(),
            "extract_outcome"
        );
        return Extractable::ImageBytes(data.clone());
    }

    tracing::info!(
        outcome = "unsupported",
        mime_type = mime_type,
        size_bytes = data.len(),
        "extract_outcome"
    );
    Extractable::Unsupported
}

pub fn bytes_to_base64(data: &Bytes) -> String {
    general_purpose::STANDARD.encode(data)
}

struct PdfMeta {
    producer: Option<String>,
    creator: Option<String>,
}

fn extract_pdf_text_and_meta(data: &[u8]) -> (String, PdfMeta) {
    let Ok(doc) = lopdf::Document::load_mem(data) else {
        return (String::new(), PdfMeta { producer: None, creator: None });
    };

    let meta = extract_pdf_meta(&doc);

    let page_nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    let mut text = String::new();
    for page_num in page_nums {
        if let Ok(page_text) = doc.extract_text(&[page_num]) {
            text.push_str(&page_text);
            text.push('\n');
        }
    }
    (text, meta)
}

fn extract_pdf_meta(doc: &lopdf::Document) -> PdfMeta {
    let info_obj = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|obj| obj.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok());

    let Some(lopdf::Object::Dictionary(info)) = info_obj else {
        return PdfMeta { producer: None, creator: None };
    };

    let get_str = |key: &[u8]| -> Option<String> {
        info.get(key)
            .ok()
            .and_then(|o| o.as_str().ok())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .filter(|s| !s.is_empty())
    };

    PdfMeta {
        producer: get_str(b"Producer"),
        creator: get_str(b"Creator"),
    }
}

fn alnum_ratio(text: &str) -> f64 {
    let alphanumeric = text.chars().filter(|c| c.is_alphanumeric()).count();
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return 1.0;
    }
    alphanumeric as f64 / total as f64
}

fn is_gibberish_ratio(ratio: f64) -> bool {
    ratio < 0.5
}

fn is_gibberish(text: &str) -> bool {
    is_gibberish_ratio(alnum_ratio(text))
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
    fn test_is_gibberish_with_clean_text() {
        assert!(!is_gibberish("Landstar Ranger Inc Freight Bill 4385951 Equipment 53VN Total Miles 2217"));
    }

    #[test]
    fn test_is_gibberish_with_symbol_heavy_text() {
        // Simulates lopdf output from CID-encoded fonts: lots of symbols, few alphanumeric chars
        let garbage: String = "⌁⌂⌃⌄⌅⌆⌇⌈⌉⌊⌋⌌⌍⌎⌏⌐⌑⌒⌓⌔⌕⌖⌗⌘⌙⌚⌛⌜⌝⌞⌟⌠⌡⌢⌣⌤⌥⌦⌧⌨〈〉⌫⌬⌭⌮⌯⌰⌱⌲⌳⌴⌵⌶⌷⌸⌹⌺⌻⌼⌽⌾⌿".repeat(3);
        assert!(is_gibberish(&garbage));
    }

    #[test]
    fn test_is_gibberish_empty_is_not_gibberish() {
        assert!(!is_gibberish(""));
    }

    #[test]
    fn test_bytes_to_base64_roundtrips() {
        let data = Bytes::from("test data");
        let b64 = bytes_to_base64(&data);
        let decoded = general_purpose::STANDARD.decode(&b64).unwrap();
        assert_eq!(decoded, b"test data");
    }
}
