import { toNaiveDateTime } from './load-form-payload.js';

export { toNaiveDateTime };

const TRIP_STOP_TYPES = [
  'origin', 'fuel', 'pickup', 'delivery', 'relay',
  'empty_move', 'maintenance', 'terminal',
];

export function tripStopTypes() {
  return [...TRIP_STOP_TYPES];
}

function str(v) { return (v ?? '').toString().trim(); }
function numOrUndef(v) {
  const s = str(v); if (s === '') return undefined;
  const n = Number(s); return Number.isNaN(n) ? undefined : n;
}
function intOrUndef(v) {
  const n = numOrUndef(v); return n === undefined ? undefined : Math.trunc(n);
}
function setIf(obj, key, val) { if (val !== undefined && val !== '') obj[key] = val; }

function buildStop(raw, index, errors) {
  const stop = {
    sequence: index + 1,
    stop_type: raw.stop_type || 'pickup',
    timezone: str(raw.timezone),
    scheduled_arrive: toNaiveDateTime(str(raw.scheduled_arrive)),
  };
  if (!stop.timezone) errors.push(`Stop ${index + 1}: timezone is required`);
  if (!stop.scheduled_arrive) errors.push(`Stop ${index + 1}: scheduled arrival is required`);

  const facilityId = str(raw.facility_id);
  if (facilityId) {
    stop.facility_id = facilityId;
  } else {
    const name = str(raw.name), addr = str(raw.address);
    if (!name || !addr) {
      errors.push(`Stop ${index + 1}: pick a facility or enter both name and address`);
    } else {
      stop.name = name;
      stop.address = addr;
    }
  }
  setIf(stop, 'scheduled_arrive_end', toNaiveDateTime(str(raw.scheduled_arrive_end)));
  setIf(stop, 'expected_dwell_minutes', intOrUndef(raw.expected_dwell_minutes));
  setIf(stop, 'detention_free_minutes', intOrUndef(raw.detention_free_minutes));
  setIf(stop, 'detention_grace_minutes', intOrUndef(raw.detention_grace_minutes));
  setIf(stop, 'notes', str(raw.notes));
  return stop;
}

export function buildCreateTripPayload({ mode = 'load', top = {}, driver_id, truck_id, trailer_ids, stops = [] }) {
  const errors = [];
  const payload = {};

  setIf(payload, 'trip_number', str(top.trip_number));
  setIf(payload, 'notes', str(top.notes));
  setIf(payload, 'sequence', intOrUndef(top.sequence));

  const loadId = str(top.load_id);
  if (mode === 'load' && loadId) payload.load_id = loadId;

  setIf(payload, 'driver_id', str(driver_id));
  setIf(payload, 'truck_id', str(truck_id));

  const trailers = (trailer_ids || []).map(str).filter(Boolean);
  if (trailers.length) payload.trailer_ids = trailers;

  if (mode === 'free') {
    if (!stops.length) {
      errors.push('At least one stop is required for a free-standing trip');
    } else {
      payload.stops = stops.map((s, i) => buildStop(s, i, errors));
    }
  } else if (stops && stops.length) {
    payload.stops = stops.map((s, i) => buildStop(s, i, errors));
  }

  return { payload, errors };
}

export function buildTripPatch({ notes = '', loaded_rate_per_mile, deadhead_rate_per_mile, extra_stop_fee, detention_rate_per_hour, free_dwell_minutes } = {}) {
  const errors = [];
  const payload = {};

  // notes is plain Option<String> on the backend (not double_option), so omit-when-blank; cannot null-clear unlike rate fields
  const notesStr = str(notes);
  if (notesStr) payload.notes = notesStr;

  function applyRate(key, field, coerce) {
    if (!field) return;
    if (field.cleared) {
      payload[key] = null;
    } else {
      const v = coerce(field.value);
      if (v !== undefined) payload[key] = v;
    }
  }

  applyRate('loaded_rate_per_mile', loaded_rate_per_mile, numOrUndef);
  applyRate('deadhead_rate_per_mile', deadhead_rate_per_mile, numOrUndef);
  applyRate('extra_stop_fee', extra_stop_fee, numOrUndef);
  applyRate('detention_rate_per_hour', detention_rate_per_hour, numOrUndef);
  applyRate('free_dwell_minutes', free_dwell_minutes, intOrUndef);

  return { payload, errors };
}
