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
      const n = parseFloat(v);
      if (Number.isNaN(n)) continue;                                  // garbage → never override
      payload[f.key] = n;                                             // intentional override
      continue;
    }
    const blank = v === '' || v === undefined || v === null;
    if (f.required && blank) { errors.push(`${f.label} is required.`); continue; }
    if (blank) continue;                                              // omit → "leave unchanged"
    if (f.type === 'number') {
      const n = parseFloat(v);
      if (!Number.isNaN(n)) payload[f.key] = n;
    } else if (f.type === 'int') {
      const n = parseInt(v, 10);
      if (!Number.isNaN(n)) payload[f.key] = n;
    } else {
      payload[f.key] = v;                                             // text / select
    }
  }
  return { payload, errors };
}

function fieldControl(f, value) {
  const val = value === undefined || value === null ? '' : value;
  const key = escHtml(f.key);
  if (f.type === 'checkbox') {
    return `<input class="form-checkbox" type="checkbox" data-field="${key}" ${value ? 'checked' : ''}>`;
  }
  if (f.type === 'select') {
    const cur = value == null ? '' : String(value);
    const opts = (f.options || []).map((o) => {
      const ov = typeof o === 'object' ? String(o.value) : String(o);
      const ol = typeof o === 'object' ? String(o.label) : String(o);
      return `<option value="${escHtml(ov)}" ${ov === cur ? 'selected' : ''}>${escHtml(ol)}</option>`;
    }).join('');
    return `<select class="form-input" data-field="${key}"><option value=""></option>${opts}</select>`;
  }
  if (f.type === 'date') {
    return `<input class="form-input" type="date" data-field="${key}" value="${escHtml(String(val))}">`;
  }
  if (f.type === 'inheritable') {
    const hasOverride = value != null && value !== '';
    const ph = f.inheritedValue != null
      ? `Inherited: ${f.inheritedValue}${f.inheritedFrom ? ` (${f.inheritedFrom})` : ''}`
      : '';
    return `<div class="inheritable-field">
      <input class="form-input" type="number" step="any" data-field="${key}"
        value="${hasOverride ? escHtml(String(value)) : ''}" placeholder="${escHtml(ph)}">
      <button type="button" class="btn-link" data-revert="${key}" ${hasOverride ? '' : 'hidden'}>Revert to inherited</button>
    </div>`;
  }
  const inputType = (f.type === 'number' || f.type === 'int') ? 'number' : 'text';
  const step = f.type === 'number' ? ' step="any"' : '';
  return `<input class="form-input" type="${inputType}"${step} data-field="${key}" value="${escHtml(String(val))}">`;
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

  // Inheritable "Revert to inherited" affordance: clicking clears the input and
  // marks the field reverted (→ explicit null in the payload). Typing a value
  // again un-reverts it. buildPayload reads this set as its third argument.
  const reverted = new Set();
  container.querySelectorAll('[data-revert]').forEach((btn) => {
    const key = btn.getAttribute('data-revert');
    btn.addEventListener('click', () => {
      const input = container.querySelector(`[data-field="${key}"]`);
      if (input) input.value = '';
      reverted.add(key);
      btn.hidden = true;
    });
  });
  container.querySelectorAll('.inheritable-field [data-field]').forEach((input) => {
    const key = input.getAttribute('data-field');
    input.addEventListener('input', () => {
      if (input.value !== '') {
        reverted.delete(key);
        const btn = container.querySelector(`[data-revert="${key}"]`);
        if (btn) btn.hidden = false;
      }
    });
  });

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
    const { payload, errors } = buildPayload(fields, readRaw(), reverted);
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
