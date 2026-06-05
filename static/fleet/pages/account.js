import { apiFetch } from '../utils/api.js';
import { escHtml, fmtDate } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';

const API_KEYS_BASE = '/fleet/api-keys';

export async function renderAccountView() {
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
      "url": "https://YOUR_HOST/fleet/mcp",
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
