import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, goBack, navigate } from '../utils/dom.js';

// Facility form is bespoke: alongside the scalar fields it carries a repeatable
// contacts sub-section (the general repeatable primitive lands with Loads in
// Phase 4). tags is a comma-separated input; lat/lng are optional and must be
// supplied together (backend 422s otherwise).

const CONTACT_FIELDS = [
  { key: 'name', label: 'Name', required: true },
  { key: 'title', label: 'Title' },
  { key: 'phone', label: 'Phone' },
  { key: 'email', label: 'Email' },
  { key: 'notes', label: 'Notes' },
];

function contactRow(c = {}) {
  const inputs = CONTACT_FIELDS.map(f => `
    <div class="form-group">
      <label class="form-label">${escHtml(f.label)}${f.required ? ' *' : ''}</label>
      <input class="form-input" data-contact-field="${f.key}" value="${escHtml(c[f.key] == null ? '' : String(c[f.key]))}">
    </div>`).join('');
  return `<div class="contact-row" data-contact-row>
    ${inputs}
    <button type="button" class="btn-link" data-remove-contact>Remove contact</button>
  </div>`;
}

export async function renderFacilityForm(id) {
  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/facilities/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  const title = id ? `Edit Facility — ${values.name || ''}` : 'New Facility';
  const contacts = values.contacts || [];

  setContent(`
    <button class="back-link" id="form-back">← Back</button>
    <div class="form-panel">
      <h2 class="form-panel__title">${escHtml(title)}</h2>
      <div class="alert alert--error" data-form-error hidden></div>
      <div class="form-group">
        <label class="form-label">Name *</label>
        <input class="form-input" data-field="name" value="${escHtml(values.name || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Address *</label>
        <input class="form-input" data-field="address" value="${escHtml(values.address || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Notes</label>
        <input class="form-input" data-field="notes" value="${escHtml(values.notes || '')}">
      </div>
      <div class="form-group">
        <label class="form-label">Tags (comma-separated)</label>
        <input class="form-input" data-field="tags" value="${escHtml((values.tags || []).join(', '))}">
      </div>
      <div class="form-group">
        <label class="form-label">Latitude (optional)</label>
        <input class="form-input" type="number" step="any" data-field="lat" value="${values.lat != null ? escHtml(String(values.lat)) : ''}">
      </div>
      <div class="form-group">
        <label class="form-label">Longitude (optional)</label>
        <input class="form-input" type="number" step="any" data-field="lng" value="${values.lng != null ? escHtml(String(values.lng)) : ''}">
      </div>
      <h3 class="form-panel__title" style="font-size:1rem;">Contacts</h3>
      <div id="contacts-host">${contacts.map(contactRow).join('')}</div>
      <div class="form-panel__actions">
        <button type="button" class="btn btn--secondary" id="add-contact">+ Add contact</button>
      </div>
      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>${id ? 'Save changes' : 'Create facility'}</button>
      </div>
    </div>
  `);

  document.getElementById('form-back').addEventListener('click', goBack);

  const errEl = document.querySelector('[data-form-error]');
  const contactsHost = document.getElementById('contacts-host');
  const submitBtn = document.querySelector('[data-form-submit]');

  function wireRemove(row) {
    row.querySelector('[data-remove-contact]').addEventListener('click', () => row.remove());
  }
  contactsHost.querySelectorAll('[data-contact-row]').forEach(wireRemove);

  document.getElementById('add-contact').addEventListener('click', () => {
    const tmp = document.createElement('div');
    tmp.innerHTML = contactRow();
    const row = tmp.firstElementChild;
    contactsHost.appendChild(row);
    wireRemove(row);
  });

  function readContacts() {
    const out = [];
    for (const row of contactsHost.querySelectorAll('[data-contact-row]')) {
      const c = {};
      for (const f of CONTACT_FIELDS) {
        const v = row.querySelector(`[data-contact-field="${f.key}"]`).value.trim();
        if (v !== '') c[f.key] = v;
      }
      if (c.name) out.push(c);   // a contact must at least have a name
    }
    return out;
  }

  function buildPayload() {
    const get = (k) => document.querySelector(`[data-field="${k}"]`).value.trim();
    const name = get('name');
    const address = get('address');
    if (!name) return { error: 'Name is required.' };
    if (!address) return { error: 'Address is required.' };

    const payload = { name, address, contacts: readContacts() };
    const notes = get('notes');
    if (notes) payload.notes = notes;
    const tags = get('tags').split(',').map(t => t.trim()).filter(Boolean);
    payload.tags = tags;

    const latRaw = get('lat');
    const lngRaw = get('lng');
    if ((latRaw === '') !== (lngRaw === '')) {
      return { error: 'Latitude and longitude must be provided together.' };
    }
    if (latRaw !== '' && lngRaw !== '') {
      const lat = parseFloat(latRaw);
      const lng = parseFloat(lngRaw);
      if (Number.isNaN(lat) || Number.isNaN(lng)) return { error: 'Latitude/longitude must be numbers.' };
      payload.lat = lat;
      payload.lng = lng;
    }
    return { payload };
  }

  submitBtn.addEventListener('click', async () => {
    const { payload, error } = buildPayload();
    if (error) { errEl.textContent = error; errEl.hidden = false; return; }
    errEl.hidden = true;
    submitBtn.disabled = true;
    try {
      const url = id
        ? `${API_BASE}/facilities/${encodeURIComponent(id)}`
        : `${API_BASE}/facilities`;
      const res = await apiFetch(url, { method: id ? 'PATCH' : 'POST', body: JSON.stringify(payload) });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('facility-detail', { id: id || saved.id });
        return;
      }
      const data = await res.json().catch(() => ({}));
      errEl.textContent = data.error || `HTTP ${res.status}`;
      errEl.hidden = false;
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
