import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, badge, fmtBytes, fmtDate } from '../utils/format.js';
import { setContent, goBack, withPending } from '../utils/dom.js';
import { confirmAction } from '../components/confirm.js';

// Object-URL lifecycle for the inline preview. renderRoute() calls
// revokeActiveObjectUrl() on every navigation so blob URLs don't leak.
let activeObjectUrl = null;

// Mirrors extract_content() in src/ai/extract.rs — the pipeline's notion of
// "this blob is text". Keeping the two in sync avoids the confusing state
// where a document was summarized as text but the viewer claims it can't be
// previewed (which is what happened to text/markdown).
//
// text/html is the one deliberate divergence: it stays out of preview
// entirely (#184/#240). The pipeline predicate answers "can we pull text out
// of this for summarizing", which is not the same question as "is this safe
// to put in the DOM" — and text/html is exactly where the two part ways.
function isTextMime(mimeType) {
  // Match on the bare type: a stored mime_type may carry parameters
  // ("text/html; charset=utf-8"), and the exclusion has to hold regardless.
  const mt = mimeType.split(';')[0].trim().toLowerCase();
  if (mt === 'text/html') return false;
  return mt.startsWith('text/')
    || mt === 'application/json'
    || mt === 'application/xml'
    || mt.includes('javascript');
}

// A text preview goes into the DOM whole, so bound it. Freight paperwork is
// small, but a stray multi-MB log would otherwise lock up the tab.
const MAX_TEXT_PREVIEW_CHARS = 1_000_000;

export function revokeActiveObjectUrl() {
  if (activeObjectUrl) {
    URL.revokeObjectURL(activeObjectUrl);
    activeObjectUrl = null;
  }
}

export async function renderDocumentDetailView(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const metaRes = await apiFetch(`${API_BASE}/blob/${id}`, {
      headers: { Accept: 'application/json' },
    });
    if (!metaRes.ok) throw new Error(`HTTP ${metaRes.status}`);
    const doc = await metaRes.json();

    const tags = (doc.tags || []).map(t => escHtml(t)).join(', ') || '—';
    const errorRow = doc.status === 'failed' && doc.error
      ? `<div class="detail-item" style="grid-column: 1 / -1;">
           <div class="detail-item__label">Error</div>
           <div class="detail-item__value" style="color:var(--color-danger);">${escHtml(doc.error)}</div>
         </div>`
      : '';

    const html = `
      <button class="back-link" id="doc-back">&#x2190; Back</button>

      <div class="detail-card">
        <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:var(--space-4);padding-bottom:var(--space-3);border-bottom:1px solid var(--color-border);">
          <div style="font-size:1rem;font-weight:700;color:var(--color-text);">${escHtml(doc.name || 'Document')}</div>
          <button class="btn btn--secondary" id="doc-download">Download</button>
        </div>
        <div class="detail-grid">
          <div class="detail-item">
            <div class="detail-item__label">Type</div>
            <div class="detail-item__value">${escHtml(doc.mime_type || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Size</div>
            <div class="detail-item__value">${fmtBytes(doc.size)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(doc.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Uploaded</div>
            <div class="detail-item__value">${fmtDate(doc.created_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Updated</div>
            <div class="detail-item__value">${fmtDate(doc.updated_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Tags</div>
            <div class="detail-item__value">${tags}</div>
          </div>
          ${doc.summary ? `
          <div class="detail-item" style="grid-column: 1 / -1;">
            <div class="detail-item__label">Summary</div>
            <div class="detail-item__value">${escHtml(doc.summary)}</div>
          </div>` : ''}
          ${errorRow}
        </div>
      </div>

      <div class="detail-card">
        <div class="detail-card__title">Preview</div>
        <div id="doc-viewer"><div class="state-loading"><div class="spinner"></div></div></div>
      </div>
    `;

    setContent(html);

    document.getElementById('doc-back').addEventListener('click', goBack);

    const downloadBtn = document.getElementById('doc-download');
    downloadBtn.addEventListener('click', async () => {
      try {
        await withPending(downloadBtn, async () => {
          // A document download has no size bound, so it opts out of the default
          // request deadline rather than aborting a legitimately slow transfer.
          const fileResp = await apiFetch(`${API_BASE}/blob/${id}`, { timeoutMs: 0 });
          if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
          const blob = await fileResp.blob();
          const url = URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = url;
          a.download = doc.name || 'document';
          a.click();
          URL.revokeObjectURL(url);
        });
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          await confirmAction({
            title: 'Download failed',
            message: err.message,
            confirmLabel: 'OK',
          });
        }
      }
    });

    const viewerEl = document.getElementById('doc-viewer');
    const mt = doc.mime_type || '';
    const isPdf = mt === 'application/pdf';
    const isImage = mt.startsWith('image/');
    const isText = isTextMime(mt);
    const canPreview = isPdf || isImage || isText;

    if (!canPreview) {
      const msg = document.createElement('div');
      msg.className = 'state-empty';
      msg.style.minHeight = '80px';
      msg.textContent = "This document type can't be previewed — use the Download button above.";
      viewerEl.textContent = '';
      viewerEl.appendChild(msg);
    } else {
      try {
        const fileResp = await apiFetch(`${API_BASE}/blob/${id}`);
        if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
        const blob = await fileResp.blob();
        viewerEl.textContent = '';
        if (isPdf) {
          const url = URL.createObjectURL(blob);
          activeObjectUrl = url;
          const iframe = document.createElement('iframe');
          iframe.src = url;
          iframe.style.cssText = 'width:100%;height:600px;border:none;';
          iframe.title = doc.name || 'preview';
          viewerEl.appendChild(iframe);
        } else if (isImage) {
          const url = URL.createObjectURL(blob);
          activeObjectUrl = url;
          const img = document.createElement('img');
          img.src = url;
          img.alt = doc.name || 'preview';
          img.style.cssText = 'max-width:100%;height:auto;display:block;';
          viewerEl.appendChild(img);
        } else if (isText) {
          const text = await blob.text();
          const truncated = text.length > MAX_TEXT_PREVIEW_CHARS;
          const pre = document.createElement('pre');
          pre.style.cssText = 'white-space:pre-wrap;word-break:break-word;max-height:600px;overflow:auto;margin:0;padding:12px;background:var(--color-surface-2);border-radius:4px;';
          pre.textContent = truncated ? text.slice(0, MAX_TEXT_PREVIEW_CHARS) : text;
          viewerEl.appendChild(pre);
          if (truncated) {
            const note = document.createElement('div');
            note.id = 'doc-preview-truncated';
            note.className = 'state-empty';
            note.style.minHeight = 'auto';
            note.textContent = 'Preview truncated — use the Download button above for the full file.';
            viewerEl.appendChild(note);
          }
        }
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          viewerEl.textContent = `Preview failed: ${err.message}`;
        }
      }
    }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent('<div class="state-error">Failed to load document.</div>');
    }
  }
}
