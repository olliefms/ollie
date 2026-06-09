import { describe, it, expect } from 'vitest';
import {
  serviceTypesFor, toNaiveDateTime, buildLoadPayload, applyResolutionChoices,
} from '../../static/fleet/pages/load-form-payload.js';

describe('serviceTypesFor', () => {
  it('pickup options', () => {
    expect(serviceTypesFor('pickup')).toEqual(['pre_loaded', 'live_load', 'relay']);
  });
  it('delivery options', () => {
    expect(serviceTypesFor('delivery')).toEqual(['live_unload', 'drop_and_hook', 'relay']);
  });
});

describe('toNaiveDateTime', () => {
  it('appends seconds to a datetime-local value', () => {
    expect(toNaiveDateTime('2026-05-10T09:15')).toBe('2026-05-10T09:15:00');
  });
  it('passes through a value that already has seconds', () => {
    expect(toNaiveDateTime('2026-05-10T09:15:30')).toBe('2026-05-10T09:15:30');
  });
  it('returns empty for blank', () => {
    expect(toNaiveDateTime('')).toBe('');
  });
});

describe('buildLoadPayload', () => {
  const baseStop = {
    stop_type: 'pickup', service_type: 'live_load', timezone: 'America/Chicago',
    scheduled_arrive: '2026-05-10T09:15', facility_id: 'fac-1',
  };

  it('omits blank top fields and auto fields, keeps required', () => {
    const { payload, errors } = buildLoadPayload({
      top: { customer_name: 'Acme', load_number: '', miles: '', weight_lbs: '' },
      stops: [baseStop], rateItems: [],
    });
    expect(errors).toEqual([]);
    expect(payload.customer_name).toBe('Acme');
    expect('load_number' in payload).toBe(false);
    expect('miles' in payload).toBe(false);
    expect('weight_lbs' in payload).toBe(false);
  });

  it('requires customer_name and at least one stop', () => {
    const { errors } = buildLoadPayload({ top: { customer_name: '' }, stops: [], rateItems: [] });
    expect(errors).toContain('Customer name is required');
    expect(errors).toContain('At least one stop is required');
  });

  it('numbers stops by row order and normalizes datetime', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [baseStop, { ...baseStop, stop_type: 'delivery', service_type: 'live_unload' }],
      rateItems: [],
    });
    expect(payload.stops[0].sequence).toBe(1);
    expect(payload.stops[1].sequence).toBe(2);
    expect(payload.stops[0].scheduled_arrive).toBe('2026-05-10T09:15:00');
  });

  it('sends facility_name+address when no facility_id', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [{ ...baseStop, facility_id: '', facility_name: 'Dock 7', address: '1 Main St' }],
      rateItems: [],
    });
    expect(payload.stops[0].facility_id).toBeUndefined();
    expect(payload.stops[0].facility_name).toBe('Dock 7');
    expect(payload.stops[0].address).toBe('1 Main St');
  });

  it('coerces rate item amounts and drops empty rows', () => {
    const { payload } = buildLoadPayload({
      top: { customer_name: 'Acme' }, stops: [baseStop],
      rateItems: [{ description: 'Line haul', amount_usd: '1200.50' }, { description: '', amount_usd: '' }],
    });
    expect(payload.rate_items).toEqual([{ description: 'Line haul', amount_usd: 1200.5 }]);
  });

  it('errors on a stop missing both facility_id and name/address', () => {
    const { errors } = buildLoadPayload({
      top: { customer_name: 'Acme' },
      stops: [{ ...baseStop, facility_id: '', facility_name: '', address: '' }],
      rateItems: [],
    });
    expect(errors.some(e => /facility/i.test(e))).toBe(true);
  });
});

describe('applyResolutionChoices', () => {
  it('sets facility_id from a chosen candidate by stop_index', () => {
    const payload = { stops: [{ facility_name: 'X', address: 'Y' }] };
    const out = applyResolutionChoices(payload, { 0: { facility_id: 'fac-9' } });
    expect(out.stops[0].facility_id).toBe('fac-9');
    expect(out.stops[0].facility_name).toBeUndefined();
    expect(out.stops[0].address).toBeUndefined();
  });
  it('sets force_new_facility when chosen', () => {
    const payload = { stops: [{ facility_name: 'X', address: 'Y' }] };
    const out = applyResolutionChoices(payload, { 0: { force_new: true } });
    expect(out.stops[0].force_new_facility).toBe(true);
    expect(out.stops[0].facility_name).toBe('X');
  });
});
