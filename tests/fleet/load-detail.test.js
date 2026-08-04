import { describe, it, expect } from 'vitest';
import { invoiceableFromStatus } from '../../static/fleet/pages/load-detail.js';

describe('invoiceableFromStatus', () => {
  it('allows a delivered freight load', () => {
    expect(invoiceableFromStatus({ status: 'delivered', kind: 'freight' })).toBe(true);
  });

  it('allows a planned administrative load', () => {
    expect(invoiceableFromStatus({ status: 'planned', kind: 'administrative' })).toBe(true);
  });

  it('refuses a planned freight load', () => {
    expect(invoiceableFromStatus({ status: 'planned', kind: 'freight' })).toBe(false);
  });

  it('refuses an in_transit administrative load', () => {
    expect(invoiceableFromStatus({ status: 'in_transit', kind: 'administrative' })).toBe(false);
  });

  it('refuses an already-invoiced load', () => {
    expect(invoiceableFromStatus({ status: 'invoiced', kind: 'administrative' })).toBe(false);
  });

  it('treats a load with no kind as freight', () => {
    expect(invoiceableFromStatus({ status: 'delivered' })).toBe(true);
    expect(invoiceableFromStatus({ status: 'planned' })).toBe(false);
  });
});
