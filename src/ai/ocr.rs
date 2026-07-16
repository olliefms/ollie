// src/ai/ocr.rs
//
// OCR for scanned documents via the tesseract CLI. Local vision models
// (moondream/llava) cannot reliably read document scans — they return empty
// or hallucinated text — while tesseract reads printed scans nearly
// verbatim. The pipeline OCRs page images first and only falls back to the
// vision model when no machine-readable text is found. (#372)
//
// tesseract ships in the runtime Docker image (tesseract-ocr +
// tesseract-ocr-eng). Everywhere it's missing or fails, ocr_image returns
// None and callers degrade gracefully to the vision path.
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Hard cap on a single OCR run. A wedged tesseract must not stall a
/// pipeline worker forever.
const OCR_TIMEOUT_SECS: u64 = 120;

/// Binary to invoke; override with OLLIE_TESSERACT_BIN (used by tests, and
/// available as an ops escape hatch for non-standard installs).
fn tesseract_bin() -> String {
    std::env::var("OLLIE_TESSERACT_BIN").unwrap_or_else(|_| "tesseract".to_string())
}

/// OCR a raster image (JPEG/PNG bytes). Returns the recognized text, or None
/// when tesseract is unavailable, fails, times out, or finds no text.
pub async fn ocr_image(image: &[u8]) -> Option<String> {
    let bin = tesseract_bin();
    match tokio::time::timeout(
        std::time::Duration::from_secs(OCR_TIMEOUT_SECS),
        run_tesseract(&bin, image),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!("tesseract timed out after {OCR_TIMEOUT_SECS}s");
            None
        }
    }
}

async fn run_tesseract(bin: &str, image: &[u8]) -> Option<String> {
    let mut child = match Command::new(bin)
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            tracing::info!("tesseract unavailable ({bin}): {e}; falling back to vision model");
            return None;
        }
    };

    // tesseract reads stdin to EOF before emitting anything, so writing the
    // whole image first cannot deadlock against the stdout pipe.
    let mut stdin = child.stdin.take()?;
    if stdin.write_all(image).await.is_err() {
        return None;
    }
    drop(stdin);

    let output = child.wait_with_output().await.ok()?;
    if !output.status.success() {
        tracing::warn!("tesseract exited with {}", output.status);
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn fake_bin(dir: &std::path::Path, body: &str) -> String {
        let path = dir.join("fake-tesseract");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "#!/bin/sh\n{body}").unwrap();
        drop(f);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn test_run_tesseract_missing_binary_returns_none() {
        assert!(run_tesseract("/nonexistent/tesseract-372", b"\xFF\xD8\xFF").await.is_none());
    }

    #[tokio::test]
    async fn test_run_tesseract_returns_stdout_text() {
        let dir = tempfile::TempDir::new().unwrap();
        let bin = fake_bin(dir.path(), "cat >/dev/null\necho 'INVOICE 110148792 Boss Shop'\n");
        // Retry: exec'ing a just-written script can hit transient ETXTBSY when
        // a parallel test forks while the script's fd is briefly open. Real
        // deployments spawn a preexisting system binary, so only the test
        // needs this.
        let mut text = None;
        for _ in 0..5 {
            text = run_tesseract(&bin, b"fake image bytes").await;
            if text.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(text.unwrap().trim(), "INVOICE 110148792 Boss Shop");
    }

    #[tokio::test]
    async fn test_run_tesseract_nonzero_exit_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let bin = fake_bin(dir.path(), "cat >/dev/null\nexit 1\n");
        assert!(run_tesseract(&bin, b"fake").await.is_none());
    }

    #[tokio::test]
    async fn test_run_tesseract_empty_output_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let bin = fake_bin(dir.path(), "cat >/dev/null\nprintf '  \\n'\n");
        assert!(run_tesseract(&bin, b"fake").await.is_none());
    }
}
