import { describe, it, expect } from 'vitest';
import { VIEW_PATHS } from '../../static/fleet/utils/dom.js';

describe('VIEW_PATHS filter/pagination serialization', () => {
  it('serializes the documents name + offset params', () => {
    expect(VIEW_PATHS.documents({})).toBe('/fleet/documents');
    expect(VIEW_PATHS.documents({ name: 'rate con', offset: 20 }))
      .toBe('/fleet/documents?name=rate+con&offset=20');
  });

  it('serializes the loads status filter', () => {
    expect(VIEW_PATHS.loads({})).toBe('/fleet/loads');
    expect(VIEW_PATHS.loads({ status: 'planned' })).toBe('/fleet/loads?status=planned');
  });

  it('serializes the trips status filter', () => {
    expect(VIEW_PATHS.trips({})).toBe('/fleet/trips');
    expect(VIEW_PATHS.trips({ status: 'dispatched' })).toBe('/fleet/trips?status=dispatched');
  });

  it('serializes the expenses filters', () => {
    expect(VIEW_PATHS.expenses({
      status: 'submitted', category: 'fuel', driver_id: 'd1',
      from: '2026-07-01', to: '2026-07-31',
    })).toBe('/fleet/expenses?status=submitted&category=fuel&driver_id=d1&from=2026-07-01&to=2026-07-31');
  });

  it('drops empty/undefined values and tolerates a missing params object', () => {
    expect(VIEW_PATHS.loads({ status: '' })).toBe('/fleet/loads');
    expect(VIEW_PATHS.expenses({ status: undefined, category: 'fuel' }))
      .toBe('/fleet/expenses?category=fuel');
    expect(VIEW_PATHS.loads()).toBe('/fleet/loads');
    expect(VIEW_PATHS.documents()).toBe('/fleet/documents');
  });
});
