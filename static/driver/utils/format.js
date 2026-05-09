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
