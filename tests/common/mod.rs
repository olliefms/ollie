// tests/common/mod.rs — shared fixtures for pipeline worker tests.
//
// These tests exercise process_blob directly against a mock Ollama HTTP
// server on an ephemeral local port. They live in separate integration-test
// binaries (pipeline_*_test.rs) because some of them set process-global env
// vars (OLLIE_TESSERACT_BIN) that must not leak across test binaries.
use axum::{routing::post, Json, Router};
use bytes::Bytes;
use chrono::Utc;
use ollie::{
    ai::OllamaClient,
    db::DbClient,
    models::blob::{BlobRecord, BlobStatus, BlobVisibility},
    storage::BlobStore,
};
use tempfile::TempDir;
use uuid::Uuid;

pub const TEST_EMBED_DIM: usize = 4;

/// Serve a mock Ollama API: /api/generate always answers `generate_response`,
/// /api/embeddings answers a fixed TEST_EMBED_DIM-dim vector.
pub async fn mock_ollama(generate_response: &'static str) -> String {
    let app = Router::new()
        .route(
            "/api/generate",
            post(move |_body: Json<serde_json::Value>| async move {
                Json(serde_json::json!({ "response": generate_response }))
            }),
        )
        .route(
            "/api/embeddings",
            post(|| async {
                Json(serde_json::json!({ "embedding": vec![0.1f32; TEST_EMBED_DIM] }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// A 1-page scanned-style PDF: no text layer, one full-page DCTDecode JPEG.
pub fn scanned_pdf() -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream};
    let jpeg = tiny_jpeg();
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
            "Filter" => "DCTDecode",
        },
        jpeg,
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

pub fn tiny_jpeg() -> Vec<u8> {
    let img = image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 16, y as u8 * 16, 0]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 75))
        .unwrap();
    buf.into_inner()
}

/// Insert a pending blob (bytes written to the store) and return its id plus
/// the handles process_blob needs. TempDirs are returned to keep them alive.
pub async fn seed_blob(
    data: Vec<u8>,
    mime_type: &str,
) -> (Uuid, DbClient, BlobStore, TempDir, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();
    let extract_dir = TempDir::new().unwrap();
    let db = DbClient::new(db_dir.path().to_str().unwrap(), TEST_EMBED_DIM)
        .await
        .unwrap();
    let store = BlobStore::new(blob_dir.path().to_str().unwrap());
    let bytes = Bytes::from(data);
    let checksum = store.write(&bytes).await.unwrap();
    let id = Uuid::new_v4();
    let record = BlobRecord {
        id,
        owner_id: 0,
        checksum,
        name: "scan.pdf".into(),
        mime_type: mime_type.into(),
        size: bytes.len() as i64,
        status: BlobStatus::Pending,
        error: None,
        summary: None,
        tags: vec![],
        embedding: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        visibility: BlobVisibility::Private,
        uploaded_by: None,
    };
    db.insert(&record).await.unwrap();
    (id, db, store, db_dir, blob_dir, extract_dir)
}

pub fn ai_client(base_url: &str) -> OllamaClient {
    OllamaClient::new(base_url, "nomic-embed-text", "llama3.2", "moondream")
}
