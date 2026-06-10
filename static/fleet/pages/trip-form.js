import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate, goBack } from '../utils/dom.js';
import {
  tripStopTypes,
  buildCreateTripPayload,
  buildTripPatch,
} from './trip-form-payload.js';

const TIMEZONES = [
  { label: 'Eastern',  value: 'America/New_York' },
  { label: 'Central',  value: 'America/Chicago' },
  { label: 'Mountain', value: 'America/Denver' },
  { label: 'Arizona',  value: 'America/Phoenix' },
  { label: 'Pacific',  value: 'America/Los_Angeles' },
  { label: 'Alaska',   value: 'America/Anchorage' },
  { label: 'Hawaii',   value: 'Pacific/Honolulu' },
];
const DEFAULT_TZ = 'America/Chicago';

const RATE_FIELDS = [
  { key: 'loaded_rate_per_mile',   label: 'Loaded Rate / Mile ($)' },
  { key: 'deadhead_rate_per_mile', label: 'Deadhead Rate / Mile ($)' },
  { key: 'extra_stop_fee',         label: 'Extra Stop Fee ($)' },
  { key: 'detention_rate_per_hour', label: 'Detention Rate / Hour ($)' },
  { key: 'free_dwell_minutes',     label: 'Free Dwell (minutes)' },
];

function tzOptions(selected) {
  return TIMEZONES.map(tz =>
    `<option value="${escHtml(tz.value)}"${tz.value === selected ? ' selected' : ''}>${escHtml(tz.label)}</option>`
  ).join('');
}

function stopTypeOptions(selected) {
  return tripStopTypes().map(t =>
    `<option value="${escHtml(t)}"${t === selected ? ' selected' : ''}>${escHtml(t.replace(/_/g, ' '))}</option>`
  ).join('');
}

function facilityOptions(facilities, selectedId) {
  const notListed = `<option value="">— Facility not listed —</option>`;
  const opts = facilities.map(f =>
    `<option value="${escHtml(f.id)}"${f.id === selectedId ? ' selected' : ''}>${escHtml(f.name)}</option>`
  ).join('');
  return notListed + opts;
}

function toLocalInput(v) {
  if (!v) return '';
  return v.replace(/^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}):\d{2}.*$/, '$1');
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

// ── stop editor (free-standing trips + optional custom stops) ─────────────────

function stopRow(facilities, s = {}) {
  const stopType = s.stop_type || 'pickup';
  const tz = s.timezone || DEFAULT_TZ;
  const facilityId = s.facility_id || '';
  const unlisted = facilityId === '';

  return `<div class="contact-row" data-stop-row>
    <div class="form-group">
      <label class="form-label">Stop type *</label>
      <select class="form-input" data-stop-field="stop_type">
        ${stopTypeOptions(stopType)}
      </select>
    </div>
    <div class="form-group">
      <label class="form-label">Facility</label>
      <select class="form-input" data-stop-field="facility_id">
        ${facilityOptions(facilities, facilityId)}
      </select>
    </div>
    <div class="form-group" data-unlisted-fields${unlisted ? '' : ' hidden'}>
      <label class="form-label">Name *</label>
      <input class="form-input" data-stop-field="name" value="${escHtml(s.name || '')}">
      <label class="form-label" style="margin-top:0.5rem">Address *</label>
      <input class="form-input" data-stop-field="address" value="${escHtml(s.address || '')}">
    </div>
    <div class="form-group">
      <label class="form-label">Scheduled arrival *</label>
      <input class="form-input" type="datetime-local" data-stop-field="scheduled_arrive" value="${escHtml(toLocalInput(s.scheduled_arrive))}">
    </div>
    <div class="form-group">
      <label class="form-label">Scheduled arrival end</label>
      <input class="form-input" type="datetime-local" data-stop-field="scheduled_arrive_end" value="${escHtml(toLocalInput(s.scheduled_arrive_end))}">
    </div>
    <div class="form-group">
      <label class="form-label">Timezone *</label>
      <select class="form-input" data-stop-field="timezone">
        ${tzOptions(tz)}
      </select>
    </div>
    <div class="form-group">
      <label class="form-label">Expected dwell (minutes)</label>
      <input class="form-input" type="number" min="0" data-stop-field="expected_dwell_minutes" value="${escHtml(s.expected_dwell_minutes != null ? String(s.expected_dwell_minutes) : '')}">
    </div>
    <div class="form-group">
      <label class="form-label">Detention free (minutes)</label>
      <input class="form-input" type="number" min="0" data-stop-field="detention_free_minutes" value="${escHtml(s.detention_free_minutes != null ? String(s.detention_free_minutes) : '')}">
    </div>
    <div class="form-group">
      <label class="form-label">Detention grace (minutes)</label>
      <input class="form-input" type="number" min="0" data-stop-field="detention_grace_minutes" value="${escHtml(s.detention_grace_minutes != null ? String(s.detention_grace_minutes) : '')}">
    </div>
    <div class="form-group">
      <label class="form-label">Notes</label>
      <input class="form-input" data-stop-field="notes" value="${escHtml(s.notes || '')}">
    </div>
    <button type="button" class="btn-link" data-remove-stop>Remove stop</button>
  </div>`;
}

function wireStopRow(row) {
  const facSel = row.querySelector('[data-stop-field="facility_id"]');
  const unlistedDiv = row.querySelector('[data-unlisted-fields]');
  facSel.addEventListener('change', () => {
    unlistedDiv.hidden = facSel.value !== '';
  });
}

function readStops(stopsHost) {
  const out = [];
  for (const row of stopsHost.querySelectorAll('[data-stop-row]')) {
    const get = (k) => row.querySelector(`[data-stop-field="${k}"]`).value;
    const facilityId = get('facility_id');
    out.push({
      stop_type: get('stop_type'),
      facility_id: facilityId,
      name: facilityId === '' ? get('name') : '',
      address: facilityId === '' ? get('address') : '',
      scheduled_arrive: get('scheduled_arrive'),
      scheduled_arrive_end: get('scheduled_arrive_end'),
      timezone: get('timezone'),
      expected_dwell_minutes: get('expected_dwell_minutes'),
      detention_free_minutes: get('detention_free_minutes'),
      detention_grace_minutes: get('detention_grace_minutes'),
      notes: get('notes'),
    });
  }
  return out;
}

// ── entry point ───────────────────────────────────────────────────────────────

export async function renderTripForm(id) {
  if (id) {
    await renderEditForm(id);
  } else {
    await renderCreateForm();
  }
}

// ── create mode ───────────────────────────────────────────────────────────────

async function renderCreateForm() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const [loads, drivers, trucks, trailers, facilities] = await Promise.all([
    fetchList('loads', ['loads', 'items']),
    fetchList('drivers', ['drivers', 'items']),
    fetchList('trucks', ['items']),
    fetchList('trailers', ['items']),
    fetchList('facilities', ['items', 'facilities']),
  ]);

  const loadOpts = `<option value="">— Select a load —</option>` + loads.map(l =>
    `<option value="${escHtml(l.id)}">${escHtml(l.load_number || l.id)}</option>`
  ).join('');
  const driverOpts = `<option value="">— None —</option>` + drivers.map(d =>
    `<option value="${escHtml(d.id)}">${escHtml(d.name || d.id)}</option>`
  ).join('');
  const truckOpts = `<option value="">— None —</option>` + trucks.map(t =>
    `<option value="${escHtml(t.id)}">${escHtml(t.unit_number || t.id)}</option>`
  ).join('');
  const trailerOpts = trailers.map(t =>
    `<option value="${escHtml(t.id)}">${escHtml(t.unit_number || t.id)}</option>`
  ).join('');

  setContent(`
    <button class="back-link" id="form-back">← Back</button>
    <div class="form-panel">
      <h2 class="form-panel__title">New Trip</h2>
      <div class="alert alert--error" data-form-error hidden></div>

      <div class="form-group">
        <label class="form-label">Trip type</label>
        <select class="form-input" id="mode-select">
          <option value="load" selected>From a load</option>
          <option value="free">Free-standing</option>
        </select>
      </div>

      <div class="form-group" id="load-group">
        <label class="form-label">Load *</label>
        <select class="form-input" data-field="load_id">${loadOpts}</select>
      </div>

      <div class="form-group">
        <label class="form-label">Trip number (blank = auto-assign)</label>
        <input class="form-input" data-field="trip_number">
      </div>
      <div class="form-group">
        <label class="form-label">Notes</label>
        <input class="form-input" data-field="notes">
      </div>

      <h3 class="form-panel__title" style="font-size:1rem;">Resources (optional)</h3>
      <div class="form-group">
        <label class="form-label">Driver</label>
        <select class="form-input" data-field="driver_id">${driverOpts}</select>
      </div>
      <div class="form-group">
        <label class="form-label">Truck</label>
        <select class="form-input" data-field="truck_id">${truckOpts}</select>
      </div>
      <div class="form-group">
        <label class="form-label">Trailers (Ctrl/Cmd-click for multiple)</label>
        <select class="form-input" data-field="trailer_ids" multiple size="4">${trailerOpts}</select>
      </div>

      <div id="stops-section">
        <h3 class="form-panel__title" style="font-size:1rem;">Stops</h3>
        <p class="form-label" id="stops-hint" style="margin-bottom:0.5rem;"></p>
        <label style="display:block;margin-bottom:0.5rem;" id="customize-wrap">
          <input type="checkbox" id="customize-stops"> Customize stops
        </label>
        <div id="stops-host" hidden></div>
        <div class="form-panel__actions" id="add-stop-actions" hidden>
          <button type="button" class="btn btn--secondary" id="add-stop">+ Add stop</button>
        </div>
      </div>

      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>Create Trip</button>
      </div>
    </div>
  `);

  document.getElementById('form-back').addEventListener('click', goBack);

  const modeSelect = document.getElementById('mode-select');
  const loadGroup = document.getElementById('load-group');
  const stopsHost = document.getElementById('stops-host');
  const addStopActions = document.getElementById('add-stop-actions');
  const customizeWrap = document.getElementById('customize-wrap');
  const customizeCb = document.getElementById('customize-stops');
  const stopsHint = document.getElementById('stops-hint');

  function updateRemoveButtons() {
    const rows = stopsHost.querySelectorAll('[data-stop-row]');
    const single = rows.length === 1;
    for (const row of rows) {
      const btn = row.querySelector('[data-remove-stop]');
      btn.hidden = single;
      btn.disabled = single;
    }
  }

  function addStopRow(s = {}) {
    const tmp = document.createElement('div');
    tmp.innerHTML = stopRow(facilities, s);
    const row = tmp.firstElementChild;
    stopsHost.appendChild(row);
    wireStopRow(row);
    row.querySelector('[data-remove-stop]').addEventListener('click', () => {
      row.remove();
      updateRemoveButtons();
    });
    updateRemoveButtons();
  }

  // Show/hide the stop editor depending on mode + customize toggle.
  function syncStopEditor() {
    const free = modeSelect.value === 'free';
    const show = free || (customizeCb && customizeCb.checked);
    stopsHost.hidden = !show;
    addStopActions.hidden = !show;
    customizeWrap.hidden = free; // free-standing always requires stops
    stopsHint.textContent = free
      ? 'Free-standing trips require at least one stop.'
      : 'A load supplies its stops automatically. Check below only to override them.';
    if (show && stopsHost.querySelectorAll('[data-stop-row]').length === 0) {
      addStopRow({ stop_type: 'pickup' });
      if (free) addStopRow({ stop_type: 'delivery' });
    }
    if (!show) stopsHost.innerHTML = '';
  }

  function syncMode() {
    loadGroup.hidden = modeSelect.value !== 'load';
    syncStopEditor();
  }

  modeSelect.addEventListener('change', syncMode);
  customizeCb.addEventListener('change', syncStopEditor);
  document.getElementById('add-stop').addEventListener('click', () => addStopRow());
  syncMode();

  document.querySelector('[data-form-submit]').addEventListener('click', async () => {
    const errEl = document.querySelector('[data-form-error]');
    if (errEl) errEl.hidden = true;

    const fieldVal = (k) => {
      const el = document.querySelector(`[data-field="${k}"]`);
      return el ? el.value.trim() : '';
    };
    const mode = modeSelect.value;
    const trailerSel = document.querySelector('[data-field="trailer_ids"]');
    const trailerIds = trailerSel
      ? Array.from(trailerSel.selectedOptions).map(o => o.value)
      : [];

    const includeStops = mode === 'free' || (customizeCb && customizeCb.checked);
    const stops = includeStops ? readStops(stopsHost) : [];

    const { payload, errors } = buildCreateTripPayload({
      mode,
      top: {
        trip_number: fieldVal('trip_number'),
        load_id: fieldVal('load_id'),
        notes: fieldVal('notes'),
      },
      driver_id: fieldVal('driver_id'),
      truck_id: fieldVal('truck_id'),
      trailer_ids: trailerIds,
      stops,
    });

    if (mode === 'load' && !payload.load_id) {
      errors.push('Pick a load, or switch to a free-standing trip');
    }

    if (errors.length) {
      if (errEl) {
        errEl.textContent = errors.join(' · ');
        errEl.hidden = false;
      }
      return;
    }

    const submitBtn = document.querySelector('[data-form-submit]');
    if (submitBtn) submitBtn.disabled = true;
    try {
      const res = await apiFetch(`${API_BASE}/trips`, {
        method: 'POST',
        body: JSON.stringify(payload),
      });
      const body = await res.json().catch(() => ({}));
      if (!res.ok) {
        if (errEl) {
          errEl.textContent = body.error || `HTTP ${res.status}`;
          errEl.hidden = false;
        }
        return;
      }
      navigate('trip-detail', { id: body.id });
    } catch (err) {
      if (err.message !== 'Unauthorized — please sign in again.') {
        if (errEl) {
          errEl.textContent = `Save failed: ${err.message}`;
          errEl.hidden = false;
        }
      }
    } finally {
      if (submitBtn) submitBtn.disabled = false;
    }
  });
}

// ── edit mode (notes + inheritable rate overrides only) ───────────────────────

function effectivePlaceholder(driverPay, key) {
  // driver_pay only exposes the two per-mile rates as concrete numbers; the
  // other three are folded into aggregate pay, so fall back to a generic hint.
  if (driverPay && driverPay[key] != null) return `Inherited: ${driverPay[key]}`;
  return 'inherited';
}

function rateOverrideRow(rf, placeholder, currentValue) {
  // Prefill with the trip's OWN override when present (null = inherited -> blank).
  const valAttr = currentValue == null ? '' : ` value="${escHtml(String(currentValue))}"`;
  return `<div class="form-group" data-override-row="${escHtml(rf.key)}">
    <label class="form-label">${escHtml(rf.label)}</label>
    <input class="form-input" type="number" step="0.01" data-override-field="${escHtml(rf.key)}"
      placeholder="${escHtml(placeholder)}"${valAttr}>
    <label style="display:block;margin-top:0.35rem;font-size:0.85rem;">
      <input type="checkbox" data-override-clear="${escHtml(rf.key)}"> Clear to inherited
    </label>
  </div>`;
}

async function renderEditForm(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const res = await apiFetch(`${API_BASE}/trips/${encodeURIComponent(id)}`);
  let values = {};
  if (res.ok) values = await res.json();
  const driverPay = values.driver_pay || null;

  const title = `Edit Trip — ${values.trip_number || values.id || ''}`;

  const ratesHtml = RATE_FIELDS.map(rf =>
    rateOverrideRow(rf, effectivePlaceholder(driverPay, rf.key), values[rf.key])
  ).join('');

  setContent(`
    <button class="back-link" id="form-back">← Back</button>
    <div class="form-panel">
      <h2 class="form-panel__title">${escHtml(title)}</h2>
      <div class="alert alert--error" data-form-error hidden></div>

      <p class="form-label" style="margin-bottom:1rem;">
        To change driver, equipment, or stops, use the trip detail page — only notes and
        rate overrides can be edited here.
      </p>

      <div class="form-group">
        <label class="form-label">Notes</label>
        <textarea class="form-input" data-field="notes" rows="3">${escHtml(values.notes || '')}</textarea>
      </div>

      <h3 class="form-panel__title" style="font-size:1rem;">Rate overrides</h3>
      <p class="form-label" style="margin-bottom:0.5rem;">
        Leave blank to inherit. Type a value to override, or check "Clear to inherited" to remove an existing override.
      </p>
      ${ratesHtml}

      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>Save changes</button>
      </div>
    </div>
  `);

  document.getElementById('form-back').addEventListener('click', goBack);

  // A typed value and "clear" are mutually exclusive — checking clear disables
  // the input; typing unchecks clear.
  for (const rf of RATE_FIELDS) {
    const input = document.querySelector(`[data-override-field="${rf.key}"]`);
    const clear = document.querySelector(`[data-override-clear="${rf.key}"]`);
    clear.addEventListener('change', () => {
      if (clear.checked) { input.value = ''; input.disabled = true; }
      else { input.disabled = false; }
    });
    input.addEventListener('input', () => {
      if (input.value) clear.checked = false;
    });
  }

  document.querySelector('[data-form-submit]').addEventListener('click', async () => {
    const errEl = document.querySelector('[data-form-error]');
    if (errEl) errEl.hidden = true;

    const overrideState = {};
    for (const rf of RATE_FIELDS) {
      const input = document.querySelector(`[data-override-field="${rf.key}"]`);
      const clear = document.querySelector(`[data-override-clear="${rf.key}"]`);
      overrideState[rf.key] = { value: input.value, cleared: clear.checked };
    }

    const { payload, errors } = buildTripPatch({
      notes: document.querySelector('[data-field="notes"]').value,
      ...overrideState,
    });

    if (errors.length) {
      if (errEl) {
        errEl.textContent = errors.join(' · ');
        errEl.hidden = false;
      }
      return;
    }

    const submitBtn = document.querySelector('[data-form-submit]');
    if (submitBtn) submitBtn.disabled = true;
    try {
      const pRes = await apiFetch(`${API_BASE}/trips/${encodeURIComponent(id)}`, {
        method: 'PATCH',
        body: JSON.stringify(payload),
      });
      const body = await pRes.json().catch(() => ({}));
      if (!pRes.ok) {
        if (errEl) {
          errEl.textContent = body.error || `HTTP ${pRes.status}`;
          errEl.hidden = false;
        }
        return;
      }
      navigate('trip-detail', { id });
    } catch (err) {
      if (err.message !== 'Unauthorized — please sign in again.') {
        if (errEl) {
          errEl.textContent = `Save failed: ${err.message}`;
          errEl.hidden = false;
        }
      }
    } finally {
      if (submitBtn) submitBtn.disabled = false;
    }
  });
}
