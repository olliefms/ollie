import { isAuthenticated, clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';

export async function renderStopDetail(container, tripId, seq) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  // Clear container
  container.innerHTML = '';

  // Page layout
  const page = document.createElement('div');
  page.className = 'stop-detail-page';

  // Header with back button
  const header = document.createElement('div');
  header.className = 'stop-detail-header';

  const backBtn = document.createElement('button');
  backBtn.className = 'back-btn stop-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    window.location.href = `/driver/trips/${tripId}`;
  });

  const stopTitle = document.createElement('h1');
  stopTitle.className = 'stop-detail-title';
  stopTitle.textContent = '';

  header.appendChild(backBtn);
  header.appendChild(stopTitle);

  // Loading state
  const loadingEl = document.createElement('div');
  loadingEl.className = 'stop-detail-loading';
  const spinner = document.createElement('div');
  spinner.className = 'spinner';
  loadingEl.appendChild(spinner);

  page.appendChild(header);
  page.appendChild(loadingEl);
  container.appendChild(page);

  try {
    const data = await apiFetch(`/trips/${tripId}/stops/${seq}`);

    // Clear loading state
    loadingEl.remove();

    // Update header
    stopTitle.textContent = `Stop ${data.sequence}`;
    const stopTypeLabel = document.createElement('div');
    stopTypeLabel.className = 'stop-detail-type';
    stopTypeLabel.textContent = formatStopType(data.stop_type);
    header.appendChild(stopTypeLabel);

    // Facility info section
    const facilitySection = document.createElement('div');
    facilitySection.className = 'stop-detail-section';

    const facilityName = document.createElement('div');
    facilityName.className = 'stop-detail-row stop-detail-row--name';
    facilityName.textContent = data.facility_name || '—';
    facilitySection.appendChild(facilityName);

    if (data.address) {
      if (data.address.street) {
        const street = document.createElement('div');
        street.className = 'stop-detail-row';
        street.textContent = data.address.street;
        facilitySection.appendChild(street);
      }
      if (data.address.city) {
        const city = document.createElement('div');
        city.className = 'stop-detail-row';
        const cityLine = data.address.state ? `${data.address.city}, ${data.address.state}` : data.address.city;
        city.textContent = cityLine;
        if (data.address.zip) {
          city.textContent += ` ${data.address.zip}`;
        }
        facilitySection.appendChild(city);
      }
    }

    page.appendChild(facilitySection);

    // Scheduled section
    const scheduledSection = document.createElement('div');
    scheduledSection.className = 'stop-detail-section';

    const scheduledLabel = document.createElement('div');
    scheduledLabel.className = 'stop-detail-section-label';
    scheduledLabel.textContent = 'Scheduled';
    scheduledSection.appendChild(scheduledLabel);

    const arriveWindow = document.createElement('div');
    arriveWindow.className = 'stop-detail-row';
    const start = formatTimeShort(data.scheduled_arrive);
    const end = data.scheduled_arrive_end ? formatTimeShort(data.scheduled_arrive_end) : null;
    if (end) {
      arriveWindow.textContent = `${start} – ${end}`;
    } else {
      arriveWindow.textContent = start;
    }
    scheduledSection.appendChild(arriveWindow);

    if (data.expected_dwell_minutes) {
      const dwell = document.createElement('div');
      dwell.className = 'stop-detail-row';
      dwell.textContent = `${data.expected_dwell_minutes} min dwell`;
      scheduledSection.appendChild(dwell);
    }

    page.appendChild(scheduledSection);

    // Actual section
    const actualSection = document.createElement('div');
    actualSection.className = 'stop-detail-section';

    const actualLabel = document.createElement('div');
    actualLabel.className = 'stop-detail-section-label';
    actualLabel.textContent = 'Actual';
    actualSection.appendChild(actualLabel);

    const arrivedRow = document.createElement('div');
    arrivedRow.className = 'stop-detail-row';
    const arrivedText = document.createElement('span');
    arrivedText.className = 'stop-detail-actual-label';
    arrivedText.textContent = 'Arrived: ';
    const arrivedTime = document.createElement('span');
    arrivedTime.textContent = data.actual_arrive ? formatTimeShort(data.actual_arrive) : '—';
    arrivedRow.appendChild(arrivedText);
    arrivedRow.appendChild(arrivedTime);
    actualSection.appendChild(arrivedRow);

    const departedRow = document.createElement('div');
    departedRow.className = 'stop-detail-row';
    const departedText = document.createElement('span');
    departedText.className = 'stop-detail-actual-label';
    departedText.textContent = 'Departed: ';
    const departedTime = document.createElement('span');
    departedTime.textContent = data.actual_depart ? formatTimeShort(data.actual_depart) : '—';
    departedRow.appendChild(departedText);
    departedRow.appendChild(departedTime);
    actualSection.appendChild(departedRow);

    page.appendChild(actualSection);

    // Commodity section
    const commoditySection = document.createElement('div');
    commoditySection.className = 'stop-detail-section';

    const commodityRow = document.createElement('div');
    commodityRow.className = 'stop-detail-row';
    commodityRow.textContent = `${data.commodity} • ${formatWeight(data.weight_lbs)}`;
    commoditySection.appendChild(commodityRow);

    page.appendChild(commoditySection);

    // Notes section
    if (data.notes) {
      const notesSection = document.createElement('div');
      notesSection.className = 'stop-detail-section';

      const notesLabel = document.createElement('div');
      notesLabel.className = 'stop-detail-section-label';
      notesLabel.textContent = 'Notes:';
      notesSection.appendChild(notesLabel);

      const notesContent = document.createElement('div');
      notesContent.className = 'stop-detail-row stop-detail-notes';
      notesContent.textContent = data.notes;
      notesSection.appendChild(notesContent);

      page.appendChild(notesSection);
    }
  } catch (err) {
    if (err.status === 401) {
      clearAuth();
      window.location.replace('/driver');
      return;
    }

    loadingEl.remove();
    const errorEl = document.createElement('div');
    errorEl.className = 'stop-detail-error';
    errorEl.textContent = err.message || 'Failed to load stop';
    page.appendChild(errorEl);
  }
}

function formatStopType(type) {
  const labels = {
    'origin': 'ORIGIN',
    'destination': 'DESTINATION',
    'pickup': 'PICKUP',
    'dropoff': 'DROPOFF',
    'intermediate': 'STOP',
  };
  return labels[type] || type.toUpperCase();
}

function formatTimeShort(dateStr) {
  if (!dateStr) return '';
  const date = new Date(dateStr);
  return date.toLocaleDateString('en-US', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatWeight(lbs) {
  if (!lbs) return '0 lb';
  return lbs.toLocaleString() + ' lb';
}
