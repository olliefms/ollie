import { isAuthenticated, clearAuth, getToken } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { formatStopType, formatWeight, formatShortTime } from '../utils/format.js';
import { navigate } from '../app.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { nowInZone, convertNaive } from '../utils/time.js';

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
  const backBtn = document.createElement('button');
  backBtn.className = 'btn-ghost-back stop-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    if (history.length > 1) history.back();
    else navigate(`/driver/trips/${tripId}`);
  });

  const header = renderAppBar({ title: `Stop ${seq}`, right: backBtn });

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

    // Stop type subtitle
    const stopTypeLabel = document.createElement('div');
    stopTypeLabel.className = 'stop-detail-type';
    stopTypeLabel.textContent = formatStopType(data.stop_type);
    page.appendChild(stopTypeLabel);

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

    // Actual section (editable)
    const actualSection = renderActualSection(data, tripId, async () => {
      await renderStopDetail(container, tripId, seq);
    });
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

  page.appendChild(renderBottomNav('trips'));
}

function renderActualSection(stop, tripId, onChange) {
  const section = document.createElement('div');
  section.className = 'stop-detail-section';

  const label = document.createElement('div');
  label.className = 'stop-detail-section-label';
  label.textContent = 'Actual';
  section.appendChild(label);

  const arrivedRow = document.createElement('div');
  arrivedRow.className = 'stop-detail-row stop-actual-row';
  arrivedRow.appendChild(renderActualLine('Arrived', stop.actual_arrive, stop.timezone, async (newVal) => {
    await patchStop(tripId, stop.sequence, { actual_arrive: newVal });
    onChange();
  }, !stop.actual_arrive));
  section.appendChild(arrivedRow);

  const departedRow = document.createElement('div');
  departedRow.className = 'stop-detail-row stop-actual-row';
  if (stop.actual_arrive) {
    departedRow.appendChild(renderActualLine('Departed', stop.actual_depart, stop.timezone, async (newVal) => {
      await patchStop(tripId, stop.sequence, { actual_depart: newVal });
      onChange();
    }, !stop.actual_depart));
  } else {
    const placeholder = document.createElement('span');
    placeholder.className = 'stop-detail-actual-disabled';
    placeholder.textContent = 'Departed: arrive first';
    departedRow.appendChild(placeholder);
  }
  section.appendChild(departedRow);

  return section;
}

function renderActualLine(label, currentValue, tz, onSave, primary) {
  const wrap = document.createElement('span');
  const lbl = document.createElement('span');
  lbl.className = 'stop-detail-actual-label';
  lbl.textContent = `${label}: `;
  wrap.appendChild(lbl);

  if (!currentValue && primary) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'btn btn-primary stop-actual-action';
    btn.textContent = `${label} now`;
    btn.addEventListener('click', async () => {
      btn.disabled = true;
      try { await onSave(nowInZone(tz)); }
      catch (err) { btn.disabled = false; alert(err.message || 'Save failed'); }
    });
    wrap.appendChild(btn);
  } else if (currentValue) {
    const time = document.createElement('span');
    time.textContent = formatShortTime(currentValue, tz);
    wrap.appendChild(time);
    const edit = document.createElement('button');
    edit.type = 'button';
    edit.className = 'stop-actual-edit';
    edit.textContent = '✎';
    edit.setAttribute('aria-label', `Edit ${label}`);
    const dt = document.createElement('input');
    dt.type = 'datetime-local';
    dt.className = 'stop-actual-editor';
    const deviceTz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const display = convertNaive(currentValue, tz, deviceTz);
    dt.value = display ? display.slice(0, 16) : '';
    dt.addEventListener('change', async () => {
      if (!dt.value) return;
      const newVal = dt.value + ':00';
      const back = convertNaive(newVal, deviceTz, tz);
      try { await onSave(back); }
      catch (err) { alert(err.message || 'Save failed'); }
    });
    edit.addEventListener('click', () => {
      if (typeof dt.showPicker === 'function') dt.showPicker();
      else dt.focus();
    });
    wrap.appendChild(edit);
    wrap.appendChild(dt);
  }
  return wrap;
}

async function patchStop(tripId, seq, body) {
  const r = await fetch(`/driver/api/v1/trips/${tripId}/stops/${seq}`, {
    method: 'PATCH',
    headers: {
      'Authorization': `Bearer ${getToken()}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!r.ok) {
    const text = await r.text();
    throw new Error(text || `Save failed: ${r.status}`);
  }
  return r.json();
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


