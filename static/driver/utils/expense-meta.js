// Expense category/status/money display helpers shared by the expenses list
// and (soon) the trip-detail upload sheet. Pure functions, no DOM — kept
// separate from pages/expenses.js so they stay unit-testable.
const CATEGORY_LABELS = {
  fuel: 'Fuel',
  tolls: 'Tolls',
  scales: 'Scales',
  lumper: 'Lumper',
  parking: 'Parking',
  repair: 'Repair',
  supplies: 'Supplies',
  permit: 'Permit',
  other: 'Other',
};

const STATUS_LABELS = {
  submitted: 'Needs review',
  reviewed: 'Reviewed',
  settled: 'Settled',
};

export function categoryLabel(category) {
  return CATEGORY_LABELS[category] || 'Other';
}

export function statusLabel(status) {
  return STATUS_LABELS[status] || status || '—';
}

export function formatMoney(value) {
  if (value === null || value === undefined) return null;
  const n = Number(value);
  if (Number.isNaN(n)) return null;
  return `$${n.toFixed(2)}`;
}

export function expenseDateStr(expense) {
  if (expense.expense_date) return expense.expense_date;
  if (expense.created_at) return String(expense.created_at).slice(0, 10);
  return '—';
}
