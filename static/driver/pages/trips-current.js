import { clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { renderTripCard } from '../components/trip-card.js';

export async function renderCurrentPane(container) {
  container.replaceChildren();
  const spinner = document.createElement('div');
  spinner.className = 'trips-loading';
  const s = document.createElement('div');
  s.className = 'spinner';
  spinner.appendChild(s);
  container.appendChild(spinner);

  try {
    const data = await apiFetch('/trips?tab=current');
    container.replaceChildren();
    const items = (data && data.items) || [];
    if (items.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'trips-empty';
      empty.textContent = 'No current trips';
      container.appendChild(empty);
      return;
    }
    items.forEach(t => container.appendChild(renderTripCard(t, 'current')));
  } catch (err) {
    if (err.status === 401) {
      clearAuth();
      window.location.replace('/driver');
      return;
    }
    container.replaceChildren();
    const e = document.createElement('div');
    e.className = 'trips-error';
    e.textContent = err.message || 'Failed to load trips';
    container.appendChild(e);
  }
}
