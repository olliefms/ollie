import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, badge, fmtBytes, fmtDate } from '../utils/format.js';
import { setContent, goBack } from '../utils/dom.js';

// Object-URL lifecycle for the inline preview. renderRoute() calls
// revokeActiveObjectUrl() on every navigation so blob URLs don't leak.
let activeObjectUrl = null;

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

    document.getElementById('doc-download').addEventListener('click', async () => {
      try {
        const fileResp = await apiFetch(`${API_BASE}/blob/${id}`);
        if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
        const blob = await fileResp.blob();
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = doc.name || 'document';
        a.click();
        URL.revokeObjectURL(url);
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          alert(`Download failed: ${err.message}`);
        }
      }
    });

    const viewerEl = document.getElementById('doc-viewer');
    const mt = doc.mime_type || '';
    const isPdf = mt === 'application/pdf';
    const isImage = mt.startsWith('image/');
    const isPlainText = mt === 'text/plain';
    const canPreview = isPdf || isImage || isPlainText;

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
        } else if (isPlainText) {
          const text = await blob.text();
          const pre = document.createElement('pre');
          pre.style.cssText = 'white-space:pre-wrap;word-break:break-word;max-height:600px;overflow:auto;margin:0;padding:12px;background:var(--color-surface-2);border-radius:4px;';
          pre.textContent = text;
          viewerEl.appendChild(pre);
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
