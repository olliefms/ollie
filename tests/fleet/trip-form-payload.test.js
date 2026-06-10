import { describe, it, expect } from 'vitest';
import {
  tripStopTypes,
  buildCreateTripPayload,
  buildTripPatch,
} from '../../static/fleet/pages/trip-form-payload.js';
import { toNaiveDateTime } from '../../static/fleet/pages/load-form-payload.js';

// ── tripStopTypes ──────────────────────────────────────────────────────────────

describe('tripStopTypes', () => {
  it('returns the exact backend variants in order', () => {
    expect(tripStopTypes()).toEqual([
      'origin', 'fuel', 'pickup', 'delivery', 'relay',
      'empty_move', 'maintenance', 'terminal',
    ]);
  });

  it('returns a new array each call (no shared reference)', () => {
    const a = tripStopTypes();
    const b = tripStopTypes();
    expect(a).not.toBe(b);
  });
});

// ── toNaiveDateTime re-export agrees with load-form-payload ───────────────────

describe('toNaiveDateTime import agreement', () => {
  it('same function: appends :00 to HH:MM', () => {
    expect(toNaiveDateTime('2026-06-10T08:00')).toBe('2026-06-10T08:00:00');
  });

  it('same function: passes through value that already has seconds', () => {
    expect(toNaiveDateTime('2026-06-10T08:00:30')).toBe('2026-06-10T08:00:30');
  });

  it('same function: returns empty for blank input', () => {
    expect(toNaiveDateTime('')).toBe('');
  });
});

// ── helpers / base fixtures ───────────────────────────────────────────────────

const baseStop = {
  stop_type: 'pickup',
  timezone: 'America/Chicago',
  scheduled_arrive: '2026-06-10T08:00',
  facility_id: 'fac-1',
};

const baseLoadState = {
  mode: 'load',
  top: { load_id: 'load-uuid-1', trip_number: 'T-001', notes: 'test note' },
  driver_id: 'drv-1',
  truck_id: 'trk-1',
  trailer_ids: ['trl-1'],
};

const baseFreeState = {
  mode: 'free',
  top: { trip_number: 'T-002' },
  stops: [baseStop],
};

// ── buildCreateTripPayload — load mode ────────────────────────────────────────

describe('buildCreateTripPayload — load mode', () => {
  it('omits stops when load_id is set and no custom stops given (backend derives)', () => {
    const { payload, errors } = buildCreateTripPayload(baseLoadState);
    expect(errors).toEqual([]);
    expect(payload.load_id).toBe('load-uuid-1');
    expect('stops' in payload).toBe(false);
  });

  it('includes load_id in payload', () => {
    const { payload } = buildCreateTripPayload(baseLoadState);
    expect(payload.load_id).toBe('load-uuid-1');
  });

  it('includes optional top fields when present', () => {
    const { payload } = buildCreateTripPayload(baseLoadState);
    expect(payload.trip_number).toBe('T-001');
    expect(payload.notes).toBe('test note');
  });

  it('omits blank optional top fields', () => {
    const state = { ...baseLoadState, top: { load_id: 'load-uuid-1', trip_number: '', notes: '' } };
    const { payload } = buildCreateTripPayload(state);
    expect('trip_number' in payload).toBe(false);
    expect('notes' in payload).toBe(false);
  });

  it('includes custom stops when provided in load mode', () => {
    const state = { ...baseLoadState, stops: [baseStop] };
    const { payload } = buildCreateTripPayload(state);
    expect(Array.isArray(payload.stops)).toBe(true);
    expect(payload.stops.length).toBe(1);
  });

  it('does NOT set load_id when load_id is blank in load mode', () => {
    const state = { mode: 'load', top: { load_id: '' }, stops: [baseStop] };
    const { payload } = buildCreateTripPayload(state);
    expect('load_id' in payload).toBe(false);
  });

  it('coerces sequence to int and omits when blank', () => {
    const state = { ...baseLoadState, top: { ...baseLoadState.top, sequence: '3' } };
    const { payload } = buildCreateTripPayload(state);
    expect(payload.sequence).toBe(3);
  });

  it('omits sequence when blank', () => {
    const state = { ...baseLoadState, top: { ...baseLoadState.top, sequence: '' } };
    const { payload } = buildCreateTripPayload(state);
    expect('sequence' in payload).toBe(false);
  });
});

// ── buildCreateTripPayload — free mode ────────────────────────────────────────

describe('buildCreateTripPayload — free mode', () => {
  it('errors when no stops provided', () => {
    const state = { mode: 'free', top: {}, stops: [] };
    const { errors } = buildCreateTripPayload(state);
    expect(errors).toContain('At least one stop is required for a free-standing trip');
  });

  it('errors when stops array is absent', () => {
    const state = { mode: 'free', top: {} };
    const { errors } = buildCreateTripPayload(state);
    expect(errors).toContain('At least one stop is required for a free-standing trip');
  });

  it('does NOT set load_id in free mode', () => {
    const { payload } = buildCreateTripPayload(baseFreeState);
    expect('load_id' in payload).toBe(false);
  });

  it('includes stops in free mode', () => {
    const { payload, errors } = buildCreateTripPayload(baseFreeState);
    expect(errors).toEqual([]);
    expect(payload.stops.length).toBe(1);
  });
});

// ── buildCreateTripPayload — resource coercion ────────────────────────────────

describe('buildCreateTripPayload — resource coercion', () => {
  it('includes driver_id when set', () => {
    const { payload } = buildCreateTripPayload(baseLoadState);
    expect(payload.driver_id).toBe('drv-1');
  });

  it('omits driver_id when blank', () => {
    const state = { ...baseLoadState, driver_id: '' };
    const { payload } = buildCreateTripPayload(state);
    expect('driver_id' in payload).toBe(false);
  });

  it('omits truck_id when blank', () => {
    const state = { ...baseLoadState, truck_id: '' };
    const { payload } = buildCreateTripPayload(state);
    expect('truck_id' in payload).toBe(false);
  });

  it('includes non-blank trailer_ids', () => {
    const { payload } = buildCreateTripPayload(baseLoadState);
    expect(payload.trailer_ids).toEqual(['trl-1']);
  });

  it('omits trailer_ids when all blank', () => {
    const state = { ...baseLoadState, trailer_ids: ['', '  '] };
    const { payload } = buildCreateTripPayload(state);
    expect('trailer_ids' in payload).toBe(false);
  });

  it('omits trailer_ids when absent', () => {
    const state = { ...baseLoadState, trailer_ids: undefined };
    const { payload } = buildCreateTripPayload(state);
    expect('trailer_ids' in payload).toBe(false);
  });

  it('filters blank entries from trailer_ids', () => {
    const state = { ...baseLoadState, trailer_ids: ['trl-1', '', 'trl-2'] };
    const { payload } = buildCreateTripPayload(state);
    expect(payload.trailer_ids).toEqual(['trl-1', 'trl-2']);
  });
});

// ── buildCreateTripPayload — stop validation and normalization ─────────────────

describe('buildCreateTripPayload — stop numbering and datetime normalization', () => {
  const stop2 = { ...baseStop, stop_type: 'delivery', scheduled_arrive: '2026-06-10T12:00' };

  it('numbers stops 1-based by row order', () => {
    const state = { ...baseFreeState, stops: [baseStop, stop2] };
    const { payload } = buildCreateTripPayload(state);
    expect(payload.stops[0].sequence).toBe(1);
    expect(payload.stops[1].sequence).toBe(2);
  });

  it('normalizes scheduled_arrive with toNaiveDateTime', () => {
    const { payload } = buildCreateTripPayload(baseFreeState);
    expect(payload.stops[0].scheduled_arrive).toBe('2026-06-10T08:00:00');
  });

  it('normalizes scheduled_arrive_end when provided', () => {
    const stop = { ...baseStop, scheduled_arrive_end: '2026-06-10T10:00' };
    const { payload } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(payload.stops[0].scheduled_arrive_end).toBe('2026-06-10T10:00:00');
  });

  it('omits scheduled_arrive_end when blank', () => {
    const stop = { ...baseStop, scheduled_arrive_end: '' };
    const { payload } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect('scheduled_arrive_end' in payload.stops[0]).toBe(false);
  });

  it('errors when timezone is missing', () => {
    const stop = { ...baseStop, timezone: '' };
    const { errors } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(errors.some(e => /timezone/i.test(e))).toBe(true);
  });

  it('errors when scheduled_arrive is missing', () => {
    const stop = { ...baseStop, scheduled_arrive: '' };
    const { errors } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(errors.some(e => /scheduled arrival/i.test(e))).toBe(true);
  });

  it('uses facility_id when set', () => {
    const { payload } = buildCreateTripPayload(baseFreeState);
    expect(payload.stops[0].facility_id).toBe('fac-1');
    expect('name' in payload.stops[0]).toBe(false);
    expect('address' in payload.stops[0]).toBe(false);
  });

  it('uses name+address when no facility_id', () => {
    const stop = { ...baseStop, facility_id: '', name: 'Warehouse A', address: '123 Main St' };
    const { payload, errors } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(errors).toEqual([]);
    expect(payload.stops[0].facility_id).toBeUndefined();
    expect(payload.stops[0].name).toBe('Warehouse A');
    expect(payload.stops[0].address).toBe('123 Main St');
  });

  it('errors when neither facility_id nor name+address provided', () => {
    const stop = { ...baseStop, facility_id: '', name: '', address: '' };
    const { errors } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(errors.some(e => /facility/i.test(e))).toBe(true);
  });

  it('coerces integer optional fields on stops', () => {
    const stop = {
      ...baseStop,
      expected_dwell_minutes: '45',
      detention_free_minutes: '30',
      detention_grace_minutes: '15',
    };
    const { payload } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(payload.stops[0].expected_dwell_minutes).toBe(45);
    expect(payload.stops[0].detention_free_minutes).toBe(30);
    expect(payload.stops[0].detention_grace_minutes).toBe(15);
  });

  it('omits blank integer optional fields on stops', () => {
    const stop = { ...baseStop, expected_dwell_minutes: '', detention_free_minutes: '' };
    const { payload } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect('expected_dwell_minutes' in payload.stops[0]).toBe(false);
    expect('detention_free_minutes' in payload.stops[0]).toBe(false);
  });

  it('passes stop notes through', () => {
    const stop = { ...baseStop, notes: 'call ahead' };
    const { payload } = buildCreateTripPayload({ ...baseFreeState, stops: [stop] });
    expect(payload.stops[0].notes).toBe('call ahead');
  });
});

// ── buildTripPatch ─────────────────────────────────────────────────────────────

describe('buildTripPatch — tri-state override semantics', () => {
  const emptyState = {
    notes: '',
    loaded_rate_per_mile: { value: '', cleared: false },
    deadhead_rate_per_mile: { value: '', cleared: false },
    extra_stop_fee: { value: '', cleared: false },
    detention_rate_per_hour: { value: '', cleared: false },
    free_dwell_minutes: { value: '', cleared: false },
  };

  it('omits all rate fields when untouched (value="" cleared=false)', () => {
    const { payload, errors } = buildTripPatch(emptyState);
    expect(errors).toEqual([]);
    expect('loaded_rate_per_mile' in payload).toBe(false);
    expect('deadhead_rate_per_mile' in payload).toBe(false);
    expect('extra_stop_fee' in payload).toBe(false);
    expect('detention_rate_per_hour' in payload).toBe(false);
    expect('free_dwell_minutes' in payload).toBe(false);
  });

  it('sends null when cleared=true (clear to inherited)', () => {
    const state = {
      ...emptyState,
      loaded_rate_per_mile: { value: '', cleared: true },
      extra_stop_fee: { value: '', cleared: true },
    };
    const { payload } = buildTripPatch(state);
    expect(payload.loaded_rate_per_mile).toBeNull();
    expect(payload.extra_stop_fee).toBeNull();
    expect('deadhead_rate_per_mile' in payload).toBe(false);
  });

  it('sends coerced number when value is set', () => {
    const state = {
      ...emptyState,
      loaded_rate_per_mile: { value: '2.50', cleared: false },
      deadhead_rate_per_mile: { value: '1.75', cleared: false },
    };
    const { payload } = buildTripPatch(state);
    expect(payload.loaded_rate_per_mile).toBe(2.5);
    expect(payload.deadhead_rate_per_mile).toBe(1.75);
  });

  it('coerces free_dwell_minutes to integer', () => {
    const state = { ...emptyState, free_dwell_minutes: { value: '120.9', cleared: false } };
    const { payload } = buildTripPatch(state);
    expect(payload.free_dwell_minutes).toBe(120);
  });

  it('sends null for free_dwell_minutes when cleared', () => {
    const state = { ...emptyState, free_dwell_minutes: { value: '', cleared: true } };
    const { payload } = buildTripPatch(state);
    expect(payload.free_dwell_minutes).toBeNull();
  });

  it('omits notes when blank', () => {
    const { payload } = buildTripPatch({ ...emptyState, notes: '' });
    expect('notes' in payload).toBe(false);
  });

  it('includes notes when set', () => {
    const state = { ...emptyState, notes: '  trimmed  ' };
    const { payload } = buildTripPatch(state);
    expect(payload.notes).toBe('trimmed');
  });

  it('never emits driver/truck/stop/mileage fields', () => {
    const state = {
      ...emptyState,
      driver_id: 'drv-1',
      truck_id: 'trk-1',
      stops: [baseStop],
      loaded_miles: '100',
    };
    const { payload } = buildTripPatch(state);
    expect('driver_id' in payload).toBe(false);
    expect('truck_id' in payload).toBe(false);
    expect('stops' in payload).toBe(false);
    expect('loaded_miles' in payload).toBe(false);
  });

  it('handles all three states simultaneously', () => {
    const state = {
      ...emptyState,
      loaded_rate_per_mile: { value: '3.00', cleared: false },    // set
      deadhead_rate_per_mile: { value: '', cleared: true },         // clear
      extra_stop_fee: { value: '', cleared: false },                // omit
    };
    const { payload } = buildTripPatch(state);
    expect(payload.loaded_rate_per_mile).toBe(3);
    expect(payload.deadhead_rate_per_mile).toBeNull();
    expect('extra_stop_fee' in payload).toBe(false);
  });

  it('returns no errors for a valid patch', () => {
    const state = {
      ...emptyState,
      loaded_rate_per_mile: { value: '2.00', cleared: false },
    };
    const { errors } = buildTripPatch(state);
    expect(errors).toEqual([]);
  });
});
