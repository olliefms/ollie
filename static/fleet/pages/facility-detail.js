import { apiFetch, API_BASE } from '../utils/api.js';
import { badge, escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';

function contactsHtml(contacts) {
  if (!contacts || !contacts.length) return '—';
  return `<ul class="detail-list">${contacts.map(c => {
    const bits = [c.title, c.phone, c.email].filter(Boolean).map(escHtml).join(' · ');
    return `<li><strong>${escHtml(c.name || '—')}</strong>${bits ? ` — ${bits}` : ''}${c.notes ? `<br><span class="detail-item__label">${escHtml(c.notes)}</span>` : ''}</li>`;
  }).join('')}</ul>`;
}

export async function renderFacilityDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/facilities/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const f = await res.json();

    const dwell = f.avg_dwell_minutes != null
      ? `${Number(f.avg_dwell_minutes).toFixed(0)} min (${f.dwell_sample_count ?? 0} samples)`
      : '—';

    const actions = [
      { label: 'Edit', scope: 'facilities:write', onClick: () => navigate('facility-edit', { id }) },
    ];
    if (f.archived) {
      actions.push({ label: 'Reactivate', scope: 'facilities:write', onClick: (s) => reactivate(s, id) });
      actions.push({ label: 'Permanently delete', scope: 'facilities:delete', className: 'btn btn--secondary', onClick: (s) => permanentDelete(s, id, f.name) });
    } else {
      actions.push({ label: 'Delete', scope: 'facilities:write', className: 'btn btn--secondary', onClick: (s) => archive(s, id, f.name) });
    }

    renderDetailPage({
      title: f.name || 'Facility',
      fields: [
        { label: 'Name', value: f.name },
        { label: 'Status', html: f.archived ? badge('archived') : badge('active') },
        { label: 'Address', value: f.address },
        { label: 'Normalized Address', value: f.normalized_address },
        { label: 'Geocode', html: badge(f.geocode_status) },
        { label: 'Geocode Failures', value: f.geocode_failure_count },
        { label: 'Coordinates', value: (f.lat != null && f.lng != null) ? `${f.lat}, ${f.lng}` : '—' },
        { label: 'Avg Dwell', value: dwell },
        { label: 'Tags', value: (f.tags || []).join(', ') },
        { label: 'Notes', value: f.notes },
        { label: 'Contacts', html: contactsHtml(f.contacts) },
      ],
      actions,
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load facility: ${err.message}</div>`);
    }
  }
}

function showError(statusEl, text) {
  statusEl.hidden = false;
  statusEl.className = 'alert alert--error';
  statusEl.textContent = text;
}

// Tier 1 — soft archive (reversible). Default delete.
async function archive(statusEl, id, name) {
  if (!confirmDelete(`facility "${name}"`)) return;
  try {
    const res = await apiFetch(`${API_BASE}/facilities/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('facilities'); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Delete failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Delete failed: ${err.message}`);
  }
}

async function reactivate(statusEl, id) {
  try {
    const res = await apiFetch(`${API_BASE}/facilities/${encodeURIComponent(id)}/reactivate`, { method: 'POST' });
    if (res.ok) { renderFacilityDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Reactivate failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Reactivate failed: ${err.message}`);
  }
}

// Tier 2 — permanent purge. Type the name to confirm; backend refuses with 409
// + an enumerated referrer list when any load stop references the facility.
async function permanentDelete(statusEl, id, name) {
  const typed = window.prompt(`Permanently delete "${name}"? This cannot be undone.\nType the facility name to confirm:`);
  if (typed == null) return;
  if (typed !== name) { showError(statusEl, 'Name did not match — permanent delete cancelled.'); return; }
  try {
    const res = await apiFetch(`${API_BASE}/facilities/${encodeURIComponent(id)}/permanent`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('facilities'); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Permanent delete failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Permanent delete failed: ${err.message}`);
  }
}
