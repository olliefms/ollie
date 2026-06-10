import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import {
  escHtml, badge, shortId, fmtDate, fmtArrivalWindow, fmtMiles,
} from '../utils/format.js';
import { setContent, navigate, goBack } from '../utils/dom.js';

// Status → allowed transitions (mirrors the backend lifecycle).
const CAN_ASSIGN     = (s) => s === 'planned';
const CAN_UNASSIGN   = (s) => s === 'assigned';
const CAN_DISPATCH   = (s) => s === 'assigned';
const CAN_UNDISPATCH = (s) => s === 'dispatched';
const CAN_STOP_TIMES = (s) => s === 'dispatched' || s === 'in_transit';
const CAN_COMPLETE   = (s) => s === 'delivered';

// A naive local datetime string, no timezone (YYYY-MM-DDTHH:MM:SS), matching the
// convention trip/load stop times use. Defaults to "now".
function nowNaive() {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

// Browser datetime-local prompt → naive string. Returns null if cancelled.
function promptNaiveDatetime(label) {
  const def = nowNaive().slice(0, 16); // YYYY-MM-DDTHH:MM for the prompt default
  const v = window.prompt(`${label} (YYYY-MM-DDTHH:MM):`, def);
  if (v === null) return null;
  const trimmed = v.trim();
  if (!trimmed) return null;
  // Pad to seconds if the user left them off.
  return /T\d{2}:\d{2}$/.test(trimmed) ? `${trimmed}:00` : trimmed;
}

export async function renderTripDetail(id) {
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = 'Trip Detail';
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const trip = await res.json();

    if (topbarTitle) topbarTitle.textContent = `Trip ${trip.trip_number || shortId(id)}`;

    const ms = trip.mileage_summary;
    const hasOrigin = !!(ms && ms.origin);
    const legs = (ms && ms.legs) || [];

    // Leg-index contract:
    //  - origin present: legs[0] is deadhead (origin → stop_1), legs[1+] loaded between stops
    //    => stop i (1-based) inbound miles = legs[i-1]
    //  - origin absent: legs[0] is stop_1 → stop_2
    //    => stop i (1-based, i>1) inbound miles = legs[i-2]; stop 1 has none
    const milesForStop = (i /* 0-based stop index */) => {
      if (hasOrigin) {
        return fmtMiles(legs[i] ? legs[i].miles : null);
      }
      if (i === 0) return '—';
      return fmtMiles(legs[i - 1] ? legs[i - 1].miles : null);
    };

    const originRow = hasOrigin ? `
      <tr>
        <td>0</td>
        <td>${escHtml(ms.origin.facility_name || '—')}${ms.origin.address ? ` — ${escHtml(ms.origin.address)}` : ''}</td>
        <td>origin</td>
        <td>—</td>
        <td>—</td>
        <td>—</td>
        <td style="text-align:right; font-variant-numeric: tabular-nums;">—</td>
        <td></td>
      </tr>
    ` : '';

    const stops = trip.stops || [];
    const canStopTimes = hasScope('trips:write') && CAN_STOP_TIMES(trip.status);

    const stopRows = stops.map((stop, i) => {
      const seq = stop.sequence;
      const stopActions = canStopTimes ? `
        <button class="btn-link" data-stop-arrive="${seq}">Arrive</button>
        <button class="btn-link" data-stop-depart="${seq}">Depart</button>
        <button class="btn-link" data-stop-late="${seq}">Late</button>
      ` : '';
      return `
      <tr>
        <td>${i + 1}</td>
        <td>${escHtml(stop.name || '—')}</td>
        <td>${escHtml(stop.stop_type || '—')}</td>
        <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
        <td>${fmtDate(stop.actual_arrive)}</td>
        <td>${fmtDate(stop.actual_depart)}</td>
        <td style="text-align:right; font-variant-numeric: tabular-nums;">${milesForStop(i)}</td>
        <td style="white-space:nowrap;">${stopActions}</td>
      </tr>
    `;
    }).join('');

    const totalMiles = ms ? fmtMiles(ms.total_miles) : '—';
    const bodyRows = (originRow + stopRows) || '<tr><td colspan="8" style="text-align:center; padding: var(--space-4); color: var(--color-text-muted);">No stops</td></tr>';

    // ── Action bar (scope + status gated) ────────────────────────
    const canWrite = hasScope('trips:write');
    const canDelete = hasScope('trips:delete');
    const st = trip.status;
    const settled = trip.settlement_ref != null;

    const actionBtns = [
      canWrite ? `<button class="btn btn--secondary" id="trip-action-edit">Edit</button>` : '',
      canWrite && CAN_ASSIGN(st) ? `<button class="btn btn--secondary" id="trip-action-assign">Assign</button>` : '',
      canWrite && CAN_UNASSIGN(st) ? `<button class="btn btn--secondary" id="trip-action-unassign">Unassign</button>` : '',
      canWrite && CAN_DISPATCH(st) ? `<button class="btn btn--secondary" id="trip-action-dispatch">Dispatch</button>` : '',
      canWrite && CAN_UNDISPATCH(st) ? `<button class="btn btn--secondary" id="trip-action-undispatch">Undispatch</button>` : '',
      canWrite && CAN_COMPLETE(st) ? `<button class="btn btn--secondary" id="trip-action-complete">Complete</button>` : '',
      canWrite ? `<button class="btn btn--secondary" id="trip-action-check-call">Check Call</button>` : '',
      canWrite && !settled ? `<button class="btn btn--secondary" id="trip-action-recalc">Recalculate Miles</button>` : '',
      canDelete ? `<button class="btn btn--secondary" id="trip-action-delete">Delete</button>` : '',
    ].join('');

    setContent(`
      <button class="back-link" id="back-to-trips">← Back to Trips</button>
      <div class="detail-card">
        <div class="detail-card__title">Trip ${escHtml(trip.trip_number || shortId(trip.id))}</div>
        <div class="detail-grid">
          <div class="detail-item"><div class="detail-item__label">Trip #</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Status</div><div class="detail-item__value">${badge(trip.status)}</div></div>
          <div class="detail-item"><div class="detail-item__label">Driver</div><div class="detail-item__value">${escHtml(trip.driver_name || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Truck</div><div class="detail-item__value">${escHtml(trip.truck_unit || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Trailer</div><div class="detail-item__value">${escHtml((trip.trailer_units || []).join(', ') || '—')}</div></div>
        </div>
        ${actionBtns ? `<div class="form-panel__actions">${actionBtns}</div>` : ''}
      </div>

      <div id="trip-action-status" class="alert" hidden style="margin-top:var(--space-3);"></div>

      <div class="detail-card">
        <div class="detail-card__title">Stops</div>
        <div class="table-wrapper">
          <table class="data-table">
            <thead><tr><th>#</th><th>Facility</th><th>Type</th><th>Scheduled Arrive</th><th>Actual Arrive</th><th>Actual Depart</th><th style="text-align:right;">Miles</th><th>Actions</th></tr></thead>
            <tbody>${bodyRows}</tbody>
            <tfoot>
              <tr><td colspan="6" style="font-weight:600;">Total Miles</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${totalMiles}</td><td></td></tr>
            </tfoot>
          </table>
        </div>
      </div>
    `);

    document.getElementById('back-to-trips').addEventListener('click', goBack);

    const statusEl = document.getElementById('trip-action-status');

    document.getElementById('trip-action-edit')?.addEventListener('click', () => {
      navigate('trip-edit', { id });
    });
    document.getElementById('trip-action-assign')?.addEventListener('click', () => assignTrip(statusEl, id));
    document.getElementById('trip-action-unassign')?.addEventListener('click', () => simpleAction(statusEl, id, `${id}/unassign`, 'Unassign'));
    document.getElementById('trip-action-dispatch')?.addEventListener('click', () => simpleAction(statusEl, id, `${id}/dispatch`, 'Dispatch'));
    document.getElementById('trip-action-undispatch')?.addEventListener('click', () => simpleAction(statusEl, id, `${id}/undispatch`, 'Undispatch'));
    document.getElementById('trip-action-complete')?.addEventListener('click', () => simpleAction(statusEl, id, `${id}/complete`, 'Complete'));
    document.getElementById('trip-action-check-call')?.addEventListener('click', () => checkCall(statusEl, id));
    document.getElementById('trip-action-recalc')?.addEventListener('click', () => recalcMiles(statusEl, id));
    document.getElementById('trip-action-delete')?.addEventListener('click', () => deleteTrip(statusEl, id));

    document.querySelectorAll('[data-stop-arrive]').forEach((el) => {
      el.addEventListener('click', () => stopTime(statusEl, id, el.dataset.stopArrive, 'arrive', 'actual_arrive', 'Arrival time'));
    });
    document.querySelectorAll('[data-stop-depart]').forEach((el) => {
      el.addEventListener('click', () => stopTime(statusEl, id, el.dataset.stopDepart, 'depart', 'actual_depart', 'Departure time'));
    });
    document.querySelectorAll('[data-stop-late]').forEach((el) => {
      el.addEventListener('click', () => stopLate(statusEl, id, el.dataset.stopLate));
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trip: ${err.message}</div>`);
    }
  }
}

function showError(statusEl, text) {
  statusEl.hidden = false;
  statusEl.className = 'alert alert--error';
  statusEl.textContent = text;
}

function showWarning(statusEl, text) {
  statusEl.hidden = false;
  statusEl.className = 'alert alert--warning';
  statusEl.textContent = text;
}

// Re-fetch + re-render after a successful non-delete action, surfacing any
// mileage warning the response carried.
async function afterAction(statusEl, id, res) {
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    showError(statusEl, body.error || `HTTP ${res.status}`);
    return false;
  }
  await renderTripDetail(id);
  if (body && body.mileage_recompute_warning) {
    const el = document.getElementById('trip-action-status');
    if (el) showWarning(el, body.mileage_recompute_warning);
  }
  return true;
}

// POST with no body, 200|204 success, re-render on success.
async function simpleAction(statusEl, id, pathSuffix, label) {
  try {
    const res = await apiFetch(`${API_BASE}/trips/${pathSuffix}`, { method: 'POST' });
    if (res.status === 204 || res.ok) { await renderTripDetail(id); return; }
    const body = await res.json().catch(() => ({}));
    showError(statusEl, body.error || `${label} failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `${label} failed: ${err.message}`);
  }
}

async function assignTrip(statusEl, id) {
  try {
    const [drivers, trucks, trailers] = await Promise.all([
      fetchList('drivers', ['drivers', 'items']),
      fetchList('trucks', ['items']),
      fetchList('trailers', ['items']),
    ]);
    const driverId = pickFrom('driver', drivers, (d) => d.name || d.id);
    if (driverId === null) return;
    const truckId = pickFrom('truck', trucks, (t) => t.unit_number || t.id);
    if (truckId === null) return;
    const trailerInput = window.prompt('Trailer unit numbers (comma-separated, optional):', '');
    if (trailerInput === null) return;
    const trailerIds = resolveTrailerIds(trailerInput, trailers);

    const res = await apiFetch(`${API_BASE}/trips/${id}/assign`, {
      method: 'POST',
      body: JSON.stringify({ driver_id: driverId, truck_id: truckId, trailer_ids: trailerIds }),
    });
    await afterAction(statusEl, id, res);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Assign failed: ${err.message}`);
  }
}

// Prompt for one id from a list by index. Returns the id, or null if cancelled.
function pickFrom(label, items, labelFn) {
  if (items.length === 0) {
    window.alert(`No ${label}s available.`);
    return null;
  }
  const menu = items.map((it, i) => `${i + 1}. ${labelFn(it)}`).join('\n');
  const raw = window.prompt(`Select a ${label} by number:\n${menu}`, '1');
  if (raw === null) return null;
  const n = parseInt(raw.trim(), 10);
  if (!Number.isInteger(n) || n < 1 || n > items.length) {
    window.alert(`Invalid ${label} selection.`);
    return null;
  }
  return items[n - 1].id;
}

function resolveTrailerIds(input, trailers) {
  const wanted = input.split(',').map((s) => s.trim()).filter(Boolean);
  const ids = [];
  for (const unit of wanted) {
    const match = trailers.find((t) => (t.unit_number || '') === unit || t.id === unit);
    if (match) ids.push(match.id);
  }
  return ids;
}

async function fetchList(path, keys) {
  try {
    const res = await apiFetch(`${API_BASE}/${path}`);
    if (!res.ok) return [];
    const data = await res.json();
    for (const k of keys) {
      if (Array.isArray(data[k])) return data[k];
    }
    return Array.isArray(data) ? data : [];
  } catch {
    return [];
  }
}

async function stopTime(statusEl, id, seq, action, field, label) {
  const dt = promptNaiveDatetime(label);
  if (dt === null) return;
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}/stops/${seq}/${action}`, {
      method: 'POST',
      body: JSON.stringify({ [field]: dt }),
    });
    await afterAction(statusEl, id, res);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `${label} failed: ${err.message}`);
  }
}

async function stopLate(statusEl, id, seq) {
  const eta = window.prompt('ETA (YYYY-MM-DDTHH:MM, optional):', '');
  if (eta === null) return;
  const notes = window.prompt('Notes (optional):', '');
  if (notes === null) return;
  const body = {};
  if (eta.trim()) {
    const t = eta.trim();
    body.eta = /T\d{2}:\d{2}$/.test(t) ? `${t}:00` : t;
  }
  if (notes.trim()) body.notes = notes.trim();
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}/stops/${seq}/late`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (res.status === 204 || res.ok) { await renderTripDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Late failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Late failed: ${err.message}`);
  }
}

async function checkCall(statusEl, id) {
  const location = window.prompt('Location:', '');
  if (location === null) return;
  if (!location.trim()) { showError(statusEl, 'Check call requires a location.'); return; }
  const notes = window.prompt('Notes (optional):', '');
  if (notes === null) return;
  const eta = window.prompt('ETA to next stop (YYYY-MM-DDTHH:MM, optional):', '');
  if (eta === null) return;
  const body = { location: location.trim() };
  if (notes.trim()) body.notes = notes.trim();
  if (eta.trim()) {
    const t = eta.trim();
    body.eta_next_stop = /T\d{2}:\d{2}$/.test(t) ? `${t}:00` : t;
  }
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}/check-call`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (res.status === 204 || res.ok) { await renderTripDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Check call failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Check call failed: ${err.message}`);
  }
}

async function recalcMiles(statusEl, id) {
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}/recalculate-miles`, {
      method: 'POST',
      body: JSON.stringify({ force: true }),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      showError(statusEl, body.error || `Recalculate failed (HTTP ${res.status}).`);
      return;
    }
    await renderTripDetail(id);
    if (body && body.mileage_recompute_warning) {
      const el = document.getElementById('trip-action-status');
      if (el) showWarning(el, body.mileage_recompute_warning);
    }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Recalculate failed: ${err.message}`);
  }
}

async function deleteTrip(statusEl, id) {
  if (!confirm('Delete this trip? Planned/assigned/dispatched trips are cancelled; an already-cancelled trip is permanently deleted. This cannot be undone.')) return;
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}`, { method: 'DELETE' });
    if (res.status === 204 || res.ok) { navigate('trips'); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Delete failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Delete failed: ${err.message}`);
  }
}
