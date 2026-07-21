// Shared expense category/status metadata + display helpers, reused by the
// list, form, and detail views so labels stay consistent with the backend.
export const EXPENSE_CATEGORY_OPTIONS = [
  { value: 'fuel', label: 'Fuel' },
  { value: 'tolls', label: 'Tolls' },
  { value: 'scales', label: 'Scales' },
  { value: 'lumper', label: 'Lumper' },
  { value: 'parking', label: 'Parking' },
  { value: 'repair', label: 'Repair' },
  { value: 'supplies', label: 'Supplies' },
  { value: 'permit', label: 'Permit' },
  { value: 'other', label: 'Other' },
];

export const PAYMENT_METHOD_OPTIONS = [
  { value: 'company', label: 'Company funds' },
  { value: 'personal', label: 'Personal / cash' },
];

export function expenseCategoryLabel(v) {
  const hit = EXPENSE_CATEGORY_OPTIONS.find(o => o.value === v);
  return hit ? hit.label : (v || '—');
}

export function statusBadge(status) {
  return { submitted: 'Needs review', reviewed: 'Reviewed', settled: 'Settled' }[status] || status || '—';
}

// Mirrors the backend derivation for display-only fallbacks.
export function dispositionLabel(e) {
  if (e.disposition === 'approved') return 'Approved';
  if (e.disposition === 'partial') return 'Partially approved';
  if (e.disposition === 'rejected') return 'Rejected';
  return '—';
}
