# ollie

A self-hosted freight **Transportation Management System (TMS)** written in Rust.

ollie runs the operational core of a trucking dispatch operation — loads, trips,
drivers, trucks, trailers, and facilities — and pairs it with an AI-enabled
document store. It exposes four API surfaces (an MCP server for AI agents, a
Fleet REST API, a driver mobile portal, and a legacy admin API) plus two
bundled web apps: a Fleet SPA and a driver PWA.

Uploaded documents (rate cons, BOLs, PODs, photos) are stored content-addressed
on disk and processed in the background by Ollama — text summaries and vector
embeddings for semantic search, with images and scanned PDFs routed through a
vision model. Vector search is powered by LanceDB.

## What's in the box

| Domain | Description |
|---|---|
| **Loads** | Freight load lifecycle: `planned → assigned → dispatched → in_transit → delivered → invoiced → settled`, with stops, rate line items, and detention tracking. |
| **Trips** | Driver-facing execution of loads, with stop arrive/depart, check-calls, relay chaining, deadhead/loaded mileage (ORS HGV routing), and a full state machine. |
| **Drivers / Trucks / Trailers** | Fleet entities with status state machines and equipment attach/detach. |
| **Facilities** | Shippers/receivers, geocoded (US Census) with semantic dedup and search. |
| **Documents (blobs)** | Content-addressed, deduplicated file storage with Ollama summaries, embeddings, and natural-language Q&A over a document. |
| **Events** | Append-only journal of operational events. |
| **Doctors** | Diagnose-and-repair tools for trip / load / facility data integrity. |

## API surfaces

ollie exposes the same domain through four surfaces — pick by caller:

| Surface | Path | Auth | Use |
|---|---|---|---|
| **Fleet MCP** | `POST /fleet/mcp` | Fleet user JWT or API key | AI agents and tool-using assistants. **Preferred.** |
| **Fleet REST** | `/fleet/api/v1/*` | Fleet user JWT or API key | Fleet web app and programmatic integrations. |
| **Driver portal** | `/driver/api/v1/*` | Driver JWT (passkey or PIN) | The driver mobile PWA only. |
| **Admin REST** | `/api/v1/*` | `ADMIN_API_KEY` bearer | **Deprecated** — backward compatibility only. |

Three endpoints are public (no auth): `GET /version`, `GET /openapi.json`, and
`GET /llms.txt`. Start with `GET /llms.txt` — it's a hand-written, agent-oriented
tour of every surface, tool, and the domain model.

The web apps are served as static SPAs: the fleet app at `/fleet` and the
driver PWA at `/driver`.

## Prerequisites

- [Rust](https://rustup.rs/) 1.78+ (for building from source)
- [Ollama](https://ollama.ai/) running locally (for document AI features)
- Docker + Docker Compose (for the containerized setup)
- An [OpenRouteService](https://openrouteservice.org/) API key (for trip/load mileage)

### Ollama models

Pull the document-processing models before starting:

```bash
ollama pull nomic-embed-text   # embeddings
ollama pull llama3.2           # text summaries
ollama pull moondream          # vision (images, scanned PDFs)
```

## Quick start with Docker Compose

```bash
git clone https://github.com/ergofobe/ollie.git
cd ollie

# Copy the example env and fill in the required secrets
cp .env.example .env
$EDITOR .env

docker compose up
```

The server listens on `http://localhost:3000`. The Compose file also starts an
Ollama container and pulls the required models on first boot.

## Building from source

```bash
git clone https://github.com/ergofobe/ollie.git
cd ollie

cp .env.example .env
$EDITOR .env

cargo build --release
./target/release/ollie
```

## Configuration

All configuration is via environment variables (or a `.env` file).

### Required

| Variable | Description |
|---|---|
| `ADMIN_API_KEY` | Bearer token for the deprecated admin REST API. Any non-empty string. |
| `DRIVER_JWT_SECRET` | Signing secret for driver-portal JWTs. **Min 32 bytes.** |
| `FLEET_JWT_SECRET` | Signing secret for fleet user JWTs and API keys. **Min 32 bytes.** |
| `DRIVER_RP_ID` | WebAuthn relying-party ID for driver passkeys (e.g. `localhost`). |
| `DRIVER_RP_ORIGIN` | WebAuthn relying-party origin (e.g. `http://localhost:3000`). |

### Recommended

| Variable | Default | Description |
|---|---|---|
| `ORS_API_KEY` | — | OpenRouteService key. Without it, trip/load mileage is not computed. |
| `OLLIE_PUBLIC_BASE_URL` | empty | Externally-reachable base URL (no trailing slash). Required for presigned blob upload/download URLs handed to MCP agents. |

### Optional

| Variable | Default | Description |
|---|---|---|
| `PORT` | `3000` | HTTP listen port |
| `DATA_DIR` | `./data` | Storage root (blobs, extracts, LanceDB) |
| `BLOB_STORE_PATH` | `./data/blobs` | Override blob storage path |
| `EXTRACT_STORE_PATH` | `./data/extracts` | Override extracted-text cache path |
| `LANCEDB_PATH` | `./data/lancedb` | Override LanceDB path |
| `OLLAMA_BASE_URL` | `http://localhost:11434` | Ollama API base URL |
| `OLLAMA_EMBED_MODEL` | `nomic-embed-text` | Embedding model |
| `OLLAMA_SUMMARY_MODEL` | `llama3.2` | Text summarization model |
| `OLLAMA_VISION_MODEL` | `moondream` | Vision model (images, scanned PDFs) |
| `OLLAMA_EMBED_DIM` | `768` | Embedding dimension (must match the embed model) |
| `PIPELINE_WORKERS` | `1` | Concurrent document-processing workers |
| `GEOCODING_WORKERS` | `1` | Concurrent facility-geocoding workers |
| `TERMINAL_TIMEZONE` | `America/New_York` | IANA timezone for driver pay-period weeks |
| `OLLIE_FREE_DWELL_MINUTES` | `120` | Free dwell before detention accrues at a stop |
| `OLLIE_BLOB_PRESIGN_TTL_SECS` | `300` | Default TTL for presigned blob URLs |
| `OLLIE_BLOB_PRESIGN_MAX_TTL_SECS` | `3600` | Hard cap on presigned blob URL TTL |
| `FACILITY_DEDUP_HIGH_THRESHOLD` | `0.92` | Cosine score above which a facility is an exact match |
| `FACILITY_DEDUP_LOW_THRESHOLD` | `0.75` | Cosine score above which a facility is a candidate match |

## Using the API

The fastest way to explore is the machine-readable spec and the LLM tour:

```bash
# Public — no auth required
curl http://localhost:3000/llms.txt         # agent-oriented overview of every surface
curl http://localhost:3000/openapi.json      # full OpenAPI 3.0 spec
curl http://localhost:3000/version           # { "version": "x.y.z" }
```

### Fleet auth

```bash
# Log in with email + password to get a JWT
curl -X POST http://localhost:3000/fleet/auth/login \
  -H "Content-Type: application/json" \
  -d '{"email": "you@example.com", "password": "..."}'

# Use the returned token against MCP or REST
curl -X POST http://localhost:3000/fleet/mcp \
  -H "Authorization: Bearer <JWT>" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

Headless callers can mint long-lived **fleet user API keys** under
`/fleet/api-keys` (used in the `Authorization` header just like a JWT). See
`/llms.txt` for the full MCP tool list, REST routes, auth model, and domain
semantics (load/trip lifecycles, stop times and detention, facility resolution).

## Document processing

Uploaded documents move through these states:

| Status | Meaning |
|---|---|
| `pending` | Queued for AI processing |
| `processing` | Actively being summarized/embedded |
| `ready` | Summary and embedding complete |
| `failed` | AI processing failed (check the `error` field) |

| File type | Processing |
|---|---|
| Text, JSON, XML | Extracted and summarized as text |
| PDF (≥50 words) | Text extracted, summarized as text |
| PDF (<50 words) | Rendered as image, described by the vision model |
| Images | Described by the vision model |
| Other | Stored but not AI-processed |

Files are content-addressed (SHA-256) and deduplicated — identical bytes share
storage and AI output.

## Development

```bash
cargo test       # unit + integration tests
cargo clippy     # lint
cargo build      # build
```

The integration tests do not require Ollama. See [AGENTS.md](AGENTS.md) for
developer and AI-agent guidance, and [docs/DESIGN.md](docs/DESIGN.md) for the UI
design system.
