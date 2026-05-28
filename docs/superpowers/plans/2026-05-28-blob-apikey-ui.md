# Blob Delete Race + Dispatcher Blob Upload UI + API Key UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close #280 (TOCTOU data-loss race in HTTP blob delete handlers), #186 (dispatcher blob upload UI with visible-to-driver checkbox), and #241 (dispatcher API-key management UI).

**Architecture:** #280 is a backend correctness fix that mirrors the already-shipped MCP `tool_delete_blob` ordering (delete DB row → recount checksum → delete bytes only when 0) in the two remaining HTTP handlers. #186 and #241 are pure frontend additions to the dispatcher SPA (`static/dispatch/app.js` + `index.html`); both touch the same two files, so they are executed as **one sequential frontend track** to avoid same-file merge conflicts. All backend endpoints for #186/#241 already exist (`POST /dispatch/api/v1/blobs` accepts `visibility`; `POST/GET/DELETE /dispatch/api-keys`).

**Tech Stack:** Rust / Axum 0.7 / LanceDB 0.29 (backend); vanilla JS hash-routed SPA (frontend). Tests: `cargo test` integration tests in `tests/integration_test.rs`.

---

## Why batched (sprint justification)

These three are **not** strictly interdependent. They are batched per explicit user instruction into one isolated worktree + one PR to keep all three off the in-flight `deprecate-create-blob` worktree's path (which is rewriting `dispatcher_portal/mcp.rs`, `mod.rs`, `config.rs`, `tests/integration_test.rs`). This sprint deliberately touches **none** of `mcp.rs`, `mod.rs`, or `config.rs`. The only shared file with that worktree is `tests/integration_test.rs` (append-only new tests) — resolve any merge by keeping both blocks.

---

## File Structure

- `src/api/blob.rs` (modify) — admin `delete_blob` handler reorder (#280)
- `src/api/dispatcher_portal/blobs.rs` (modify) — dispatcher `delete_blob` handler reorder (#280)
- `tests/integration_test.rs` (modify, append) — one integration test per handler (#280)
- `static/dispatch/app.js` (modify) — FormData-aware `apiFetch`; upload UI in documents view (#186); `account` view + API-key management (#241)
- `static/dispatch/index.html` (modify) — add `Account` sidebar link; bump `?v=` cache stamp on `app.js` (#186/#241)

---

## Task 1: Fix TOCTOU race in admin HTTP blob delete (#280)

**Files:**
- Modify: `src/api/blob.rs:113-135` (`delete_blob`)
- Test: `tests/integration_test.rs` (append)

The current handler deletes storage bytes *before* the DB row, with a `count_by_checksum` check that races a concurrent dedup upload. Reorder to mirror `tool_delete_blob` in `src/api/dispatcher_portal/mcp.rs:1554-1562`: delete the DB row first, recount by checksum, delete bytes only when the recount is `0`.

- [ ] **Step 1: Write the failing test**

Append to `tests/integration_test.rs`. This test creates two blob records sharing one checksum (via dedup upload of identical bytes), deletes one, and asserts the other's bytes survive (download returns 200). Use the existing test harness helpers — match the patterns already in the file (`test_server()` returning `(server, _db_dir, _blob_dir, _rx)`; `axum-test` responses use `.as_bytes()`; uploads are multipart).

```rust
#[tokio::test]
async fn test_admin_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _db_dir, _blob_dir, _rx) = test_server().await;

    // Upload identical bytes twice → two records, one checksum (dedup).
    let bytes = b"shared-content-for-delete-race-admin";
    let first = upload_bytes(&server, bytes, "first.txt").await;
    let second = upload_bytes(&server, bytes, "second.txt").await;
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    assert_ne!(first_id, second_id, "dedup must create a distinct record");

    // Delete the first record.
    let del = server.delete(&format!("/api/v1/blob/{first_id}")).await;
    del.assert_status(StatusCode::NO_CONTENT);

    // The second record's bytes must still be downloadable.
    let got = server
        .get(&format!("/api/v1/blob/{second_id}"))
        .await;
    got.assert_status_ok();
    assert_eq!(got.as_bytes(), &bytes[..], "shared bytes must survive sibling delete");
}
```

> **Note on helpers:** If `upload_bytes` does not already exist in `tests/integration_test.rs`, add a small helper near the other helpers that POSTs a multipart `file` field with the given name to `/api/v1/blobs` with the admin bearer token and returns the parsed JSON body. Reuse the exact multipart construction already used by existing upload tests in the file — do not invent a new pattern. If an equivalent helper exists under a different name, use it instead.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml --test integration_test test_admin_delete_blob_keeps_bytes_when_checksum_shared -- --nocapture`

Expected: FAIL — under the current ordering the bytes are deleted while the sibling record still references them, so the second download returns 404/500 (NotFound). (If dedup timing happens to mask it, the test still encodes the correct invariant and must pass after the fix.)

- [ ] **Step 3: Apply the reorder**

Replace the body of `delete_blob` in `src/api/blob.rs` (the `ref_count`/`delete` block at lines ~125-133) with delete-row-first ordering:

```rust
    let record = state.db.get_by_id(id).await?;

    if state.db.any_load_references_blob(id).await? {
        return Err(AppError::Conflict(
            "blob is referenced by one or more loads and cannot be deleted".into(),
        ));
    }

    // Delete the DB record FIRST, then re-count by checksum. LanceDB has no
    // transactions; this ordering is what makes concurrent delete-vs-upload safe.
    // A concurrent dedup ingest (storage write is a no-op) inserts another record
    // for the same checksum; the post-delete recount sees it and keeps the bytes.
    // Deleting the row before the bytes also means a mid-operation failure orphans
    // a file (recoverable) rather than leaving a record pointing at deleted bytes.
    state.db.delete_by_id(id).await?;
    let remaining = state.db.count_by_checksum(&record.checksum).await?;
    if remaining == 0 {
        state.store.delete(&record.checksum).await?;
        let extract_base = std::path::Path::new(&state.config.extract_store_path);
        if let Err(e) = delete_extract(extract_base, &record.checksum).await {
            tracing::warn!("failed to delete extract cache for {}: {e}", record.checksum);
        }
    }
    Ok(StatusCode::NO_CONTENT)
```

(`delete_extract` is already imported in `src/api/blob.rs`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml --test integration_test test_admin_delete_blob_keeps_bytes_when_checksum_shared`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/api/blob.rs tests/integration_test.rs
git commit -m "fix(blobs): reorder admin blob delete to close TOCTOU data-loss race (#280)"
```

---

## Task 2: Fix TOCTOU race in dispatcher HTTP blob delete (#280)

**Files:**
- Modify: `src/api/dispatcher_portal/blobs.rs:245-267` (`delete_blob`)
- Test: `tests/integration_test.rs` (append)

Same fix as Task 1, applied to the dispatcher handler. The dispatcher handler is identical in structure (lines 257-265). Use the dispatcher portal's JWT-auth test pattern already present in `tests/integration_test.rs` for the test.

- [ ] **Step 1: Write the failing test**

Append to `tests/integration_test.rs`. Mirror the dispatcher-portal test setup already in the file (obtain a dispatcher JWT, upload via `/dispatch/api/v1/blobs`, etc.). If the file already has dispatcher blob tests, copy their auth/upload helper usage exactly.

```rust
#[tokio::test]
async fn test_dispatcher_delete_blob_keeps_bytes_when_checksum_shared() {
    let (server, _db_dir, _blob_dir, _rx) = test_server().await;
    let token = dispatcher_login_token(&server).await; // use existing helper name

    let bytes = b"shared-content-for-delete-race-dispatch";
    let first = dispatch_upload_bytes(&server, &token, bytes, "first.txt").await;
    let second = dispatch_upload_bytes(&server, &token, bytes, "second.txt").await;
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    assert_ne!(first_id, second_id);

    let del = server
        .delete(&format!("/dispatch/api/v1/blob/{first_id}"))
        .authorization_bearer(&token)
        .await;
    del.assert_status(StatusCode::NO_CONTENT);

    let got = server
        .get(&format!("/dispatch/api/v1/blob/{second_id}"))
        .authorization_bearer(&token)
        .await;
    got.assert_status_ok();
    assert_eq!(got.as_bytes(), &bytes[..]);
}
```

> **Note on helpers:** Use whatever dispatcher-login and dispatcher-upload helpers already exist in `tests/integration_test.rs` (names above are illustrative). If none exist, add minimal helpers mirroring the admin ones but targeting `/dispatch/auth/login` for a token and `/dispatch/api/v1/blobs` for upload. Grep the test file first: `grep -n "dispatch" tests/integration_test.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml --test integration_test test_dispatcher_delete_blob_keeps_bytes_when_checksum_shared`
Expected: FAIL (NotFound on second download) before the fix.

- [ ] **Step 3: Apply the reorder**

Replace the `ref_count`/`delete` block in `src/api/dispatcher_portal/blobs.rs` `delete_blob` (lines ~257-265) with the same delete-row-first ordering as Task 1 Step 3 (identical code; `delete_extract` is already imported in `blobs.rs`):

```rust
    state.db.delete_by_id(id).await?;
    let remaining = state.db.count_by_checksum(&record.checksum).await?;
    if remaining == 0 {
        state.store.delete(&record.checksum).await?;
        let extract_base = std::path::Path::new(&state.config.extract_store_path);
        if let Err(e) = delete_extract(extract_base, &record.checksum).await {
            tracing::warn!("failed to delete extract cache for {}: {e}", record.checksum);
        }
    }
    Ok(StatusCode::NO_CONTENT)
```

Keep the existing `get_by_id` + `any_load_references_blob` guard above it unchanged.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml --test integration_test test_dispatcher_delete_blob_keeps_bytes_when_checksum_shared`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/api/dispatcher_portal/blobs.rs tests/integration_test.rs
git commit -m "fix(blobs): reorder dispatcher blob delete to close TOCTOU data-loss race (#280)"
```

---

## Task 3: FormData-aware apiFetch + dispatcher blob upload UI (#186)

**Files:**
- Modify: `static/dispatch/app.js:66-83` (`apiFetch`), `static/dispatch/app.js:1110-1194` (`renderDocumentsView`)

The SPA's `apiFetch` hardcodes `'Content-Type': 'application/json'`, which breaks multipart uploads (the browser must set its own `multipart/form-data; boundary=…`). First make `apiFetch` skip the JSON content-type when the body is `FormData`, then add an upload form to the documents view.

- [ ] **Step 1: Make `apiFetch` FormData-aware**

Replace `apiFetch` in `static/dispatch/app.js` (lines 66-83) with:

```js
async function apiFetch(path, options = {}) {
  const token = getToken();
  const isFormData = options.body instanceof FormData;
  const headers = {
    ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(options.headers || {}),
  };

  const res = await fetch(path, { ...options, headers });

  if (res.status === 401) {
    clearToken();
    showLogin();
    throw new Error('Unauthorized — please sign in again.');
  }

  return res;
}
```

- [ ] **Step 2: Add the upload form to the documents view**

In `renderDocumentsView`, extend `filterHtml` (lines 1125-1131) to include an upload control row beneath the filter row. Replace the `filterHtml` template literal with:

```js
    const filterHtml = `
      <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3);align-items:center;flex-wrap:wrap;">
        <input class="form-input" id="doc-filter-name" type="text"
          placeholder="Filter by name…" value="${escHtml(filterName)}" style="max-width:240px;">
        <button class="btn btn--secondary" id="doc-filter-apply">Search</button>
        <span style="flex:1;"></span>
        <input type="file" id="doc-upload-file" style="display:none;">
        <label style="display:flex;gap:var(--space-1);align-items:center;font-size:var(--text-sm);">
          <input type="checkbox" id="doc-upload-visible-driver"> Visible to driver
        </label>
        <button class="btn btn--primary" id="doc-upload-btn">+ Upload</button>
      </div>
      <div id="doc-upload-status" class="alert" hidden style="margin-bottom:var(--space-3);"></div>
    `;
```

- [ ] **Step 3: Wire the upload handler**

After the existing event-listener wiring in `renderDocumentsView` (after the `.doc-row` forEach, before the `catch`), add:

```js
    const fileInput = document.getElementById('doc-upload-file');
    const uploadBtn = document.getElementById('doc-upload-btn');
    const statusEl = document.getElementById('doc-upload-status');

    uploadBtn?.addEventListener('click', () => fileInput?.click());

    fileInput?.addEventListener('change', async () => {
      const file = fileInput.files && fileInput.files[0];
      if (!file) return;

      const visibleToDriver = document.getElementById('doc-upload-visible-driver')?.checked;
      const fd = new FormData();
      fd.append('file', file);
      if (visibleToDriver) fd.append('visibility', 'driver');

      statusEl.hidden = false;
      statusEl.className = 'alert';
      statusEl.textContent = `Uploading ${file.name}…`;
      uploadBtn.disabled = true;

      try {
        const res = await apiFetch(`${API_BASE}/blobs`, { method: 'POST', body: fd });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        statusEl.className = 'alert alert--info';
        statusEl.textContent = `Uploaded ${file.name}.`;
        navigate('documents', { name: filterName });
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          statusEl.className = 'alert alert--error';
          statusEl.textContent = `Upload failed: ${err.message}`;
        }
      } finally {
        uploadBtn.disabled = false;
        fileInput.value = '';
      }
    });
```

> **Note:** `'driver'` must match the `BlobVisibility` serde value accepted by the backend. Verify with `grep -n "BlobVisibility" src/models*.rs src/models/*.rs` and confirm the `driver` variant deserializes from the lowercase string `"driver"` (the upload handler parses via `raw.parse::<BlobVisibility>()`). If the accepted token differs, use the correct one.

- [ ] **Step 4: Manual verification**

Backend has no automated frontend test harness. Verify by reading the diff and confirming: (a) `apiFetch` no longer forces JSON content-type for FormData; (b) the upload form posts to `${API_BASE}/blobs`; (c) the checkbox appends `visibility=driver`; (d) the list refreshes via `navigate('documents', …)` on success. CSS classes used here (`alert`, `alert--info`, `alert--error`, `btn--primary`, `btn--secondary`, `form-input`) all exist in `static/dispatch/css/components.css` — confirmed. No new CSS is introduced, so the `?v=` CSS stamps in `index.html` stay unchanged.

- [ ] **Step 5: Commit**

```bash
git add static/dispatch/app.js
git commit -m "feat(dispatch-ui): blob upload form with visible-to-driver checkbox (#186)"
```

---

## Task 4: Dispatcher API-key management UI (#241)

**Files:**
- Modify: `static/dispatch/app.js` (add `account` to `VIEW_TITLES`, a case in `_renderView`, and a `renderAccountView` function), `static/dispatch/index.html` (add Account sidebar link; bump `?v=` stamp)

Backend endpoints already exist: `POST /dispatch/api-keys` (create, returns plaintext `key` once), `GET /dispatch/api-keys` (list active), `DELETE /dispatch/api-keys/:id` (revoke). Note these are mounted at `/dispatch/api-keys`, **not** under `API_BASE` (`/dispatch/api/v1`). Use a separate constant.

- [ ] **Step 1: Add the Account sidebar link in index.html**

In `static/dispatch/index.html`, add after the Documents sidebar button (line 70-72):

```html
        <button class="sidebar__link" data-view="account">
          <span>Account</span>
        </button>
```

- [ ] **Step 2: Bump the cache-bust stamp in index.html**

The `?v=` stamps are stale (`1.7.0`). Bump the `app.js` stamp so returning dispatchers load the new JS. Change line 95:

```html
  <script src="/dispatch/app.js?v=1.20.2"></script>
```

(Leave the CSS stamps unless a CSS file is edited in this sprint — none is.)

- [ ] **Step 3: Register the account view in app.js**

Add to `VIEW_TITLES` (after `document: 'Document',` at line 110):

```js
  account: 'Account',
```

Add a case in `_renderView`'s switch (after the `document` case, ~line 208):

```js
    case 'account':
      renderAccountView();
      break;
```

- [ ] **Step 4: Add the `renderAccountView` function**

Add a new function (place it near `renderDocumentsView`, and add an `API_KEYS_BASE` constant near `API_BASE` at line 8: `const API_KEYS_BASE = '/dispatch/api-keys';`):

```js
async function renderAccountView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(API_KEYS_BASE);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const keys = data.keys || [];

    const createHtml = `
      <div style="margin-bottom:var(--space-4);padding:var(--space-3);background:var(--color-surface-2);border-radius:var(--radius-sm);">
        <h3 style="margin-top:0;">Create API key</h3>
        <div style="display:flex;gap:var(--space-2);align-items:flex-end;flex-wrap:wrap;">
          <div class="form-group" style="margin:0;">
            <label class="form-label" for="ak-label">Label</label>
            <input class="form-input" id="ak-label" type="text" maxlength="64" placeholder="e.g. Claude MCP connector" style="max-width:260px;">
          </div>
          <div class="form-group" style="margin:0;">
            <label class="form-label" for="ak-expires">Expires in (days, 1–365)</label>
            <input class="form-input" id="ak-expires" type="number" min="1" max="365" value="365" style="max-width:160px;">
          </div>
          <button class="btn btn--primary" id="ak-create-btn">Create key</button>
        </div>
        <div id="ak-create-status" class="alert" hidden style="margin-top:var(--space-3);"></div>
      </div>
    `;

    let listHtml;
    if (keys.length === 0) {
      listHtml = `
        <div class="state-empty">
          No API keys yet. Create one above to connect Claude's remote MCP connector.
          <pre style="text-align:left;overflow:auto;margin-top:var(--space-3);padding:var(--space-2);background:var(--color-surface-2);border-radius:var(--radius-sm);">{
  "mcpServers": {
    "ollie": {
      "url": "https://YOUR_HOST/dispatch/mcp",
      "headers": { "Authorization": "Bearer YOUR_API_KEY" }
    }
  }
}</pre>
        </div>`;
    } else {
      const rows = keys.map(k => `
        <tr>
          <td>${escHtml(k.label)}</td>
          <td style="font-family:monospace;">${escHtml(k.key_prefix)}…</td>
          <td>${fmtDate(k.created_at)}</td>
          <td>${fmtDate(k.expires_at)}</td>
          <td>${k.last_used_at ? fmtDate(k.last_used_at) : '—'}</td>
          <td><button class="btn btn--secondary ak-revoke" data-key-id="${k.id}">Revoke</button></td>
        </tr>
      `).join('');
      listHtml = `
        <div class="table-wrapper">
          <table class="data-table">
            <thead><tr><th>Label</th><th>Prefix</th><th>Created</th><th>Expires</th><th>Last used</th><th></th></tr></thead>
            <tbody>${rows}</tbody>
          </table>
        </div>`;
    }

    setContent(createHtml + listHtml);

    document.getElementById('ak-create-btn')?.addEventListener('click', async () => {
      const label = document.getElementById('ak-label').value.trim();
      const expires = parseInt(document.getElementById('ak-expires').value, 10);
      const statusEl = document.getElementById('ak-create-status');
      if (!label) {
        statusEl.hidden = false;
        statusEl.className = 'alert alert--error';
        statusEl.textContent = 'Label is required.';
        return;
      }
      try {
        const r = await apiFetch(API_KEYS_BASE, {
          method: 'POST',
          body: JSON.stringify({ label, expires_in_days: expires }),
        });
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const created = await r.json();
        statusEl.hidden = false;
        statusEl.className = 'alert alert--info';
        statusEl.innerHTML = `Key created. Copy it now — it cannot be shown again:<br>
          <code style="word-break:break-all;">${escHtml(created.key)}</code>
          <button class="btn btn--secondary" id="ak-copy-btn" style="margin-top:var(--space-2);">Copy</button>`;
        document.getElementById('ak-copy-btn')?.addEventListener('click', () => {
          navigator.clipboard?.writeText(created.key);
        });
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          statusEl.hidden = false;
          statusEl.className = 'alert alert--error';
          statusEl.textContent = `Create failed: ${err.message}`;
        }
      }
    });

    document.querySelectorAll('.ak-revoke').forEach(btn => {
      btn.addEventListener('click', async () => {
        if (!confirm('Revoke this API key? Integrations using it will stop working immediately.')) return;
        try {
          const r = await apiFetch(`${API_KEYS_BASE}/${btn.dataset.keyId}`, { method: 'DELETE' });
          if (!r.ok && r.status !== 204) throw new Error(`HTTP ${r.status}`);
          navigate('account');
        } catch (err) {
          if (err.message !== 'Unauthorized — please sign in again.') {
            alert(`Revoke failed: ${err.message}`);
          }
        }
      });
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load API keys: ${err.message}</div>`);
    }
  }
}
```

> **Note on CSS classes (pinned):** This task uses ONLY classes/tokens confirmed to exist in `static/dispatch/css/components.css` + `base.css`: `alert`, `alert--info`, `alert--error`, `btn`, `btn--primary`, `btn--secondary`, `form-input`, `form-label`, `form-group`, `data-table`, `table-wrapper`, `state-empty`, `state-error`, and tokens `--color-surface-2`, `--radius-sm`, `--space-*`, `--text-sm`. No new CSS class or token is introduced — do NOT add `card`, `btn--danger`, `btn--sm`, or `alert--success`, and do NOT introduce raw hex values. Because no CSS file is edited, the `?v=` CSS stamps in `index.html` stay unchanged.

- [ ] **Step 5: Manual verification**

Read the diff and confirm: (a) `account` is in `VIEW_TITLES`, the switch, and has a sidebar link; (b) create posts JSON to `/dispatch/api-keys` and renders the plaintext `key` once with a copy button; (c) revoke DELETEs `/dispatch/api-keys/:id` with a confirm and refreshes; (d) empty state shows the MCP `.mcp.json` snippet; (e) all CSS classes resolved to existing tokens.

- [ ] **Step 6: Commit**

```bash
git add static/dispatch/app.js static/dispatch/index.html
git commit -m "feat(dispatch-ui): API key management page (#241)"
```

---

## Task 5: Full verification + PR

- [ ] **Step 1: Run the whole suite**

Run: `cargo test --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml`
Expected: all pass (config-from-env flakiness in parallel is a known false negative — re-run in isolation if it trips).

- [ ] **Step 2: Clippy + build**

Run: `cargo clippy --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml --all-targets` and `cargo build --manifest-path /Users/jimp7508/src/ollie/.claude/worktrees/sprint-blob-apikey-ui/Cargo.toml`
Expected: no warnings/errors introduced by this change.

- [ ] **Step 3: Self-review under triage rules, then Opus code review (cap 2 iterations).**

- [ ] **Step 4: Push branch and open PR to main.** Closes #280, #186, #241.

---

## Self-Review (plan vs. issues)

- **#280 coverage:** Task 1 (admin) + Task 2 (dispatcher) reorder both HTTP handlers and add one integration test each, asserting sibling bytes survive — matches the issue's exact scope ("apply reorder to both", "integration test per handler").
- **#186 coverage:** Task 3 adds the dispatcher upload form, the visible-to-driver checkbox appending `visibility=driver`, posts to `POST /dispatch/api/v1/blobs`, and refreshes the list on success — matches all three scope bullets. Bonus prerequisite (FormData-aware `apiFetch`) is required for multipart to work at all.
- **#241 coverage:** Task 4 adds the account page listing keys (label, prefix, created/expires/last-used), create flow with one-time plaintext display + copy + "cannot be shown again" wording, revoke with confirm, and the empty-state `.mcp.json` snippet — matches all four scope bullets.
- **Placeholder scan:** No TBD/TODO. All code shown. CSS-class and serde-value uncertainties are flagged with exact grep commands and fallbacks rather than left vague.
- **Type consistency:** `API_KEYS_BASE` defined in Task 4 Step 4; `apiFetch` FormData behavior defined in Task 3 Step 1 and relied on in Task 3 Step 3. `delete_extract`/`count_by_checksum`/`delete_by_id` are existing methods used identically to the MCP reference.
- **Conflict avoidance:** No edits to `mcp.rs`, `mod.rs`, or `config.rs` (the in-flight worktree's surface). Only shared file is `tests/integration_test.rs` (append-only).
