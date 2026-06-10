import { apiFetch, API_BASE } from '../utils/api.js';
import { navigate } from '../utils/dom.js';
import { renderFormPage } from './_form.js';

// `status` is intentionally absent — the backend rejects it (deny_unknown_fields);
// trailers transition status only via the trip lifecycle. `owner_name` is required
// by the backend when owner is not "fleet" (validated server-side, surfaced inline).
const FIELDS = [
  { key: 'unit_number', label: 'Unit Number', type: 'text', required: true },
  { key: 'owner', label: 'Owner', type: 'select', required: true,
    options: ['fleet', 'carrier', 'customer', 'other'] },
  { key: 'owner_name', label: 'Owner Name (required unless fleet)', type: 'text' },
  { key: 'year', label: 'Year', type: 'int' },
  { key: 'make', label: 'Make', type: 'text' },
  { key: 'trailer_type', label: 'Trailer Type', type: 'text' },
  { key: 'length_ft', label: 'Length (ft)', type: 'number' },
  { key: 'vin', label: 'VIN', type: 'text' },
  { key: 'plate', label: 'Plate', type: 'text' },
  { key: 'plate_state', label: 'Plate State', type: 'text' },
  { key: 'notes', label: 'Notes', type: 'text' },
];

export async function renderTrailerForm(id) {
  let values = {};
  if (id) {
    const res = await apiFetch(`${API_BASE}/trailers/${encodeURIComponent(id)}`);
    if (res.ok) values = await res.json();
  }

  renderFormPage({
    title: id ? `Edit Trailer — ${values.unit_number || ''}` : 'New Trailer',
    fields: FIELDS,
    values,
    submitLabel: id ? 'Save changes' : 'Add trailer',
    onSubmit: async (payload) => {
      const url = id
        ? `${API_BASE}/trailers/${encodeURIComponent(id)}`
        : `${API_BASE}/trailers`;
      const res = await apiFetch(url, { method: id ? 'PATCH' : 'POST', body: JSON.stringify(payload) });
      if (res.ok) {
        const saved = await res.json().catch(() => ({}));
        navigate('trailer-detail', { id: id || saved.id });
      }
      return res;
    },
  });
}
