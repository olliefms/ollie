import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';

// `status` is intentionally absent — the backend rejects it (deny_unknown_fields);
// trucks transition status only via the trip lifecycle.
const FIELDS = [
  { key: 'unit_number', label: 'Unit Number', type: 'text', required: true },
  { key: 'year', label: 'Year', type: 'int' },
  { key: 'make', label: 'Make', type: 'text' },
  { key: 'model', label: 'Model', type: 'text' },
  { key: 'vin', label: 'VIN', type: 'text' },
  { key: 'plate', label: 'Plate', type: 'text' },
  { key: 'plate_state', label: 'Plate State', type: 'text' },
  { key: 'notes', label: 'Notes', type: 'text' },
];

export async function renderTruckForm(id) {
  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/trucks/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  renderFormPage({
    title: id ? `Edit Truck — ${values.unit_number || ''}` : 'New Truck',
    fields: FIELDS,
    values,
    submitLabel: id ? 'Save changes' : 'Add truck',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/trucks/${encodeURIComponent(id)}`
        : `${API_BASE}/trucks`;
      const res = await apiFetch(url, { method: id ? 'PATCH' : 'POST', body: JSON.stringify(payload) });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('truck-detail', { id: id || saved.id });
      }
      return res;
    },
  });
}
