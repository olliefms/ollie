import { formatStatus } from '../utils/format.js';
import { formatDeliveredAt } from '../utils/week.js';
import { navigate } from '../app.js';

export function renderTripCard(trip, mode) {
  const card = document.createElement('div');
  card.className = 'trip-card';
  card.addEventListener('click', () => navigate(`/driver/trips/${trip.id}`));

  const header = document.createElement('div');
  header.className = 'trip-card__header';
  const num = document.createElement('div');
  num.className = 'trip-card__number';
  num.textContent = trip.trip_number;
  const status = document.createElement('div');
  status.className = `badge badge--${(trip.status || '').replace(/[^a-z0-9_]/g, '_')}`;
  status.textContent = formatStatus(trip.status);
  header.appendChild(num);
  header.appendChild(status);
  card.appendChild(header);

  const route = document.createElement('div');
  route.className = 'trip-card__route';
  route.appendChild(document.createTextNode(trip.origin || ''));
  route.appendChild(document.createTextNode(' → '));
  route.appendChild(document.createTextNode(trip.destination || ''));
  card.appendChild(route);

  if (mode === 'current') {
    const wrap = document.createElement('div');
    wrap.className = 'trip-card__progress';
    const bar = document.createElement('div');
    bar.className = 'progress-bar';
    const fill = document.createElement('div');
    fill.className = 'progress-bar__fill';
    const pct = trip.stop_count > 0 ? (trip.stops_completed / trip.stop_count) * 100 : 0;
    fill.style.width = `${pct}%`;
    bar.appendChild(fill);
    const label = document.createElement('span');
    label.className = 'progress-bar__label';
    label.textContent = `${trip.stops_completed} / ${trip.stop_count} stops`;
    wrap.appendChild(bar);
    wrap.appendChild(label);
    card.appendChild(wrap);
    if (trip.next_stop_name) {
      const ns = document.createElement('div');
      ns.className = 'trip-card__next-stop';
      ns.textContent = `Next: ${trip.next_stop_name}`;
      card.appendChild(ns);
    }
  } else if (mode === 'past') {
    if (trip.delivered_at) {
      const d = document.createElement('div');
      d.className = 'trip-card__date';
      d.textContent = formatDeliveredAt(trip.delivered_at, trip.delivered_tz);
      card.appendChild(d);
    }
  } else if (mode === 'upcoming') {
    if (trip.scheduled_start) {
      const d = document.createElement('div');
      d.className = 'trip-card__date';
      const date = new Date(trip.scheduled_start);
      d.textContent = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
      card.appendChild(d);
    }
  }

  return card;
}
