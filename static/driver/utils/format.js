export function formatStopType(type) {
  const labels = {
    'origin': 'ORIGIN',
    'fuel': 'FUEL',
    'pickup': 'PICKUP',
    'delivery': 'DELIVERY',
    'relay': 'RELAY',
    'empty_move': 'EMPTY MOVE',
    'maintenance': 'MAINTENANCE',
    'terminal': 'TERMINAL',
  };
  return labels[type] || type.toUpperCase();
}

export function formatWeight(lbs) {
  if (!lbs) return '0 lb';
  return lbs.toLocaleString() + ' lb';
}

export function formatStatus(status) {
  const labels = {
    'planned': 'Planned',
    'assigned': 'Assigned',
    'dispatched': 'Dispatched',
    'in_transit': 'In Transit',
    'delivered': 'Delivered',
    'completed': 'Completed',
    'cancelled': 'Cancelled',
  };
  return labels[status] || status;
}

// Format a UTC ISO string for display in a stop's local timezone.
// Falls back to browser locale if timezone is absent or unrecognized.
export function formatStopTime(dateStr, timezone) {
  if (!dateStr) return '—';
  const opts = {
    month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit',
  };
  if (timezone) {
    try {
      opts.timeZone = timezone;
      return new Date(dateStr).toLocaleString('en-US', opts);
    } catch {
      // fall through to locale default
    }
  }
  return new Date(dateStr).toLocaleString('en-US', opts);
}

export function formatShortTime(dateStr, timezone) {
  if (!dateStr) return '—';
  const opts = {
    month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit',
  };
  if (timezone) {
    try {
      opts.timeZone = timezone;
      return new Date(dateStr).toLocaleString('en-US', opts);
    } catch {
      // fall through
    }
  }
  return new Date(dateStr).toLocaleString('en-US', opts);
}
