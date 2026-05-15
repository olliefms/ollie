import { isAuthenticated, clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { formatStopType, formatWeight, formatShortTime } from '../utils/format.js';
import { navigate } from '../app.js';

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
  backBtn.className = 'btn-ghost-back stop-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    navigate(`/driver/trips/${tripId}`);
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
        street.className = 'stop-detail-row stop-detail-row--copyable';
        street.textContent = data.address.street;
        street.title = 'Tap to copy address';
        street.addEventListener('click', () => copyAddress(street, data.address.street));
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

    if (data.facility_name || data.address) {
      page.appendChild(facilitySection);
    }

    // Contacts section (rendered after address, before scheduled)
    if (data.contacts && data.contacts.length > 0) {
      const contactsSection = document.createElement('div');
      contactsSection.className = 'stop-detail-section';

      const contactsLabel = document.createElement('div');
      contactsLabel.className = 'stop-detail-section-label';
      contactsLabel.textContent = 'Contacts';
      contactsSection.appendChild(contactsLabel);

      data.contacts.forEach(contact => {
        const contactRow = document.createElement('div');
        contactRow.className = 'stop-detail-row';

        const nameEl = document.createElement('strong');
        nameEl.textContent = contact.name;
        contactRow.appendChild(nameEl);

        if (contact.title) {
          const titleEl = document.createElement('span');
          titleEl.className = 'contact-title';
          titleEl.textContent = ` — ${contact.title}`;
          contactRow.appendChild(titleEl);
        }

        if (contact.phone) {
          const phoneRow = document.createElement('div');
          phoneRow.className = 'stop-detail-row';
          const phoneLink = document.createElement('a');
          phoneLink.href = `tel:${contact.phone}`;
          phoneLink.textContent = contact.phone;
          phoneRow.appendChild(phoneLink);
          contactsSection.appendChild(contactRow);
          contactsSection.appendChild(phoneRow);
        } else {
          contactsSection.appendChild(contactRow);
        }
      });

      page.appendChild(contactsSection);
    }

    // Scheduled section
    if (data.scheduled_arrive || data.expected_dwell_minutes) {
      const scheduledSection = document.createElement('div');
      scheduledSection.className = 'stop-detail-section';

      const scheduledLabel = document.createElement('div');
      scheduledLabel.className = 'stop-detail-section-label';
      scheduledLabel.textContent = 'Scheduled';
      scheduledSection.appendChild(scheduledLabel);

      if (data.scheduled_arrive) {
        const arriveWindow = document.createElement('div');
        arriveWindow.className = 'stop-detail-row';
        const start = formatShortTime(data.scheduled_arrive, data.timezone);
        const end = data.scheduled_arrive_end ? formatShortTime(data.scheduled_arrive_end, data.timezone) : null;
        if (end) {
          arriveWindow.textContent = `${start} – ${end}`;
        } else {
          arriveWindow.textContent = start;
        }
        scheduledSection.appendChild(arriveWindow);
      }

      if (data.expected_dwell_minutes) {
        const dwell = document.createElement('div');
        dwell.className = 'stop-detail-row';
        dwell.textContent = `${data.expected_dwell_minutes} min dwell`;
        scheduledSection.appendChild(dwell);
      }

      page.appendChild(scheduledSection);
    }

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
    arrivedTime.textContent = data.actual_arrive ? formatShortTime(data.actual_arrive, data.timezone) : '—';
    arrivedRow.appendChild(arrivedText);
    arrivedRow.appendChild(arrivedTime);
    actualSection.appendChild(arrivedRow);

    const departedRow = document.createElement('div');
    departedRow.className = 'stop-detail-row';
    const departedText = document.createElement('span');
    departedText.className = 'stop-detail-actual-label';
    departedText.textContent = 'Departed: ';
    const departedTime = document.createElement('span');
    departedTime.textContent = data.actual_depart ? formatShortTime(data.actual_depart, data.timezone) : '—';
    departedRow.appendChild(departedText);
    departedRow.appendChild(departedTime);
    actualSection.appendChild(departedRow);

    page.appendChild(actualSection);

    // Commodity section
    const commoditySection = document.createElement('div');
    commoditySection.className = 'stop-detail-section';

    const commodityLabel = document.createElement('div');
    commodityLabel.className = 'stop-detail-section-label';
    commodityLabel.textContent = 'Commodity';
    commoditySection.appendChild(commodityLabel);

    const commodityRow = document.createElement('div');
    commodityRow.className = 'stop-detail-row';
    commodityRow.textContent = `${data.commodity || '—'} • ${formatWeight(data.weight_lbs)}`;
    commoditySection.appendChild(commodityRow);

    page.appendChild(commoditySection);

    // Notes section
    if (data.notes) {
      const notesSection = document.createElement('div');
      notesSection.className = 'stop-detail-section';

      const notesLabel = document.createElement('div');
      notesLabel.className = 'stop-detail-section-label';
      notesLabel.textContent = 'Notes';
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

function copyAddress(el, text) {
  const original = el.textContent;
  if (navigator.clipboard) {
    navigator.clipboard.writeText(text).then(() => {
      el.textContent = 'Copied!';
      el.classList.add('stop-detail-row--copied');
      setTimeout(() => {
        el.textContent = original;
        el.classList.remove('stop-detail-row--copied');
      }, 1500);
    }).catch(() => selectText(el));
  } else {
    selectText(el);
  }
}

function selectText(el) {
  const range = document.createRange();
  range.selectNodeContents(el);
  const sel = window.getSelection();
  sel.removeAllRanges();
  sel.addRange(range);
}


