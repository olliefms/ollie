import { apiFetch, API_BASE } from '../utils/api.js';
import { badge, escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';
import { renderDetailPage, detailLink } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';

const RATE_FIELDS = [
  { key: 'loaded_rate_per_mile', label: 'Loaded Rate / Mile', money: true },
  { key: 'deadhead_rate_per_mile', label: 'Deadhead Rate / Mile', money: true },
  { key: 'extra_stop_fee', label: 'Extra Stop Fee', money: true },
  { key: 'detention_rate_per_hour', label: 'Detention Rate / Hour', money: true },
  { key: 'free_dwell_minutes', label: 'Free Dwell (min)', money: false },
];

const fmtVal = (v, money) => (money ? `$${Number(v).toFixed(2)}` : String(v));

// Effective rate: the driver's own override if set, else the terminal's floor.
function rateField(driver, terminal, rf) {
  const own = driver[rf.key];
  if (own != null) return { label: rf.label, value: `${fmtVal(own, rf.money)} (override)` };
  if (terminal && terminal[rf.key] != null) {
    return { label: rf.label, value: `${fmtVal(terminal[rf.key], rf.money)} (inherited)` };
  }
  return { label: rf.label, value: '—' };
}

// Fetch a unit by id; returns its unit_number (falls back to '(unknown unit)').
async function unitNumber(kind, id) {
  try {
    const res = await apiFetch(`${API_BASE}/${kind}/${encodeURIComponent(id)}`);
    if (res.ok) { const u = await res.json(); return u.unit_number || '(unknown unit)'; }
  } catch (_) { /* fall through */ }
  return '(unknown unit)';
}

// Attached-truck field as a clickable link, or '—' when none.
async function truckLink(truckId) {
  if (!truckId) return '—';
  return detailLink('truck-detail', truckId, await unitNumber('trucks', truckId));
}

// Attached-trailer field: comma-separated links, or '—' when none.
async function trailersLink(trailerIds) {
  if (!trailerIds.length) return '—';
  const links = await Promise.all(
    trailerIds.map(async (tid) => detailLink('trailer-detail', tid, await unitNumber('trailers', tid))),
  );
  return links.join(', ');
}

export async function renderDriverDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/drivers/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const d = await res.json();

    let terminal = null;
    if (d.terminal_id) {
      const tRes = await apiFetch(`${API_BASE}/terminals/${encodeURIComponent(d.terminal_id)}`);
      if (tRes.ok) terminal = await tRes.json();
    }

    const [truckField, trailerField] = await Promise.all([
      truckLink(d.current_truck_id),
      trailersLink(d.current_trailer_ids || []),
    ]);

    renderDetailPage({
      title: d.name || 'Driver',
      fields: [
        { label: 'Name', value: d.name },
        { label: 'Status', html: badge(d.status) },
        { label: 'Phone', value: d.phone },
        { label: 'Email', value: d.email },
        { label: 'License #', value: d.license_number },
        { label: 'License State', value: d.license_state },
        { label: 'License Expiry', value: d.license_expiry },
        { label: 'Terminal', value: terminal ? terminal.name : (d.terminal_id || '—') },
        { label: 'Truck', html: truckField },
        { label: 'Trailers', html: trailerField },
        { label: 'Notes', value: d.notes },
        ...RATE_FIELDS.map(rf => rateField(d, terminal, rf)),
      ],
      actions: [
        { label: 'Edit', scope: 'drivers:write', onClick: () => navigate('driver-edit', { id }) },
        { label: 'Set PIN', scope: 'drivers:write', onClick: (s) => setPin(s, id) },
        { label: 'Manage Equipment', scope: 'drivers:write', onClick: (s) => manageEquipment(s, id, d) },
        { label: 'Delete', scope: 'drivers:delete', className: 'btn btn--secondary', onClick: (s) => deleteDriver(s, id, d.name) },
      ],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load driver: ${escHtml(err.message)}</div>`);
    }
  }
}

function showStatus(statusEl, kind, text) {
  statusEl.hidden = false;
  statusEl.className = `alert alert--${kind}`;
  statusEl.textContent = text;
}

async function setPin(statusEl, id) {
  const pin = window.prompt('Set a new driver PIN (digits):');
  if (pin == null || pin === '') return;
  try {
    const res = await apiFetch(`${API_BASE}/drivers/${encodeURIComponent(id)}/pin`, {
      method: 'POST', body: JSON.stringify({ pin }),
    });
    if (res.ok) { showStatus(statusEl, 'success', 'PIN updated.'); return; }
    const data = await res.json().catch(() => ({}));
    showStatus(statusEl, 'error', data.error || `Set PIN failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      showStatus(statusEl, 'error', `Set PIN failed: ${err.message}`);
    }
  }
}

// Soft delete: backend sets status = Inactive (hides from active lists/pickers).
async function deleteDriver(statusEl, id, name) {
  if (!confirmDelete(`driver "${name}"`)) return;
  try {
    const res = await apiFetch(`${API_BASE}/drivers/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('drivers'); return; }
    const data = await res.json().catch(() => ({}));
    showStatus(statusEl, 'error', data.error || `Delete failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      showStatus(statusEl, 'error', `Delete failed: ${err.message}`);
    }
  }
}

// Inline equipment panel: attach a truck and/or trailers, or detach the current
// ones. Renders into the detail status area to avoid a separate route.
async function manageEquipment(statusEl, id, driver) {
  statusEl.hidden = false;
  statusEl.className = 'alert';
  statusEl.innerHTML = '<div class="spinner"></div>';

  let trucks = [];
  let trailers = [];
  try {
    const [tkRes, trRes] = await Promise.all([
      apiFetch(`${API_BASE}/trucks`),
      apiFetch(`${API_BASE}/trailers`),
    ]);
    if (tkRes.ok) { const x = await tkRes.json(); trucks = x.items || (Array.isArray(x) ? x : []); }
    if (trRes.ok) { const x = await trRes.json(); trailers = x.items || (Array.isArray(x) ? x : []); }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      showStatus(statusEl, 'error', `Failed to load equipment: ${err.message}`);
    }
    return;
  }

  const truckOpts = trucks
    .filter(t => t.status !== 'inactive' && t.status !== 'out_of_service')
    .map(t => `<option value="${escHtml(t.id)}">${escHtml(t.unit_number || t.id)}</option>`).join('');
  const trailerOpts = trailers
    .filter(t => t.status !== 'inactive' && t.status !== 'out_of_service')
    .map(t => `<option value="${escHtml(t.id)}">${escHtml(t.unit_number || t.id)}</option>`).join('');

  const curTruck = driver.current_truck_id
    ? (trucks.find(t => t.id === driver.current_truck_id) || {}).unit_number || '(unknown unit)'
    : '—';
  const curTrailers = (driver.current_trailer_ids || []).length
    ? driver.current_trailer_ids.map(tid => (trailers.find(t => t.id === tid) || {}).unit_number || '(unknown unit)').join(', ')
    : '—';

  statusEl.innerHTML = `
    <div class="form-group">
      <div class="detail-item__label">Current truck: ${escHtml(String(curTruck))}</div>
      <div class="detail-item__label">Current trailers: ${escHtml(String(curTrailers))}</div>
    </div>
    <div class="form-group">
      <label class="form-label">Attach truck</label>
      <select class="form-input" data-eq="truck"><option value="">— none —</option>${truckOpts}</select>
    </div>
    <div class="form-group">
      <label class="form-label">Attach trailer</label>
      <select class="form-input" data-eq="trailer"><option value="">— none —</option>${trailerOpts}</select>
    </div>
    <div class="form-panel__actions">
      <button class="btn btn--primary" data-eq-attach>Attach</button>
      <button class="btn btn--secondary" data-eq-detach-truck>Detach truck</button>
      <button class="btn btn--secondary" data-eq-detach-trailers>Detach all trailers</button>
    </div>
    <div class="alert alert--error" data-eq-error hidden></div>`;

  const errEl = statusEl.querySelector('[data-eq-error]');
  const post = async (path, body) => {
    const res = await apiFetch(`${API_BASE}/drivers/${encodeURIComponent(id)}/${path}`, {
      method: 'POST', body: JSON.stringify(body),
    });
    if (res.ok) { renderDriverDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    errEl.hidden = false;
    errEl.textContent = data.error || `Request failed (HTTP ${res.status}).`;
  };

  statusEl.querySelector('[data-eq-attach]').addEventListener('click', () => {
    const truck = statusEl.querySelector('[data-eq="truck"]').value;
    const trailer = statusEl.querySelector('[data-eq="trailer"]').value;
    const body = {};
    if (truck) body.truck = truck;
    if (trailer) body.trailer_ids = [trailer];
    if (!truck && !trailer) {
      errEl.hidden = false; errEl.textContent = 'Pick a truck and/or trailer to attach.'; return;
    }
    post('attach-equipment', body);
  });
  statusEl.querySelector('[data-eq-detach-truck]').addEventListener('click', () => post('detach-equipment', { truck: true }));
  statusEl.querySelector('[data-eq-detach-trailers]').addEventListener('click', () => post('detach-equipment', { all_trailers: true }));
}
