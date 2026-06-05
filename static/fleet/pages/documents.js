import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml, badge, fmtBytes, fmtDate } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';

export async function renderDocumentsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const offset = params.offset || 0;
  const filterName = params.name || '';

  try {
    const qs = new URLSearchParams({ limit: 20, offset });
    if (filterName) qs.set('name', filterName);

    const resp = await apiFetch(`${API_BASE}/blobs?${qs}`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const data = await resp.json();
    const blobs = data.items || [];

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

    let tableHtml = '';
    if (blobs.length === 0 && offset === 0) {
      tableHtml = '<div class="state-empty">No documents found</div>';
    } else {
      const rows = blobs.map(b => `
        <tr class="doc-row" data-blob-id="${b.id}" style="cursor:pointer;">
          <td>${escHtml(b.name) || '—'}</td>
          <td style="font-size:var(--text-sm);color:var(--color-text-muted);">${escHtml((b.mime_type || '').split('/').pop())}</td>
          <td>${fmtBytes(b.size)}</td>
          <td>${badge(b.status)}</td>
          <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(b.summary) || '—'}</td>
          <td>${fmtDate(b.created_at)}</td>
        </tr>
      `).join('');

      tableHtml = `
        <div class="table-wrapper">
          <table class="data-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Type</th>
                <th>Size</th>
                <th>Status</th>
                <th>Summary</th>
                <th>Uploaded</th>
              </tr>
            </thead>
            <tbody>${rows}</tbody>
          </table>
        </div>
        ${blobs.length === 20 ? `
          <div style="text-align:center;margin-top:var(--space-3);">
            <button class="btn btn--secondary" id="doc-load-more">Load more</button>
          </div>` : ''}
      `;
    }

    setContent(filterHtml + tableHtml);

    document.getElementById('doc-filter-apply')?.addEventListener('click', () => {
      const name = document.getElementById('doc-filter-name').value.trim();
      navigate('documents', { name });
    });
    document.getElementById('doc-filter-name')?.addEventListener('keydown', e => {
      if (e.key === 'Enter') navigate('documents', { name: e.target.value.trim() });
    });
    document.getElementById('doc-load-more')?.addEventListener('click', () => {
      navigate('documents', { name: filterName, offset: offset + 20 });
    });

    document.querySelectorAll('.doc-row').forEach(row => {
      row.addEventListener('click', () => {
        navigate('document', { id: row.dataset.blobId });
      });
    });

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
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load documents: ${err.message}</div>`);
    }
  }
}
