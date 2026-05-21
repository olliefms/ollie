# Doc preview polish sprint (2026-05-20)

Three independent doc-preview fixes batched into one PR for shared review context.

## In scope

- **#184** — Dispatch app: inline preview blocked in Chrome and Brave because `iframe.sandbox = ''` is maximum restriction. Simple fix only — match the driver-app pattern.
- **#205** — Driver app: doc preview should fallback to image branch only for explicit `image/*` MIME, not for unknown types.
- **#206** — Driver app: doc preview overlay needs Esc-key + backdrop-click dismissal.

## Out of scope

- **#184 full fix** — signed preview token endpoint that solves Brave's blob-URL-PDF download behavior. Deferred; tracked in #184 research comment from 2026-05-17. Brave will remain broken after this sprint; that is a documented limitation, not a regression.
- Any change to `Content-Disposition` headers on the blob endpoints.

## Why batched

Three small, independent doc-preview fixes that share a single review pass. Not interdependent. Single PR for review-context efficiency, not atomicity.

## Tasks

### Task 1 — #184 dispatch sandbox

**File:** `static/dispatch/app.js` (line 1187)

Change:
```js
iframe.sandbox = '';
```
to:
```js
iframe.sandbox = 'allow-same-origin';
```

`allow-same-origin` is the minimum permission needed to let the browser's native PDF viewer and image renderer initialize. Without `allow-scripts`, scripts in HTML blobs still cannot execute, so XSS from a malicious uploaded HTML file is still blocked.

**Verification:** Open a PDF in the dispatch app via Chrome (Playwright MCP). Confirm the iframe renders. Manual user check in Brave is expected to still fail (documented limitation).

### Task 2 — #205 driver MIME fallback

**File:** `static/driver/pages/trip-detail.js` (lines 297-308, inside `openDocPreview`)

Replace the current `if pdf else img` with explicit MIME branching:

```js
if (doc.mime_type === 'application/pdf') {
  const frame = document.createElement('iframe');
  frame.sandbox = 'allow-same-origin';
  frame.src = blobUrl;
  overlay.appendChild(frame);
} else if (doc.mime_type && doc.mime_type.startsWith('image/')) {
  const img = document.createElement('img');
  img.className = 'doc-preview__img';
  img.alt = doc.name;
  img.src = blobUrl;
  overlay.appendChild(img);
} else {
  const msg = document.createElement('div');
  msg.className = 'doc-preview__error';
  msg.textContent = "This document type can't be previewed.";
  overlay.appendChild(msg);
}
```

**Verification:** Unit covered by inspection — no driver-app tests exist for this code path. Manual: trigger preview with a non-image, non-pdf doc — expect the unsupported message, not a broken `<img>`.

### Task 3 — #206 driver Esc + backdrop dismiss

**File:** `static/driver/pages/trip-detail.js` (inside `openDocPreview`, around the `teardown` definition)

Extend `teardown` to remove the new listeners. Add the listeners after the overlay is in the DOM.

```js
const onKey = (e) => { if (e.key === 'Escape') teardown(); };
const onBackdrop = (e) => { if (e.target === overlay) teardown(); };

const teardown = () => {
  document.removeEventListener('keydown', onKey);
  overlay.removeEventListener('click', onBackdrop);
  if (blobUrl) URL.revokeObjectURL(blobUrl);
  if (overlay.parentNode) overlay.parentNode.removeChild(overlay);
};

// (existing) close.addEventListener('click', teardown);
document.addEventListener('keydown', onKey);
overlay.addEventListener('click', onBackdrop);
```

Note: `teardown` is currently declared `const teardown = () =>` before `close.addEventListener('click', teardown)`. The listeners `onKey` and `onBackdrop` reference `teardown` from their closures — declare them with `let`/`const` and ensure declaration order is valid (declare the named handler functions, then declare `teardown` referencing them, then attach all listeners).

**Verification:** Manual: open preview, press Esc → closes. Open preview, click outside the close button on the dark overlay → closes. Click on the close button (×) — still works.

## Sequencing

No interdependence. Tasks 1, 2, 3 can be done in any order. Implementation is small enough to do inline rather than dispatching subagents.

## Test command

```bash
cargo test --manifest-path /Users/jimp7508/src/ollie/Cargo.toml
```

Static-file changes don't affect Rust tests, but run the suite to ensure no regression.

## Manual verification

- Chrome (Playwright MCP): load dispatch doc detail page, open a PDF → renders inline.
- Driver app (manual): three flows — PDF preview still works; unknown-type preview shows unsupported message; Esc and backdrop click both close the overlay.

## Release notes hook

If/when this ships in the next release, mention:
- Inline document preview now works in Chrome (Brave still requires download — separate work).
- Driver doc preview: improved error message for unsupported types, added Esc/backdrop close.
