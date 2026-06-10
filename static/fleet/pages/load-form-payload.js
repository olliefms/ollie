const PICKUP = ['pre_loaded', 'live_load', 'relay'];
const DELIVERY = ['live_unload', 'drop_and_hook', 'relay'];

export function serviceTypesFor(stopType) {
  return stopType === 'delivery' ? [...DELIVERY] : [...PICKUP];
}

export function toNaiveDateTime(v) {
  if (!v) return '';
  return /\dT\d{2}:\d{2}$/.test(v) ? `${v}:00` : v;
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
    service_type: raw.service_type,
    timezone: str(raw.timezone),
    scheduled_arrive: toNaiveDateTime(str(raw.scheduled_arrive)),
  };
  if (!stop.timezone) errors.push(`Stop ${index + 1}: timezone is required`);
  if (!stop.scheduled_arrive) errors.push(`Stop ${index + 1}: scheduled arrival is required`);

  const facilityId = str(raw.facility_id);
  if (facilityId) {
    stop.facility_id = facilityId;
  } else {
    const name = str(raw.facility_name), addr = str(raw.address);
    if (!name || !addr) {
      errors.push(`Stop ${index + 1}: pick a facility or enter both name and address`);
    } else {
      stop.facility_name = name;
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

export function buildLoadPayload({ top = {}, stops = [], rateItems = [] }) {
  const errors = [];
  const payload = {};

  if (!str(top.customer_name)) errors.push('Customer name is required');
  setIf(payload, 'customer_name', str(top.customer_name));
  setIf(payload, 'customer_ref', str(top.customer_ref));
  setIf(payload, 'load_number', str(top.load_number));
  setIf(payload, 'commodity', str(top.commodity));
  setIf(payload, 'notes', str(top.notes));
  setIf(payload, 'weight_lbs', numOrUndef(top.weight_lbs));
  setIf(payload, 'miles', numOrUndef(top.miles));

  const tags = (top.tags || []).map(str).filter(Boolean);
  if (tags.length) payload.tags = tags;

  if (!stops.length) errors.push('At least one stop is required');
  payload.stops = stops.map((s, i) => buildStop(s, i, errors));

  const rate = rateItems
    .map(r => ({ description: str(r.description), amount_usd: numOrUndef(r.amount_usd) }))
    .filter(r => r.description && r.amount_usd !== undefined);
  if (rate.length) payload.rate_items = rate;

  return { payload, errors };
}

export function applyResolutionChoices(payload, choices) {
  const stops = payload.stops.map((stop, i) => {
    const choice = choices[i];
    if (!choice) return stop;
    if (choice.facility_id) {
      const { facility_name, address, ...rest } = stop;
      return { ...rest, facility_id: choice.facility_id };
    }
    if (choice.force_new) return { ...stop, force_new_facility: true };
    return stop;
  });
  return { ...payload, stops };
}
