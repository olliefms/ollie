import { isAuthenticated, clearAuth, getToken } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { formatStopType, formatWeight, formatStatus, formatStopTime } from '../utils/format.js';
import { navigate } from '../app.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { pdfIcon, photoIcon } from '../components/icons.js';

export async function renderTripDetail(container, tripId) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  const renderToken = Symbol('trip-detail-render');
  container.__renderToken = renderToken;

  container.replaceChildren();

  let currentDriverId = null;
  try {
    const me = await apiFetch('/me');
    currentDriverId = me.id;
  } catch (_) { /* ignore */ }

  if (container.__renderToken !== renderToken) return;

  const page = document.createElement('div');
  page.className = 'trip-detail-page';

  const backBtn = document.createElement('button');
  backBtn.type = 'button';
  backBtn.className = 'btn-ghost-back trip-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    if (history.length > 1) history.back();
    else navigate('/driver/trips');
  });

  const appBar = renderAppBar({ title: 'Loading…', right: backBtn });
  page.appendChild(appBar);

  const loadingEl = document.createElement('div');
  loadingEl.className = 'trip-detail-loading';
  const spinner = document.createElement('div');
  spinner.className = 'spinner';
  loadingEl.appendChild(spinner);
  page.appendChild(loadingEl);

  container.appendChild(page);

  try {
    const data = await apiFetch(`/trips/${tripId}`);
    if (container.__renderToken !== renderToken) return;

    loadingEl.remove();

    const titleEl = appBar.querySelector('.app-bar__title');
    if (titleEl) titleEl.textContent = data.trip_number;

    const statusBadge = document.createElement('div');
    statusBadge.className = `badge badge--${data.status}`;
    statusBadge.textContent = formatStatus(data.status);
    const rightSlot = appBar.querySelector('.app-bar__right');
    if (rightSlot) {
      rightSlot.insertBefore(statusBadge, rightSlot.firstChild);
    } else {
      appBar.appendChild(statusBadge);
    }

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

    // Documents card
    const docsSection = document.createElement('section');
    docsSection.className = 'docs-card';

    const docsHeader = document.createElement('div');
    docsHeader.className = 'docs-card__header';
    const docsTitle = document.createElement('h2');
    docsTitle.textContent = 'Documents';
    const uploadBtn = document.createElement('button');
    uploadBtn.type = 'button';
    uploadBtn.className = 'btn btn-secondary docs-card__upload';
    uploadBtn.textContent = '+ Upload';
    docsHeader.appendChild(docsTitle);
    docsHeader.appendChild(uploadBtn);
    docsSection.appendChild(docsHeader);

    const docsList = document.createElement('div');
    docsList.className = 'docs-list';
    docsSection.appendChild(docsList);

    const fileInput = document.createElement('input');
    fileInput.type = 'file';
    fileInput.accept = 'image/*,application/pdf';
    fileInput.capture = 'environment';
    fileInput.style.display = 'none';
    docsSection.appendChild(fileInput);

    let pendingDoctype = 'other';

    uploadBtn.addEventListener('click', () => openDoctypeSheet(dt => {
      pendingDoctype = dt;
      fileInput.click();
    }));

    fileInput.addEventListener('change', async () => {
      const file = fileInput.files && fileInput.files[0];
      if (!file) return;
      const form = new FormData();
      form.append('file', file);
      form.append('doctype', pendingDoctype);
      try {
        const r = await fetch(`/driver/api/v1/trips/${tripId}/documents`, {
          method: 'POST',
          headers: { 'Authorization': `Bearer ${getToken()}` },
          body: form,
        });
        if (!r.ok) throw new Error(`Upload failed (${r.status})`);
        await refreshDocs();
      } catch (err) {
        alert(err.message || 'Upload failed');
      } finally {
        fileInput.value = '';
      }
    });

    async function refreshDocs() {
      try {
        const docs = await apiFetch(`/trips/${tripId}/documents`);
        docsList.replaceChildren();
        if (docs.length === 0) {
          const empty = document.createElement('div');
          empty.className = 'docs-empty';
          empty.textContent = 'No documents yet.';
          docsList.appendChild(empty);
          return;
        }
        docs.forEach(doc => docsList.appendChild(renderDocRow(doc)));
      } catch (e) { /* silent */ }
    }

    function renderDocRow(doc) {
      const row = document.createElement('div');
      row.className = 'docs-row';
      const iconWrap = document.createElement('span');
      iconWrap.className = 'docs-row__icon';
      iconWrap.appendChild(doc.mime_type === 'application/pdf' ? pdfIcon() : photoIcon());
      const meta = document.createElement('div');
      meta.className = 'docs-row__meta';
      const title = document.createElement('div');
      const doctypeTag = (doc.tags || []).find(t => t.startsWith('doctype:'));
      const dtypeLabel = doctypeTag ? doctypeTag.split(':')[1].toUpperCase() : '';
      title.textContent = dtypeLabel ? `${dtypeLabel} — ${doc.name}` : doc.name;
      const time = document.createElement('div');
      time.className = 'docs-row__time';
      time.textContent = new Date(doc.created_at).toLocaleString('en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
      meta.appendChild(title);
      meta.appendChild(time);
      row.appendChild(iconWrap);
      row.appendChild(meta);
      row.addEventListener('click', () => openDocPreview(doc));
      if (doc.uploaded_by === currentDriverId) {
        const kebab = document.createElement('button');
        kebab.type = 'button';
        kebab.className = 'docs-row__kebab';
        kebab.textContent = '⋯';
        kebab.setAttribute('aria-label', 'Delete document');
        kebab.addEventListener('click', async e => {
          e.stopPropagation();
          if (!confirm('Delete this document?')) return;
          await fetch(`/driver/api/v1/trips/${tripId}/documents/${doc.id}`, {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${getToken()}` },
          });
          await refreshDocs();
        });
        row.appendChild(kebab);
      }
      return row;
    }

    function openDocPreview(doc) {
      const overlay = document.createElement('div');
      overlay.className = 'doc-preview';
      const frame = document.createElement('iframe');
      frame.sandbox = '';
      frame.src = `/driver/api/v1/trips/${tripId}/documents/${doc.id}/content`;
      const close = document.createElement('button');
      close.type = 'button';
      close.className = 'doc-preview__close';
      close.textContent = '×';
      close.setAttribute('aria-label', 'Close preview');
      close.addEventListener('click', () => document.body.removeChild(overlay));
      overlay.appendChild(close);
      overlay.appendChild(frame);
      document.body.appendChild(overlay);
    }

    function openDoctypeSheet(onPick) {
      const sheet = document.createElement('div');
      sheet.className = 'sheet';
      const inner = document.createElement('div');
      inner.className = 'sheet__inner';
      const title = document.createElement('div');
      title.className = 'sheet__title';
      title.textContent = 'What kind of document?';
      inner.appendChild(title);
      [['bol','BOL'],['pod','POD'],['scale_ticket','Scale Ticket'],['other','Other']].forEach(([id, label]) => {
        const b = document.createElement('button');
        b.type = 'button';
        b.className = 'btn btn-secondary sheet__option';
        b.textContent = label;
        b.addEventListener('click', () => {
          document.body.removeChild(sheet);
          onPick(id);
        });
        inner.appendChild(b);
      });
      sheet.appendChild(inner);
      sheet.addEventListener('click', e => { if (e.target === sheet) document.body.removeChild(sheet); });
      document.body.appendChild(sheet);
    }

    await refreshDocs();
    if (container.__renderToken !== renderToken) return;
    page.appendChild(docsSection);
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

  page.appendChild(renderBottomNav('trips'));
}

function renderStopNode(stop, tripId) {
  const node = document.createElement('div');

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

  node.addEventListener('click', () => {
    navigate(`/driver/trips/${tripId}/stops/${stop.sequence}`);
  });

  node.appendChild(content);
  return node;
}
