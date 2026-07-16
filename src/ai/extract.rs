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

/// Recover the dominant embedded raster image from a scanned PDF's first page.
///
/// Scanner/camera-capture PDFs embed each page as one full-page JPEG XObject
/// whose stream filter is exactly `DCTDecode` — the stream content IS the JPEG
/// bytes, no decoding needed. Only that case is handled: a filter chain (e.g.
/// `[FlateDecode, DCTDecode]`) or non-JPEG encodings (CCITTFax, JBIG2, raw
/// Flate bitmaps) return None and the caller falls back to the text path.
/// Only the first page is considered — one page is enough context for a 1-2
/// sentence summary, and each extra page would be another vision-model call.
pub fn scanned_pdf_page_image(data: &[u8]) -> Option<Vec<u8>> {
    let doc = lopdf::Document::load_mem(data).ok()?;
    let (_, first_page) = doc.get_pages().into_iter().next()?;
    let images = doc.get_page_images(first_page).ok()?;
    images
        .into_iter()
        .filter(|img| img.filters.as_deref() == Some(&["DCTDecode".to_string()]))
        .max_by_key(|img| img.content.len())
        .map(|img| img.content.to_vec())
        .filter(|bytes| is_supported_image(bytes))
}

/// Shrink an image to fit the vision model's payload budget — in bytes AND
/// in pixel dimensions.
///
/// Bytes within `max_bytes` whose long edge is within `max_dim` pass through
/// untouched. Everything else is decoded, downscaled to a bounded long edge,
/// and re-encoded as JPEG; the sizes step down until one fits. The dimension
/// cap exists because moondream returns an EMPTY response for full-resolution
/// scans (e.g. 2432×3168) even when they're under the byte budget — the
/// failure trigger is resolution, not payload size (#372). Returns None when
/// the bytes can't be decoded or won't fit even at the smallest size —
/// callers treat that as "no image available".
pub fn fit_image_for_vision(bytes: &[u8], max_bytes: usize, max_dim: u32) -> Option<Vec<u8>> {
    if bytes.len() <= max_bytes && is_supported_image(bytes) {
        let dims = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        if let Some((w, h)) = dims {
            if w.max(h) <= max_dim {
                return Some(bytes.to_vec());
            }
        }
    }
    let img = image::load_from_memory(bytes).ok()?;
    let steps = if max_dim > 768 { vec![max_dim, 768] } else { vec![max_dim] };
    for max_dim in steps {
        let resized = if img.width().max(img.height()) > max_dim {
            img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        // JPEG has no alpha channel — flatten before encoding or the encoder errors.
        let rgb = image::DynamicImage::ImageRgb8(resized.to_rgb8());
        let mut buf = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 75);
        if rgb.write_with_encoder(encoder).is_err() {
            return None;
        }
        let buf = buf.into_inner();
        if buf.len() <= max_bytes {
            return Some(buf);
        }
    }
    None
}

pub(crate) fn word_count(s: &str) -> usize {
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

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 16, y as u8 * 16, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 75))
            .unwrap();
        buf.into_inner()
    }

    fn pdf_with_image(filter: &str, img_bytes: &[u8]) -> Vec<u8> {
        use lopdf::{dictionary, Document, Object, Stream};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let img_stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 8,
                "Height" => 8,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => filter,
            },
            img_bytes.to_vec(),
        );
        let img_id = doc.add_object(img_stream);
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"q 8 0 0 8 0 0 cm /Im0 Do Q".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "Contents" => Object::Reference(content_id),
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Im0" => Object::Reference(img_id) },
            },
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn test_scanned_pdf_page_image_recovers_dctdecode_jpeg() {
        let jpeg = tiny_jpeg();
        let pdf = pdf_with_image("DCTDecode", &jpeg);
        let recovered = scanned_pdf_page_image(&pdf).expect("page image should be recovered");
        assert_eq!(recovered, jpeg);
    }

    #[test]
    fn test_scanned_pdf_page_image_skips_non_jpeg_encodings() {
        // A FlateDecode bitmap's stream content is not directly usable JPEG.
        let pdf = pdf_with_image("FlateDecode", &[0u8; 192]);
        assert!(scanned_pdf_page_image(&pdf).is_none());
    }

    #[test]
    fn test_scanned_pdf_page_image_rejects_non_pdf_and_imageless_pdf() {
        assert!(scanned_pdf_page_image(b"not a pdf at all").is_none());
        // Valid magic but truncated/garbage body.
        assert!(scanned_pdf_page_image(b"%PDF-1.7\ngarbage").is_none());
    }

    #[test]
    fn test_fit_image_for_vision_passes_small_images_through() {
        let jpeg = tiny_jpeg();
        let out = fit_image_for_vision(&jpeg, 500_000, 1024).unwrap();
        assert_eq!(out, jpeg);
    }

    #[test]
    fn test_fit_image_for_vision_downscales_large_dimensions_under_byte_budget() {
        // A mostly-flat scan-resolution page compresses far below the byte
        // budget, but its raw pixel dimensions make moondream return an empty
        // response — it must be downscaled anyway (#372). This mirrors the
        // production repro: a 2432×3168 phone scan at 490 KB.
        let img = image::RgbImage::from_fn(2432, 3168, |x, _| {
            image::Rgb([240 + (x % 7) as u8, 255, 250])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 75))
            .unwrap();
        let flat = buf.into_inner();
        let max = 500_000;
        assert!(flat.len() <= max, "fixture must be under the byte budget (got {})", flat.len());

        let out = fit_image_for_vision(&flat, max, 1024).expect("image should be fitted");
        let decoded = image::load_from_memory(&out).unwrap();
        assert!(
            decoded.width().max(decoded.height()) <= 1024,
            "dimensions must be capped, got {}x{}",
            decoded.width(),
            decoded.height()
        );
    }

    #[test]
    fn test_fit_image_for_vision_shrinks_oversized_images() {
        // Noise-like fixture (hash of pixel coords) at scan-like resolution:
        // several MB encoded, far above the real 500 KB vision budget. Even
        // noise at the 1024px fallback step compresses under that budget, so
        // the shrink is deterministic.
        let img = image::RgbImage::from_fn(3500, 2500, |x, y| {
            let h = x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503) ^ (x.wrapping_mul(y));
            image::Rgb([h as u8, (h >> 8) as u8, (h >> 16) as u8])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90))
            .unwrap();
        let big = buf.into_inner();
        let max = 500_000;
        assert!(big.len() > max, "fixture must exceed the budget (got {})", big.len());
        let out = fit_image_for_vision(&big, max, 1024).expect("oversized image should be shrunk");
        assert!(out.len() <= max);
        assert!(is_supported_image(&out), "output must be a decodable JPEG");
        assert!(image::load_from_memory(&out).is_ok());
    }

    #[test]
    fn test_fit_image_for_vision_rejects_undecodable_payloads() {
        assert!(fit_image_for_vision(&vec![0xFFu8; 600_000], 500_000, 1024).is_none());
        assert!(fit_image_for_vision(b"hello", 500_000, 1024).is_none());
    }
}
