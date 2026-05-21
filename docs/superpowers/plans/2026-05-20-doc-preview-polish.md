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

### Task 1 — #184 dispatch MIME-branch preview

**File:** `static/dispatch/app.js` around line 1167.

**Empirical finding from Playwright (Chrome current build):** the original plan's `sandbox='allow-same-origin'` does NOT render blob PDFs — Chrome's PDF viewer fails under *every* sandbox value (`''`, `'allow-same-origin'`, `'allow-scripts'`, combinations). The only working configuration is no sandbox attribute at all. The proposed simple fix in the issue body is wrong.

To remove the sandbox safely, branch by MIME and use the right element per type:

- `application/pdf` → `<iframe>` with no sandbox attribute. PDFs cannot execute scripts, so the parent's session is not exposed even though a blob URL inherits the parent's origin.
- `image/*` → `<img>` element. No iframe needed.
- `text/plain` → `<pre>` with `textContent` (XSS-safe by definition).
- `text/html` → dropped from the canPreview list. It is the only previewable type that can execute scripts, and we have no safe way to render it inline now. Falls through to the "use Download" message.

This is a small scope creep from the original plan but is the correct, minimal fix that actually addresses the bug.

**Verification:** Playwright MCP run during the sprint confirmed:
- iframe with no sandbox → PDF renders correctly in Chrome.
- iframe with `sandbox=''` / `'allow-same-origin'` → broken-doc icon (reproduces the bug).
- iframe with `'allow-scripts'` variants → blank / no render.

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

### Task 2b — Driver app PDF preview parity

While we are in `static/driver/pages/trip-detail.js openDocPreview`, the existing PDF branch also sets `frame.sandbox = 'allow-same-origin'`, which the Playwright investigation showed is broken in current Chrome — drivers see the same broken-doc icon for PDFs that dispatch users do. Remove the line. The driver PDF preview becomes a plain `<iframe>` with no sandbox, matching the dispatch fix and resolving the latent glitch the user reported.

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
