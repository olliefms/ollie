---
name: verify
description: Build, launch, and drive a local ollie server end-to-end to verify a change against real Ollama. Use when verifying blob-pipeline, fleet-API, or MCP changes by observation rather than tests.
---

# Verify ollie changes end-to-end

## Build + launch

Debug binary is fine (`cargo build -j 2` — cap jobs; 10 parallel LanceDB test-binary
links exhaust a 16 GB box). Launch against a throwaway data dir:

```bash
env DRIVER_JWT_SECRET="$(openssl rand -hex 16)" \
    FLEET_JWT_SECRET="$(openssl rand -hex 16)" \
    DRIVER_RP_ID=localhost DRIVER_RP_ORIGIN=http://localhost:3999 \
    OLLAMA_BASE_URL=http://<ollama-host>:11434 \
    OLLAMA_EMBED_MODEL=nomic-embed-text OLLAMA_SUMMARY_MODEL=llama3.2 \
    OLLAMA_VISION_MODEL=moondream OLLAMA_EMBED_DIM=768 \
    DATA_DIR=/tmp/verify-data PORT=3999 RUST_LOG=ollie=info \
    ./target/debug/ollie
```

If your Ollama runs in a docker container without a published port, find its IP with
`docker inspect <container> --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}'`.
Readiness: poll `GET /version`.

## Auth

First user: `POST /fleet/setup` `{"email","name","password"}` → `{"token"}`.
Later runs against the same DATA_DIR: `POST /fleet/auth/login` with the same creds.

## Drive

- Upload: `curl -X POST :3999/fleet/api/v1/blobs -H "Authorization: Bearer $TOKEN" -F "file=@f.pdf;type=application/pdf"`
- Blob metadata: GET `/fleet/api/v1/blob/{id}` **with `Accept: application/json`** —
  without it you get the raw file bytes.
- MCP (`POST /fleet/mcp`) is stateful: send `initialize`, capture the
  `Mcp-Session-Id` response header, send `notifications/initialized`, then
  `tools/call` — all with `Accept: application/json, text/event-stream` and the
  session header. Responses are SSE `data:` lines.
- Pipeline outcomes: `RUST_LOG=ollie=info` logs `pipeline completed for <id>
  (summary source: text|ocr|vision|pdf_text|preserved|none)`.

## Gotchas

- No tesseract on the host? Wrapper script works:
  `#!/bin/sh\nexec docker run --rm -i tesseractshadow/tesseract4re tesseract "$@"`
  then `OLLIE_TESSERACT_BIN=<wrapper>`. Point it at /nonexistent to force the
  vision fallback.
- Identical bytes dedupe by checksum (201 + copied summary) — vary the file if you
  need a fresh pipeline run, or use MCP `resummarize_blob` to re-queue.
- moondream reads photos, not documents: expect hallucinated text if a document
  scan reaches the vision fallback. That is a model limitation, not a pipeline bug.
