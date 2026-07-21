import { test } from 'node:test';
import assert from 'node:assert/strict';
import { categoryLabel, statusLabel, formatMoney, expenseDateStr } from '../../static/driver/utils/expense-meta.js';

test('categoryLabel maps all nine backend categories', () => {
  const cases = {
    fuel: 'Fuel', tolls: 'Tolls', scales: 'Scales', lumper: 'Lumper',
    parking: 'Parking', repair: 'Repair', supplies: 'Supplies',
    permit: 'Permit', other: 'Other',
  };
  for (const [value, label] of Object.entries(cases)) {
    assert.equal(categoryLabel(value), label);
  }
});

test('categoryLabel falls back to Other for unknown values', () => {
  assert.equal(categoryLabel('bogus'), 'Other');
  assert.equal(categoryLabel(undefined), 'Other');
});

test('statusLabel maps submitted/reviewed/settled', () => {
  assert.equal(statusLabel('submitted'), 'Needs review');
  assert.equal(statusLabel('reviewed'), 'Reviewed');
  assert.equal(statusLabel('settled'), 'Settled');
});

test('statusLabel falls back to em dash when missing', () => {
  assert.equal(statusLabel(undefined), '—');
  assert.equal(statusLabel(null), '—');
});

test('formatMoney formats numbers to two decimals with a dollar sign', () => {
  assert.equal(formatMoney(80), '$80.00');
  assert.equal(formatMoney(19.9), '$19.90');
  assert.equal(formatMoney('42.5'), '$42.50');
});

test('formatMoney returns null for absent/invalid amounts', () => {
  assert.equal(formatMoney(null), null);
  assert.equal(formatMoney(undefined), null);
  assert.equal(formatMoney('not-a-number'), null);
});

test('expenseDateStr prefers expense_date over created_at', () => {
  const s = expenseDateStr({ expense_date: '2026-07-10', created_at: '2026-07-15T10:00:00Z' });
  assert.equal(s, '2026-07-10');
});

test('expenseDateStr falls back to the created_at date when expense_date is unset', () => {
  const s = expenseDateStr({ expense_date: null, created_at: '2026-07-15T10:00:00Z' });
  assert.equal(s, '2026-07-15');
});

test('expenseDateStr returns em dash when both are missing', () => {
  assert.equal(expenseDateStr({}), '—');
});
