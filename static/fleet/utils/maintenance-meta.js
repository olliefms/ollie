// Shared maintenance category metadata + formatters, reused by the list,
// form, detail, and equipment-embed views so labels stay consistent.
export const CATEGORY_OPTIONS = [
  { value: 'preventive_maintenance', label: 'Preventive Maintenance' },
  { value: 'repair', label: 'Repair' },
  { value: 'tire', label: 'Tire' },
  { value: 'inspection', label: 'Inspection' },
  { value: 'oil_change', label: 'Oil Change' },
  { value: 'brakes', label: 'Brakes' },
  { value: 'other', label: 'Other' },
];

const LABELS = Object.fromEntries(CATEGORY_OPTIONS.map(o => [o.value, o.label]));

export function categoryLabel(value) {
  return LABELS[value] || value || '—';
}

export function money(value) {
  if (value == null) return '—';
  return `$${Number(value).toFixed(2)}`;
}
