import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';

// Rate-override fields inherit their effective value from the driver's terminal.
// Each terminal carries a concrete rate floor under the same field names.
const RATE_FIELDS = [
  { key: 'loaded_rate_per_mile', label: 'Loaded Rate / Mile ($)' },
  { key: 'deadhead_rate_per_mile', label: 'Deadhead Rate / Mile ($)' },
  { key: 'extra_stop_fee', label: 'Extra Stop Fee ($)' },
  { key: 'detention_rate_per_hour', label: 'Detention Rate / Hour ($)' },
  { key: 'free_dwell_minutes', label: 'Free Dwell (minutes)' },
];

function inheritedPlaceholder(terminal, key) {
  if (!terminal) return '';
  const iv = terminal[key];
  return iv != null ? `Inherited: ${iv} (Terminal: ${terminal.name})` : '';
}

export async function renderDriverForm(id) {
  // Terminals back both the terminal select and the inherited-rate source.
  let terminals = [];
  try {
    const tRes = await apiFetch(`${API_BASE}/terminals`);
    if (tRes.ok) {
      const tData = await tRes.json();
      terminals = tData.items || tData.terminals || (Array.isArray(tData) ? tData : []);
    }
  } catch { /* terminals optional — fields still render, just without inherited hints */ }

  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/drivers/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  const defaultTerminal = terminals.find(t => t.is_default) || terminals[0];
  const selectedId = values.terminal_id || (defaultTerminal && defaultTerminal.id);
  const selectedTerminal = terminals.find(t => t.id === selectedId) || defaultTerminal;

  // `status` is intentionally absent — UpdateDriverRequest does not accept it
  // (drivers transition status via the trip lifecycle / soft delete only).
  const fields = [
    { key: 'name', label: 'Name', type: 'text', required: true },
    { key: 'phone', label: 'Phone', type: 'text' },
    { key: 'email', label: 'Email', type: 'text' },
    { key: 'license_number', label: 'License Number', type: 'text' },
    { key: 'license_state', label: 'License State', type: 'text' },
    { key: 'license_expiry', label: 'License Expiry', type: 'date' },
    { key: 'notes', label: 'Notes', type: 'text' },
    {
      key: 'terminal_id', label: 'Terminal', type: 'select',
      options: terminals.map(t => ({ value: t.id, label: t.name })),
    },
    ...RATE_FIELDS.map(rf => ({
      key: rf.key, label: rf.label, type: 'inheritable',
      inheritedValue: selectedTerminal ? selectedTerminal[rf.key] : null,
      inheritedFrom: selectedTerminal ? `Terminal: ${selectedTerminal.name}` : '',
    })),
  ];

  renderFormPage({
    title: id ? `Edit Driver — ${values.name || ''}` : 'New Driver',
    fields,
    values,
    submitLabel: id ? 'Save changes' : 'Add driver',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/drivers/${encodeURIComponent(id)}`
        : `${API_BASE}/drivers`;
      const res = await apiFetch(url, { method: id ? 'PATCH' : 'POST', body: JSON.stringify(payload) });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('driver-detail', { id: id || saved.id });
      }
      return res;
    },
  });

  // When the terminal selection changes, refresh the inherited-rate ghost
  // placeholders so the user sees what each rate would fall back to.
  const host = document.getElementById('form-host');
  const termSel = host && host.querySelector('[data-field="terminal_id"]');
  if (termSel) {
    termSel.addEventListener('change', () => {
      const t = terminals.find(x => x.id === termSel.value);
      for (const rf of RATE_FIELDS) {
        const input = host.querySelector(`[data-field="${rf.key}"]`);
        if (input) input.placeholder = inheritedPlaceholder(t, rf.key);
      }
    });
  }
}
