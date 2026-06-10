import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage } from './_detail.js';

const money = v => (v != null ? `$${Number(v).toFixed(2)}` : '—');

export async function renderTerminalDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/terminals/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const t = await res.json();

    renderDetailPage({
      title: t.name || 'Terminal',
      fields: [
        { label: 'Name', value: t.name },
        { label: 'Timezone', value: t.timezone },
        { label: 'Default', value: t.is_default ? 'Yes' : 'No' },
        { label: 'Loaded Rate / Mile', value: money(t.loaded_rate_per_mile) },
        { label: 'Deadhead Rate / Mile', value: money(t.deadhead_rate_per_mile) },
        { label: 'Extra Stop Fee', value: money(t.extra_stop_fee) },
        { label: 'Detention Rate / Hour', value: money(t.detention_rate_per_hour) },
        { label: 'Free Dwell (min)', value: t.free_dwell_minutes },
      ],
      actions: [
        { label: 'Edit', scope: 'terminals:write', onClick: () => navigate('terminal-edit', { id }) },
        { label: 'Delete', scope: 'terminals:delete', onClick: (statusEl) => deleteTerminal(statusEl, id, t.name) },
      ],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load terminal: ${escHtml(err.message)}</div>`);
    }
  }
}

// Terminal delete is a guarded permanent delete: the backend refuses with 409
// if the terminal is the default or has assigned drivers. Surface that message.
async function deleteTerminal(statusEl, id, name) {
  if (!confirm(`Permanently delete terminal "${name}"? This cannot be undone, and is refused if any driver still references it.`)) return;
  try {
    const res = await apiFetch(`${API_BASE}/terminals/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('terminals'); return; }
    const data = await res.json().catch(() => ({}));
    statusEl.hidden = false;
    statusEl.className = 'alert alert--error';
    statusEl.textContent = data.error || `Delete failed (HTTP ${res.status}).`;
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      statusEl.hidden = false;
      statusEl.className = 'alert alert--error';
      statusEl.textContent = `Delete failed: ${err.message}`;
    }
  }
}
