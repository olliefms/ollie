import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';

const FIELDS = [
  { key: 'name', label: 'Name', type: 'text', required: true },
  { key: 'timezone', label: 'Timezone', type: 'text', required: true },
  { key: 'loaded_rate_per_mile', label: 'Loaded Rate / Mile ($)', type: 'number' },
  { key: 'deadhead_rate_per_mile', label: 'Deadhead Rate / Mile ($)', type: 'number' },
  { key: 'extra_stop_fee', label: 'Extra Stop Fee ($)', type: 'number' },
  { key: 'detention_rate_per_hour', label: 'Detention Rate / Hour ($)', type: 'number' },
  { key: 'free_dwell_minutes', label: 'Free Dwell (minutes)', type: 'int' },
  { key: 'is_default', label: 'Set as default terminal', type: 'checkbox' },
];

export async function renderTerminalForm(id) {
  let values = { timezone: 'America/New_York', free_dwell_minutes: 120 };
  if (id) {
    const res = await apiFetch(`${API_BASE}/terminals/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  renderFormPage({
    title: id ? `Edit Terminal — ${values.name || ''}` : 'New Terminal',
    fields: FIELDS,
    values,
    submitLabel: id ? 'Save changes' : 'Create terminal',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/terminals/${encodeURIComponent(id)}`
        : `${API_BASE}/terminals`;
      const res = await apiFetch(url, { method: id ? 'PUT' : 'POST', body: JSON.stringify(payload) });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('terminal-detail', { id: id || saved.id });
      }
      return res;
    },
  });
}
