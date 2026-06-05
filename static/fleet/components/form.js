import { escHtml } from '../utils/format.js';

/**
 * Pure payload builder. `raw` maps field key → input value (string or bool);
 * `reverted` is a Set of inheritable keys the user chose to revert to inherited.
 * Returns { payload, errors }.
 */
export function buildPayload(fields, raw, reverted = new Set()) {
  const payload = {};
  const errors = [];
  for (const f of fields) {
    const v = raw[f.key];
    if (f.type === 'checkbox') {
      payload[f.key] = !!v;
      continue;
    }
    if (f.type === 'inheritable') {
      if (reverted.has(f.key)) { payload[f.key] = null; continue; }   // explicit clear
      if (v === '' || v === undefined || v === null) continue;        // inherited stays
      payload[f.key] = parseFloat(v);                                 // intentional override
      continue;
    }
    const blank = v === '' || v === undefined || v === null;
    if (f.required && blank) { errors.push(`${f.label} is required.`); continue; }
    if (blank) continue;                                              // omit → "leave unchanged"
    if (f.type === 'number') payload[f.key] = parseFloat(v);
    else if (f.type === 'int') payload[f.key] = parseInt(v, 10);
    else payload[f.key] = v;                                          // text / select
  }
  return { payload, errors };
}

function fieldControl(f, value) {
  const val = value === undefined || value === null ? '' : value;
  if (f.type === 'checkbox') {
    return `<input class="form-checkbox" type="checkbox" data-field="${f.key}" ${value ? 'checked' : ''}>`;
  }
  if (f.type === 'select') {
    const opts = (f.options || []).map(o =>
      `<option value="${escHtml(o)}" ${o === value ? 'selected' : ''}>${escHtml(o)}</option>`).join('');
    return `<select class="form-input" data-field="${f.key}"><option value=""></option>${opts}</select>`;
  }
  if (f.type === 'inheritable') {
    const ph = f.inheritedValue != null ? `Inherited: ${f.inheritedValue} (${escHtml(f.inheritedFrom || '')})` : '';
    return `<input class="form-input" type="number" step="any" data-field="${f.key}"
      value="${value != null ? value : ''}" placeholder="${escHtml(ph)}">`;
  }
  const inputType = (f.type === 'number' || f.type === 'int') ? 'number' : 'text';
  const step = f.type === 'number' ? ' step="any"' : '';
  return `<input class="form-input" type="${inputType}"${step} data-field="${f.key}" value="${escHtml(String(val))}">`;
}

/**
 * Render an inline form panel into `container`.
 * opts: { title, fields, values, submitLabel, onSubmit(payload) -> Promise }
 */
export function renderForm(container, { title, fields, values = {}, submitLabel = 'Save', onSubmit }) {
  const rows = fields.map(f => `
    <div class="form-group">
      <label class="form-label">${escHtml(f.label)}</label>
      ${fieldControl(f, values[f.key])}
    </div>`).join('');

  container.innerHTML = `
    <div class="form-panel">
      <h2 class="form-panel__title">${escHtml(title || '')}</h2>
      <div class="alert alert--error" data-form-error hidden></div>
      ${rows}
      <div class="form-panel__actions">
        <button class="btn btn--primary" data-form-submit>${escHtml(submitLabel)}</button>
      </div>
    </div>`;

  const errEl = container.querySelector('[data-form-error]');
  const submitBtn = container.querySelector('[data-form-submit]');

  function readRaw() {
    const raw = {};
    for (const f of fields) {
      const el = container.querySelector(`[data-field="${f.key}"]`);
      if (!el) continue;
      raw[f.key] = f.type === 'checkbox' ? el.checked : el.value;
    }
    return raw;
  }

  submitBtn.addEventListener('click', async () => {
    const { payload, errors } = buildPayload(fields, readRaw());
    if (errors.length) {
      errEl.textContent = errors.join(' ');
      errEl.hidden = false;
      return;
    }
    errEl.hidden = true;
    submitBtn.disabled = true;
    try {
      const res = await onSubmit(payload);
      if (res && res.ok === false) {
        const data = await res.json().catch(() => ({}));
        errEl.textContent = data.error || `HTTP ${res.status}`;
        errEl.hidden = false;
      }
    } catch (err) {
      if (err && err.message !== 'Unauthorized — please sign in again.') {
        errEl.textContent = `Save failed: ${err.message}`;
        errEl.hidden = false;
      }
    } finally {
      submitBtn.disabled = false;
    }
  });
}
