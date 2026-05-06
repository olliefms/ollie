# ollie

A RAG-enabled blob store. Upload files; get back semantic search.

Files are stored content-addressed on disk. Ollama generates summaries and embeddings in the background. LanceDB powers vector search.

## Prerequisites

- [Rust](https://rustup.rs/) 1.78+ (for building from source)
- [Ollama](https://ollama.ai/) running locally (for AI features)
- Docker + Docker Compose (for the containerized setup)

### Ollama models

Pull the required models before starting:

```bash
ollama pull nomic-embed-text
ollama pull llama3.2
ollama pull llava
```

## Quick Start with Docker Compose

```bash
# Clone the repo
git clone https://github.com/ergofobe/ollie.git
cd ollie

# Set your API key
echo "ADMIN_API_KEY=your-secret-key" > .env

# Start
docker compose up
```

The API is available at `http://localhost:3000`.

## Building from Source

```bash
git clone https://github.com/ergofobe/ollie.git
cd ollie

# Create .env
cat > .env <<EOF
ADMIN_API_KEY=your-secret-key
DATA_DIR=./data
EOF

cargo build --release
./target/release/ollie
```

## Configuration

All configuration is via environment variables (or `.env` file):

| Variable | Required | Default | Description |
|---|---|---|---|
| `ADMIN_API_KEY` | **Yes** | — | Bearer token for all API requests |
| `PORT` | No | `3000` | HTTP listen port |
| `DATA_DIR` | No | `./data` | Storage root (blobs + LanceDB) |
| `OLLAMA_BASE_URL` | No | `http://localhost:11434` | Ollama API base URL |
| `OLLAMA_EMBED_MODEL` | No | `nomic-embed-text` | Embedding model name |
| `OLLAMA_SUMMARY_MODEL` | No | `llama3.2` | Summarization model name |
| `OLLAMA_VISION_MODEL` | No | `llava` | Vision model name (PDFs, images) |
| `OLLAMA_EMBED_DIM` | No | `768` | Embedding dimension (must match model) |
| `PIPELINE_WORKERS` | No | `1` | Concurrent AI processing workers |

## API Reference

All requests require `Authorization: Bearer <ADMIN_API_KEY>`.

---

### Upload a blob

```
POST /api/v1/blobs
Content-Type: multipart/form-data
```

**Fields:**
- `file` (required) — the file to upload
- `name` (optional) — display name; defaults to the original filename
- `tags` (optional, repeatable) — arbitrary string tags

**Returns:**
- `202 Accepted` — new file, queued for AI processing
- `201 Created` — duplicate (same content already stored; summary/embedding copied)

```bash
# Upload a file
curl -X POST http://localhost:3000/api/v1/blobs \
  -H "Authorization: Bearer your-secret-key" \
  -F "file=@/path/to/document.pdf" \
  -F "name=My Document" \
  -F "tags=research" \
  -F "tags=2024"
```

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "owner_id": 0,
  "checksum": "abc123...",
  "name": "My Document",
  "mime_type": "application/pdf",
  "size": 204800,
  "status": "pending",
  "tags": "research,2024",
  "created_at": "2026-05-06T12:00:00Z",
  "updated_at": "2026-05-06T12:00:00Z"
}
```

---

### List blobs

```
GET /api/v1/blobs
```

**Query parameters:**
- `q` — semantic search query (uses vector search)
- `tag` — filter by tag (repeatable: `?tag=research&tag=2024`)
- `limit` — max results (default: 20)
- `offset` — pagination offset (ignored when `q` is set)

```bash
# List all blobs
curl http://localhost:3000/api/v1/blobs \
  -H "Authorization: Bearer your-secret-key"

# Semantic search
curl "http://localhost:3000/api/v1/blobs?q=machine+learning" \
  -H "Authorization: Bearer your-secret-key"

# Filter by tag
curl "http://localhost:3000/api/v1/blobs?tag=research&tag=2024" \
  -H "Authorization: Bearer your-secret-key"
```

```json
{
  "items": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "name": "My Document",
      "mime_type": "application/pdf",
      "size": 204800,
      "status": "ready",
      "tags": "research,2024",
      "created_at": "2026-05-06T12:00:00Z"
    }
  ],
  "total": 1,
  "limit": 20,
  "offset": 0
}
```

---

### Get a blob

```
GET /api/v1/blobs/:id
```

Returns JSON metadata or the raw file bytes depending on the `Accept` header:

```bash
# Get metadata
curl http://localhost:3000/api/v1/blobs/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer your-secret-key" \
  -H "Accept: application/json"

# Download file
curl http://localhost:3000/api/v1/blobs/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer your-secret-key" \
  -H "Accept: */*" \
  -o downloaded.pdf
```

---

### Update a blob

```
PUT /api/v1/blobs/:id
Content-Type: application/json
```

Updates `name` and/or `tags`. At least one field required.

```bash
curl -X PUT http://localhost:3000/api/v1/blobs/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer your-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"name": "Updated Name", "tags": "new-tag,another-tag"}'
```

---

### Delete a blob

```
DELETE /api/v1/blobs/:id
```

Deletes the record and the file (if no other blobs share the same content).

```bash
curl -X DELETE http://localhost:3000/api/v1/blobs/550e8400-e29b-41d4-a716-446655440000 \
  -H "Authorization: Bearer your-secret-key"
```

Returns `204 No Content`.

---

## Blob Status

Blobs move through these states:

| Status | Meaning |
|---|---|
| `pending` | Queued for AI processing |
| `processing` | Actively being summarized/embedded |
| `ready` | Summary and embedding complete |
| `failed` | AI processing failed (check `error` field) |

## Content Handling

| File type | Processing |
|---|---|
| Text, JSON, XML | Extracted and summarized as text |
| PDF (≥50 words) | Text extracted, summarized as text |
| PDF (<50 words) | Rendered as image, described by vision model |
| Images | Described by vision model |
| Other | Stored but not processed (status stays `pending` if Ollama is unavailable) |

## Development

```bash
# Run tests
cargo test

# Check for issues
cargo clippy

# Build
cargo build
```

See [AGENTS.md](AGENTS.md) for developer and agent-specific guidance.
