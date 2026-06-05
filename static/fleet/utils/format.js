export function escHtml(s) {
  if (!s) return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

export function badge(status) {
  if (!status) return '';
  const slug = status.toLowerCase().replace(/[^a-z0-9_]/g, '_');
  return `<span class="badge badge--${slug}">${escHtml(status)}</span>`;
}

export function shortId(id) {
  if (!id) return '—';
  return id.slice(0, 8);
}

export function fmtDate(isoStr) {
  if (!isoStr) return '—';
  try {
    return new Date(isoStr).toLocaleString();
  } catch {
    return isoStr;
  }
}

export function fmtArrivalWindow(start, end) {
  if (!start) return '—';
  if (!end) {
    try { return new Date(start).toLocaleString(); } catch { return start; }
  }
  try {
    const s = new Date(start);
    const e = new Date(end);
    const sameDay = s.toDateString() === e.toDateString();
    if (sameDay) {
      const sStr = s.toLocaleString();
      const eStr = e.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      return `${sStr}–${eStr}`;
    }
    return `${s.toLocaleString()} – ${e.toLocaleString()}`;
  } catch {
    return start;
  }
}

export function fmtBytes(n) {
  if (!n) return '—';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export function fmtUSD(n) {
  if (n === null || n === undefined) return '—';
  const sign = n < 0 ? '-' : '';
  const abs = Math.abs(n);
  return `${sign}$${abs.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

export function fmtMiles(n) {
  if (n === null || n === undefined) return '—';
  return `${n.toFixed(1)} mi`;
}

export function humanizeEventType(type) {
  const map = {
    'trip.assigned':     'Trip Assigned',
    'trip.unassigned':   'Trip Unassigned',
    'trip.dispatched':   'Trip Dispatched',
    'trip.undispatched': 'Trip Undispatched',
    'trip.in_transit':   'Trip In Transit',
    'trip.delivered':    'Trip Delivered',
    'trip_completed':    'Trip Completed',
    'trip.cancelled':    'Trip Cancelled',
    'stop.arrived':      'Stop Arrived',
    'stop.departed':     'Stop Departed',
    'stop.late':         'Stop Late',
    'check_call':        'Check Call',
    'driver_available':  'Driver Available',
    'truck_available':   'Truck Available',
    'trailer_available': 'Trailer Available',
  };
  return map[type] || String(type).replace(/[_.]/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
}
