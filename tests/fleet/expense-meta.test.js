import { describe, it, expect } from 'vitest';
import {
  EXPENSE_CATEGORY_OPTIONS,
  PAYMENT_METHOD_OPTIONS,
  expenseCategoryLabel,
  statusBadge,
  dispositionLabel,
} from '../../static/fleet/utils/expense-meta.js';

describe('expense-meta', () => {
  it('exposes the nine expense categories and two payment methods', () => {
    expect(EXPENSE_CATEGORY_OPTIONS.map(o => o.value)).toEqual([
      'fuel', 'tolls', 'scales', 'lumper', 'parking', 'repair', 'supplies', 'permit', 'other',
    ]);
    expect(PAYMENT_METHOD_OPTIONS.map(o => o.value)).toEqual(['company', 'personal']);
  });

  describe('expenseCategoryLabel', () => {
    it('maps a known category to its label', () => {
      expect(expenseCategoryLabel('fuel')).toBe('Fuel');
      expect(expenseCategoryLabel('repair')).toBe('Repair');
    });

    it('falls back to the raw value for an unknown category', () => {
      expect(expenseCategoryLabel('mystery')).toBe('mystery');
    });

    it('falls back to an em dash when empty', () => {
      expect(expenseCategoryLabel('')).toBe('—');
      expect(expenseCategoryLabel(undefined)).toBe('—');
    });
  });

  describe('statusBadge', () => {
    it('renders friendly labels for the three statuses', () => {
      expect(statusBadge('submitted')).toBe('Needs review');
      expect(statusBadge('reviewed')).toBe('Reviewed');
      expect(statusBadge('settled')).toBe('Settled');
    });

    it('falls back to the raw status or an em dash', () => {
      expect(statusBadge('weird')).toBe('weird');
      expect(statusBadge(undefined)).toBe('—');
    });
  });

  describe('dispositionLabel', () => {
    it('maps every disposition branch', () => {
      expect(dispositionLabel({ disposition: 'approved' })).toBe('Approved');
      expect(dispositionLabel({ disposition: 'partial' })).toBe('Partially approved');
      expect(dispositionLabel({ disposition: 'rejected' })).toBe('Rejected');
      expect(dispositionLabel({})).toBe('—');
    });
  });
});
