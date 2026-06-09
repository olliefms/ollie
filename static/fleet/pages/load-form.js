import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate, goBack } from '../utils/dom.js';
import { buildLoadPayload, serviceTypesFor } from './load-form-payload.js';

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

function tzOptions(selected) {
  return TIMEZONES.map(tz =>
    `<option value="${escHtml(tz.value)}"${tz.value === selected ? ' selected' : ''}>${escHtml(tz.label)}</option>`
  ).join('');
}

function serviceTypeOptions(stopType, selected) {
  return serviceTypesFor(stopType).map(st =>
    `<option value="${escHtml(st)}"${st === selected ? ' selected' : ''}>${escHtml(st.replace(/_/g, ' '))}</option>`
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
  // Trim seconds from YYYY-MM-DDTHH:MM:SS → YYYY-MM-DDTHH:MM
  return v.replace(/^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}):\d{2}.*$/, '$1');
}

function stopRow(facilities, s = {}) {
  const stopType = s.stop_type || 'pickup';
  const tz = s.timezone || DEFAULT_TZ;
  const facilityId = s.facility_id || '';
  const unlisted = facilityId === '';
  const serviceType = s.service_type || '';

  return `<div class="contact-row" data-stop-row>
    <div class="form-group">
      <label class="form-label">Stop type *</label>
      <select class="form-input" data-stop-field="stop_type">
        <option value="pickup"${stopType === 'pickup' ? ' selected' : ''}>Pickup</option>
        <option value="delivery"${stopType === 'delivery' ? ' selected' : ''}>Delivery</option>
      </select>
    </div>
    <div class="form-group">
      <label class="form-label">Service type</label>
      <select class="form-input" data-stop-field="service_type">
        ${serviceTypeOptions(stopType, serviceType)}
      </select>
    </div>
    <div class="form-group">
      <label class="form-label">Facility</label>
      <select class="form-input" data-stop-field="facility_id">
        ${facilityOptions(facilities, facilityId)}
      </select>
    </div>
    <div class="form-group" data-unlisted-fields${unlisted ? '' : ' hidden'}>
      <label class="form-label">Facility name *</label>
      <input class="form-input" data-stop-field="facility_name" value="${escHtml(s.facility_name || '')}">
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

function rateItemRow(r = {}) {
  return `<div class="contact-row" data-rate-row>
    <div class="form-group">
      <label class="form-label">Description</label>
      <input class="form-input" data-rate-field="description" value="${escHtml(r.description || '')}">
    </div>
    <div class="form-group">
      <label class="form-label">Amount ($)</label>
      <input class="form-input" type="number" step="0.01" data-rate-field="amount_usd" value="${escHtml(r.amount_usd != null ? String(r.amount_usd) : '')}">
    </div>
    <button type="button" class="btn-link" data-remove-rate>Remove item</button>
  </div>`;
}

function wireStopRow(row) {
  const typeSel = row.querySelector('[data-stop-field="stop_type"]');
  const svcSel = row.querySelector('[data-stop-field="service_type"]');
  const facSel = row.querySelector('[data-stop-field="facility_id"]');
  const unlistedDiv = row.querySelector('[data-unlisted-fields]');

  typeSel.addEventListener('change', () => {
    const prev = svcSel.value;
    const types = serviceTypesFor(typeSel.value);
    svcSel.innerHTML = types.map(st =>
      `<option value="${escHtml(st)}"${st === prev ? ' selected' : ''}>${escHtml(st.replace(/_/g, ' '))}</option>`
    ).join('');
    if (types.includes(prev)) svcSel.value = prev;
  });

  facSel.addEventListener('change', () => {
    unlistedDiv.hidden = facSel.value !== '';
  });
}

function wireRemoveStop(row, updateRemoveButtons) {
  row.querySelector('[data-remove-stop]').addEventListener('click', () => {
    row.remove();
    updateRemoveButtons();
  });
}

function wireRemoveRate(row, updateTotal) {
  row.querySelector('[data-remove-rate]').addEventListener('click', () => {
    row.remove();
    updateTotal();
  });
}

function readStops(stopsHost) {
  const out = [];
  for (const row of stopsHost.querySelectorAll('[data-stop-row]')) {
    const get = (k) => row.querySelector(`[data-stop-field="${k}"]`).value;
    const facilityId = get('facility_id');
    const stop = {
      stop_type: get('stop_type'),
      service_type: get('service_type'),
      facility_id: facilityId,
      facility_name: facilityId === '' ? get('facility_name') : '',
      address: facilityId === '' ? get('address') : '',
      scheduled_arrive: get('scheduled_arrive'),
      scheduled_arrive_end: get('scheduled_arrive_end'),
      timezone: get('timezone'),
      expected_dwell_minutes: get('expected_dwell_minutes'),
      detention_free_minutes: get('detention_free_minutes'),
      detention_grace_minutes: get('detention_grace_minutes'),
      notes: get('notes'),
    };
    out.push(stop);
  }
  return out;
}

function readRateItems(rateHost) {
  const out = [];
  for (const row of rateHost.querySelectorAll('[data-rate-row]')) {
    const description = row.querySelector('[data-rate-field="description"]').value;
    const amount_usd = row.querySelector('[data-rate-field="amount_usd"]').value;
    out.push({ description, amount_usd });
  }
  return out;
}

function computeTotal(rateHost) {
  let total = 0;
  for (const row of rateHost.querySelectorAll('[data-rate-row]')) {
    const v = parseFloat(row.querySelector('[data-rate-field="amount_usd"]').value);
    if (!Number.isNaN(v)) total += v;
  }
  return total;
}

// TODO(Task 5): replace stub with candidate picker
function handleFacilityResolution(body, payload) {
  const errEl = document.querySelector('[data-form-error]');
  errEl.textContent = 'One or more stops matched existing facilities. Disambiguation step coming next.';
  errEl.hidden = false;
}

export async function renderLoadForm(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  let facilities = [];
  try {
    const fRes = await apiFetch(`${API_BASE}/facilities`);
    if (fRes.ok) {
      const fData = await fRes.json();
      facilities = fData.items || fData.facilities || (Array.isArray(fData) ? fData : []);
    }
  } catch { /* render anyway; facility select will just have "not listed" */ }

  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/loads/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  const title = id ? `Edit Load — ${values.load_number || values.id || ''}` : 'New Load';
  const stops = values.stops || [];
  const rateItems = values.rate_items || [];

  const stopsHtml = stops.length
    ? stops.map(s => stopRow(facilities, s)).join('')
    : stopRow(facilities);

  const rateHtml = rateItems.map(r => rateItemRow(r)).join('');

  setContent(`
    <button class="back-link" id="form-back">← Back</button>
    <div class="form-panel">
      <h2 class="form-panel__title">${escHtml(title)}</h2>
      <div class="alert alert--error" data-form-error hidden></div>

      <h3 class="form-panel__title" style="font-size:1rem;">Load details</h3>
      <div class="form-group">
        <label class="form-label">Customer name *</label>
        <input class="form-input" data-field="customer_name" value="${escHtml(values.customer_name || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Customer reference</label>
        <input class="form-input" data-field="customer_ref" value="${escHtml(values.customer_ref || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Load number (blank = auto-assign)</label>
        <input class="form-input" data-field="load_number" value="${escHtml(values.load_number || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Commodity</label>
        <input class="form-input" data-field="commodity" value="${escHtml(values.commodity || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Weight (lbs)</label>
        <input class="form-input" type="number" min="0" data-field="weight_lbs" value="${escHtml(values.weight_lbs != null ? String(values.weight_lbs) : '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Miles (blank = async route)</label>
        <input class="form-input" type="number" min="0" data-field="miles" value="${escHtml(values.miles != null ? String(values.miles) : '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Notes</label>
        <input class="form-input" data-field="notes" value="${escHtml(values.notes || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Tags (comma-separated)</label>
        <input class="form-input" data-field="tags" value="${escHtml((values.tags || []).join(', '))}">
      </div>

      <h3 class="form-panel__title" style="font-size:1rem;">Stops</h3>
      <div id="stops-host">${stopsHtml}</div>
      <div class="form-panel__actions">
        <button type="button" class="btn btn--secondary" id="add-stop">+ Add stop</button>
      </div>

      <h3 class="form-panel__title" style="font-size:1rem;">Rate items</h3>
      <div id="rate-host">${rateHtml}</div>
      <div class="form-panel__actions">
        <button type="button" class="btn btn--secondary" id="add-rate">+ Add rate item</button>
        <span id="rate-total" style="margin-left:1rem;font-weight:600;"></span>
      </div>

      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>${id ? 'Save Load' : 'Create Load'}</button>
      </div>
    </div>
  `);

  document.getElementById('form-back').addEventListener('click', goBack);

  const errEl = document.querySelector('[data-form-error]');
  const stopsHost = document.getElementById('stops-host');
  const rateHost = document.getElementById('rate-host');
  const submitBtn = document.querySelector('[data-form-submit]');
  const rateTotalEl = document.getElementById('rate-total');

  function updateRemoveButtons() {
    const rows = stopsHost.querySelectorAll('[data-stop-row]');
    const single = rows.length === 1;
    for (const row of rows) {
      const btn = row.querySelector('[data-remove-stop]');
      btn.hidden = single;
      btn.disabled = single;
    }
  }

  function updateRateTotal() {
    const total = computeTotal(rateHost);
    rateTotalEl.textContent = rateHost.querySelectorAll('[data-rate-row]').length
      ? `Total: $${total.toFixed(2)}`
      : '';
  }

  function addStopRow(s = {}) {
    const tmp = document.createElement('div');
    tmp.innerHTML = stopRow(facilities, s);
    const row = tmp.firstElementChild;
    stopsHost.appendChild(row);
    wireStopRow(row);
    wireRemoveStop(row, updateRemoveButtons);
    updateRemoveButtons();
  }

  function addRateRow(r = {}) {
    const tmp = document.createElement('div');
    tmp.innerHTML = rateItemRow(r);
    const row = tmp.firstElementChild;
    rateHost.appendChild(row);
    wireRemoveRate(row, updateRateTotal);
    row.querySelector('[data-rate-field="amount_usd"]').addEventListener('input', updateRateTotal);
    updateRateTotal();
  }

  // Wire existing stop rows
  for (const row of stopsHost.querySelectorAll('[data-stop-row]')) {
    wireStopRow(row);
    wireRemoveStop(row, updateRemoveButtons);
  }
  updateRemoveButtons();

  // Wire existing rate rows
  for (const row of rateHost.querySelectorAll('[data-rate-row]')) {
    wireRemoveRate(row, updateRateTotal);
    row.querySelector('[data-rate-field="amount_usd"]').addEventListener('input', updateRateTotal);
  }
  updateRateTotal();

  document.getElementById('add-stop').addEventListener('click', () => addStopRow());
  document.getElementById('add-rate').addEventListener('click', () => addRateRow());

  submitBtn.addEventListener('click', async () => {
    errEl.hidden = true;

    const get = (k) => document.querySelector(`[data-field="${k}"]`).value.trim();
    const tagsRaw = get('tags');
    const top = {
      customer_name: get('customer_name'),
      customer_ref: get('customer_ref'),
      load_number: get('load_number'),
      commodity: get('commodity'),
      weight_lbs: get('weight_lbs'),
      miles: get('miles'),
      notes: get('notes'),
      tags: tagsRaw.split(',').map(t => t.trim()).filter(Boolean),
    };

    const stopData = readStops(stopsHost);
    const rateData = readRateItems(rateHost);

    const { payload, errors } = buildLoadPayload({ top, stops: stopData, rateItems: rateData });

    if (errors.length) {
      errEl.textContent = errors.join(' · ');
      errEl.hidden = false;
      return;
    }

    submitBtn.disabled = true;
    try {
      const url = id
        ? `${API_BASE}/loads/${encodeURIComponent(id)}`
        : `${API_BASE}/loads`;
      const res = await apiFetch(url, {
        method: id ? 'PUT' : 'POST',
        body: JSON.stringify(payload),
      });
      const body = await res.json().catch(() => ({}));

      if (!res.ok) {
        errEl.textContent = body.error || `HTTP ${res.status}`;
        errEl.hidden = false;
        return;
      }

      if (Array.isArray(body) && body.some(r => r && r.facility_resolution_required)) {
        handleFacilityResolution(body, payload);
        return;
      }

      navigate('load-detail', { id: body.id });
    } catch (err) {
      if (err.message !== 'Unauthorized — please sign in again.') {
        errEl.textContent = `Save failed: ${err.message}`;
        errEl.hidden = false;
      }
    } finally {
      submitBtn.disabled = false;
    }
  });
}
