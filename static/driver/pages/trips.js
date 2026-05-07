import { isAuthenticated } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';

export async function renderTrips(container) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  // Clear container
  container.innerHTML = '';

  // Page layout
  const page = document.createElement('div');
  page.className = 'trips-page';

  // Header
  const header = document.createElement('div');
  header.className = 'trips-header';
  const h1 = document.createElement('h1');
  h1.textContent = 'My Trips';
  header.appendChild(h1);

  // Tab bar
  const tabBar = document.createElement('div');
  tabBar.className = 'tab-bar';
  const tabs = [
    { id: 'past', label: 'Past' },
    { id: 'current', label: 'Current' },
    { id: 'upcoming', label: 'Upcoming' },
  ];
  let activeTab = 'current';
  const tabEls = {};

  tabs.forEach(tab => {
    const tabBtn = document.createElement('button');
    tabBtn.className = 'tab';
    if (tab.id === activeTab) tabBtn.classList.add('tab--active');
    tabBtn.textContent = tab.label;
    tabBtn.addEventListener('click', async () => {
      activeTab = tab.id;
      Object.values(tabEls).forEach(el => el.classList.remove('tab--active'));
      tabBtn.classList.add('tab--active');
      await loadTrips(activeTab);
    });
    tabBar.appendChild(tabBtn);
    tabEls[tab.id] = tabBtn;
  });

  // Trip list
  const tripList = document.createElement('div');
  tripList.className = 'trip-list';
  tripList.id = 'trip-list';

  page.appendChild(header);
  page.appendChild(tabBar);
  page.appendChild(tripList);
  container.appendChild(page);

  // Load initial trips
  async function loadTrips(tab) {
    const listEl = document.getElementById('trip-list');
    listEl.innerHTML = '';

    // Loading state
    const loadingEl = document.createElement('div');
    loadingEl.className = 'trips-loading';
    const spinner = document.createElement('div');
    spinner.className = 'spinner';
    loadingEl.appendChild(spinner);
    listEl.appendChild(loadingEl);

    try {
      const data = await apiFetch(`/trips?tab=${tab}`);
      listEl.innerHTML = '';

      if (!data.items || data.items.length === 0) {
        const emptyEl = document.createElement('div');
        emptyEl.className = 'trips-empty';
        emptyEl.textContent = tab === 'current' ? 'No current trips' : `No ${tab} trips`;
        listEl.appendChild(emptyEl);
        return;
      }

      data.items.forEach(trip => {
        const card = renderTripCard(trip, tab);
        listEl.appendChild(card);
      });
    } catch (err) {
      listEl.innerHTML = '';
      const errorEl = document.createElement('div');
      errorEl.className = 'trips-error';
      errorEl.textContent = err.message || 'Failed to load trips';
      listEl.appendChild(errorEl);
    }
  }

  function renderTripCard(trip, tab) {
    const card = document.createElement('div');
    card.className = 'trip-card';
    card.style.cursor = 'pointer';
    card.addEventListener('click', () => {
      window.location.href = `/driver/trips/${trip.id}`;
    });

    // Header with trip number and status
    const cardHeader = document.createElement('div');
    cardHeader.className = 'trip-card__header';

    const tripNum = document.createElement('div');
    tripNum.className = 'trip-card__number';
    tripNum.textContent = trip.trip_number;

    const status = document.createElement('div');
    status.className = `badge badge--${trip.status}`;
    status.textContent = formatStatus(trip.status);

    cardHeader.appendChild(tripNum);
    cardHeader.appendChild(status);
    card.appendChild(cardHeader);

    // Route info
    const route = document.createElement('div');
    route.className = 'trip-card__route';
    const arrow = document.createTextNode(' → ');
    route.appendChild(document.createTextNode(trip.origin));
    route.appendChild(arrow);
    route.appendChild(document.createTextNode(trip.destination));
    card.appendChild(route);

    // Tab-specific content
    if (tab === 'current') {
      const progressWrapper = document.createElement('div');
      progressWrapper.className = 'trip-card__progress';

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

      progressWrapper.appendChild(bar);
      progressWrapper.appendChild(label);
      card.appendChild(progressWrapper);

      if (trip.stops_completed < trip.stop_count && trip.stops && trip.stops.length > 0) {
        const nextStop = trip.stops[trip.stops_completed];
        const nextStopEl = document.createElement('div');
        nextStopEl.className = 'trip-card__next-stop';
        nextStopEl.textContent = `Next: ${nextStop.location}`;
        card.appendChild(nextStopEl);
      }
    } else {
      // Past and Upcoming tabs show scheduled start date
      const date = document.createElement('div');
      date.className = 'trip-card__date';
      date.textContent = formatDate(trip.scheduled_start);
      card.appendChild(date);
    }

    return card;
  }

  function formatStatus(status) {
    const labels = {
      'in_transit': 'In Transit',
      'delivered': 'Delivered',
      'pending': 'Pending',
      'scheduled': 'Scheduled',
    };
    return labels[status] || status;
  }

  function formatDate(dateStr) {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    return date.toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // Initial load
  await loadTrips(activeTab);
}
