import { isAuthenticated, clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { formatStopType, formatWeight, formatStatus, formatStopTime } from '../utils/format.js';
import { navigate } from '../app.js';

export async function renderTripDetail(container, tripId) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  // Clear container
  container.innerHTML = '';

  // Page layout
  const page = document.createElement('div');
  page.className = 'trip-detail-page';

  // Header with back button
  const header = document.createElement('div');
  header.className = 'trip-detail-header';

  const backBtn = document.createElement('button');
  backBtn.className = 'btn-ghost-back trip-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    navigate('/driver/trips');
  });

  const tripNumber = document.createElement('h1');
  tripNumber.className = 'trip-detail-number';
  tripNumber.textContent = '';

  header.appendChild(backBtn);
  header.appendChild(tripNumber);

  // Loading state
  const loadingEl = document.createElement('div');
  loadingEl.className = 'trip-detail-loading';
  const spinner = document.createElement('div');
  spinner.className = 'spinner';
  loadingEl.appendChild(spinner);

  page.appendChild(header);
  page.appendChild(loadingEl);
  container.appendChild(page);

  try {
    const data = await apiFetch(`/trips/${tripId}`);

    // Clear loading state
    loadingEl.remove();

    // Update header
    tripNumber.textContent = data.trip_number;
    const statusBadge = document.createElement('div');
    statusBadge.className = `badge badge--${data.status}`;
    statusBadge.textContent = formatStatus(data.status);
    header.appendChild(statusBadge);

    // Equipment row
    const equipmentSection = document.createElement('div');
    equipmentSection.className = 'trip-detail-section';

    const truckDiv = document.createElement('div');
    truckDiv.className = 'trip-detail-row';
    truckDiv.textContent = `Truck: ${data.truck_unit || '—'}`;
    equipmentSection.appendChild(truckDiv);

    if (data.trailer_units && data.trailer_units.length > 0) {
      const trailerDiv = document.createElement('div');
      trailerDiv.className = 'trip-detail-row';
      trailerDiv.textContent = `Trailer: ${data.trailer_units.join(', ')}`;
      equipmentSection.appendChild(trailerDiv);
    }

    page.appendChild(equipmentSection);

    // Load info row
    const loadSection = document.createElement('div');
    loadSection.className = 'trip-detail-section';

    if (data.load) {
      if (data.load.load_number) {
        const loadNumDiv = document.createElement('div');
        loadNumDiv.className = 'trip-detail-row';
        loadNumDiv.textContent = `Load #: ${data.load.load_number}`;
        loadSection.appendChild(loadNumDiv);
      }

      const refDiv = document.createElement('div');
      refDiv.className = 'trip-detail-row';
      refDiv.textContent = `Ref: ${data.load.customer_ref}`;
      loadSection.appendChild(refDiv);

      const commodityDiv = document.createElement('div');
      commodityDiv.className = 'trip-detail-row';
      commodityDiv.textContent = `${data.load.commodity} • ${formatWeight(data.load.weight_lbs)}`;
      loadSection.appendChild(commodityDiv);

      if (data.load.notes) {
        const notesDiv = document.createElement('div');
        notesDiv.className = 'trip-detail-row trip-detail-notes';
        notesDiv.textContent = data.load.notes;
        loadSection.appendChild(notesDiv);
      }
    } else {
      const noLoadDiv = document.createElement('div');
      noLoadDiv.className = 'trip-detail-row';
      noLoadDiv.textContent = 'No load assigned';
      loadSection.appendChild(noLoadDiv);
    }

    page.appendChild(loadSection);

    // Stop timeline
    const stopsSection = document.createElement('div');
    stopsSection.className = 'trip-detail-section trip-detail-stops';

    if (data.stops && data.stops.length > 0) {
      const stopTimeline = document.createElement('div');
      stopTimeline.className = 'stop-timeline';

      data.stops.forEach(stop => {
        const stopNode = renderStopNode(stop, tripId);
        stopTimeline.appendChild(stopNode);
      });

      stopsSection.appendChild(stopTimeline);
    } else {
      const emptyMsg = document.createElement('div');
      emptyMsg.className = 'trip-detail-empty';
      emptyMsg.textContent = 'No stops assigned yet.';
      stopsSection.appendChild(emptyMsg);
    }

    page.appendChild(stopsSection);
  } catch (err) {
    if (err.status === 401) {
      clearAuth();
      window.location.replace('/driver');
      return;
    }

    loadingEl.remove();
    const errorEl = document.createElement('div');
    errorEl.className = 'trip-detail-error';
    errorEl.textContent = err.message || 'Failed to load trip';
    page.appendChild(errorEl);
  }
}

function renderStopNode(stop, tripId) {
  const node = document.createElement('div');

  // Determine stop state
  let state = 'upcoming';
  if (stop.actual_depart) {
    state = 'completed';
  } else if (stop.actual_arrive) {
    state = 'current';
  }

  node.className = `stop-node stop-node--${state}`;

  const marker = document.createElement('div');
  marker.className = 'stop-node__marker';
  node.appendChild(marker);

  const content = document.createElement('div');
  content.className = 'stop-node__content';

  const stopTypeLabel = document.createElement('span');
  stopTypeLabel.className = 'stop-node__type';
  stopTypeLabel.textContent = formatStopType(stop.stop_type);

  const stopName = document.createElement('span');
  stopName.className = 'stop-node__name';
  stopName.textContent = stop.name;

  const title = document.createElement('div');
  title.className = 'stop-node__title';
  title.appendChild(stopTypeLabel);
  title.appendChild(document.createTextNode(' — '));
  title.appendChild(stopName);

  const time = document.createElement('div');
  time.className = 'stop-node__time';
  time.textContent = formatStopTime(stop.scheduled_arrive, stop.timezone);

  content.appendChild(title);
  content.appendChild(time);

  // Make clickable to navigate to stop detail
  node.addEventListener('click', () => {
    navigate(`/driver/trips/${tripId}/stops/${stop.sequence}`);
  });

  node.appendChild(content);
  return node;
}

