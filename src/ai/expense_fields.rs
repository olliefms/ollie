// src/ai/expense_fields.rs
//
// Best-effort structured extraction of receipt fields for expense review
// suggestions. Never authoritative, never fatal: any failure yields None and
// the fleet manager types the values by hand.
use serde::Deserialize;

use crate::ai::{
    extract::{
        bytes_to_base64, fit_image_for_vision, scanned_pdf_page_image, word_count,
        Extractable, MAX_VISION_BYTES, MAX_VISION_DIM,
    },
    ocr::ocr_image,
    OllamaClient,
};

/// Minimum OCR word count to trust tesseract output over the vision model —
/// same gate the summarization pipeline uses (see pipeline/worker.rs, #372).
const OCR_MIN_WORDS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedExpenseFields {
    pub amount: Option<f64>,
    pub date: Option<String>,
    pub vendor: Option<String>,
    pub card_last4: Option<String>,
}

#[derive(Deserialize)]
struct RawFields {
    amount: Option<serde_json::Value>,
    date: Option<String>,
    vendor: Option<String>,
    card_last4: Option<String>,
}

const PROMPT: &str = "You are reading a purchase receipt or invoice. Extract exactly these fields and reply with ONLY a JSON object, no other text: {\"amount\": <total charged as a number, or null>, \"date\": <purchase date as YYYY-MM-DD, or null>, \"vendor\": <merchant name, or null>, \"card_last4\": <last 4 digits of the card used, or null>}";

/// Find the outermost {...} in the model reply and parse it leniently.
pub fn parse_expense_json(raw: &str) -> Option<SuggestedExpenseFields> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    let parsed: RawFields = serde_json::from_str(&raw[start..=end]).ok()?;
    let amount = parsed
        .amount
        .and_then(|v| match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.trim().trim_start_matches('$').parse().ok(),
            _ => None,
        })
        .filter(|a| *a >= 0.0);
    let non_empty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let out = SuggestedExpenseFields {
        amount,
        date: non_empty(parsed.date),
        vendor: non_empty(parsed.vendor),
        card_last4: non_empty(parsed.card_last4),
    };
    if out.amount.is_none() && out.date.is_none() && out.vendor.is_none() && out.card_last4.is_none() {
        return None;
    }
    Some(out)
}

/// Ask the text model to extract fields from receipt text (OCR output, a text
/// upload, or a scanned PDF's text layer).
async fn extract_from_text(ai: &OllamaClient, text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let capped: String = text.chars().take(6000).collect();
    ai.generate(&ai.summary_model.clone(), &format!("{PROMPT}\n\nReceipt text:\n{capped}"), None)
        .await
        .ok()
}

/// Read a receipt image OCR-first, mirroring the summarization pipeline
/// (#372): tesseract reads printed receipts far better than the local vision
/// model, so extract from OCR text when it yields enough words and fall back
/// to the vision model only for images with no machine-readable text.
async fn extract_from_image(ai: &OllamaClient, image: Vec<u8>) -> Option<String> {
    if let Some(text) = ocr_image(&image).await {
        if word_count(&text) >= OCR_MIN_WORDS {
            return extract_from_text(ai, &text).await;
        }
    }
    let fitted = tokio::task::spawn_blocking(move || {
        fit_image_for_vision(&image, MAX_VISION_BYTES, MAX_VISION_DIM)
    })
    .await
    .ok()
    .flatten()?;
    ai.generate(&ai.vision_model.clone(), PROMPT, Some(bytes_to_base64(&bytes::Bytes::from(fitted))))
        .await
        .ok()
}

/// Best-effort receipt field extraction. Never errors — any failure along the
/// way (unsupported content, model call failure, unparseable reply) simply
/// yields `None` and the caller leaves the expense's suggested_* fields alone.
pub async fn extract_expense_fields(ai: &OllamaClient, content: &Extractable) -> Option<SuggestedExpenseFields> {
    let raw = match content {
        Extractable::Text(text) => extract_from_text(ai, text).await?,
        Extractable::ImageBytes(bytes) => extract_from_image(ai, bytes.to_vec()).await?,
        Extractable::ScannedPdf(bytes, raw_text) => {
            let bytes = bytes.clone();
            let page_image = tokio::task::spawn_blocking(move || scanned_pdf_page_image(&bytes))
                .await
                .ok()
                .flatten();
            let extracted = match page_image {
                Some(img) => extract_from_image(ai, img).await,
                None => None,
            };
            match extracted {
                Some(raw) => raw,
                None => extract_from_text(ai, raw_text).await?,
            }
        }
        Extractable::Unsupported => return None,
    };
    parse_expense_json(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_clean_json() {
        let s = parse_expense_json(r#"{"amount": 84.12, "date": "2026-07-18", "vendor": "Pilot #442", "card_last4": "9910"}"#).unwrap();
        assert_eq!(s.amount, Some(84.12));
        assert_eq!(s.date.as_deref(), Some("2026-07-18"));
        assert_eq!(s.vendor.as_deref(), Some("Pilot #442"));
        assert_eq!(s.card_last4.as_deref(), Some("9910"));
    }

    #[test]
    fn test_parses_json_wrapped_in_prose_and_fences() {
        let raw = "Sure! Here is the extraction:\n```json\n{\"amount\": 12.5, \"date\": null, \"vendor\": \"CAT Scale\", \"card_last4\": null}\n```";
        let s = parse_expense_json(raw).unwrap();
        assert_eq!(s.amount, Some(12.5));
        assert_eq!(s.date, None);
        assert_eq!(s.vendor.as_deref(), Some("CAT Scale"));
    }

    #[test]
    fn test_amount_as_string_is_coerced() {
        let s = parse_expense_json(r#"{"amount": "84.12", "date": null, "vendor": null, "card_last4": null}"#).unwrap();
        assert_eq!(s.amount, Some(84.12));
    }

    #[test]
    fn test_garbage_returns_none() {
        assert!(parse_expense_json("I could not read this receipt.").is_none());
        assert!(parse_expense_json("{not json").is_none());
    }

    #[test]
    fn test_all_null_fields_returns_none() {
        // Nothing extracted -> treat as no suggestion at all.
        assert!(parse_expense_json(r#"{"amount": null, "date": null, "vendor": null, "card_last4": null}"#).is_none());
    }
}
