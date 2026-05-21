/* ============================================================
   Ollie Dispatch — SPA
   Single-file vanilla JS, no framework, no build step.
   ============================================================ */

// ─── Constants ──────────────────────────────────────────────
const TOKEN_KEY = 'dispatch_token';
const API_BASE = '/dispatch/api/v1';
const AUTH_BASE = '/dispatch/auth';

// ─── State ──────────────────────────────────────────────────
let currentView = 'loads';
let currentParams = {};
let navHistory = [];
let eventsRefreshTimer = null;

// ─── Auth helpers ────────────────────────────────────────────

function getToken() {
  return localStorage.getItem(TOKEN_KEY);
}

function saveToken(token) {
  localStorage.setItem(TOKEN_KEY, token);
}

function clearToken() {
  localStorage.removeItem(TOKEN_KEY);
}

/**
 * Decode a JWT payload (base64url → JSON) without verifying the signature.
 * Used only for checking `exp` for UX purposes.
 */
function decodeJwtPayload(token) {
  try {
    const parts = token.split('.');
    if (parts.length !== 3) return null;
    const payload = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    const json = atob(payload);
    return JSON.parse(json);
  } catch {
    return null;
  }
}

function isTokenExpired(token) {
  const payload = decodeJwtPayload(token);
  if (!payload || !payload.exp) return true;
  // exp is in seconds; Date.now() is in ms
  return payload.exp * 1000 < Date.now();
}

function isAuthenticated() {
  const token = getToken();
  if (!token) return false;
  if (isTokenExpired(token)) {
    clearToken();
    return false;
  }
  return true;
}

// ─── API fetch wrapper ───────────────────────────────────────

async function apiFetch(path, options = {}) {
  const token = getToken();
  const headers = {
    'Content-Type': 'application/json',
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(options.headers || {}),
  };

  const res = await fetch(path, { ...options, headers });

  if (res.status === 401) {
    clearToken();
    showLogin();
    throw new Error('Unauthorized — please sign in again.');
  }

  return res;
}

// ─── View/Auth toggle ────────────────────────────────────────

function showLogin() {
  document.getElementById('login-view').hidden = false;
  document.getElementById('app-shell').hidden = true;
  clearEventsRefresh();
}

function showApp() {
  document.getElementById('login-view').hidden = true;
  document.getElementById('app-shell').hidden = false;
}

// ─── Navigation ──────────────────────────────────────────────

const VIEW_TITLES = {
  home: 'Home',
  loads: 'Loads',
  'load-detail': 'Load Detail',
  drivers: 'Drivers',
  'driver-detail': 'Driver Detail',
  trips: 'Trips',
  'trip-detail': 'Trip Detail',
  events: 'Events',
  documents: 'Documents',
  document: 'Document',
};

// ─── Hash routing ────────────────────────────────────────────

let activeObjectUrl = null;

function encodeViewHash(view, params) {
  const entries = Object.entries(params).filter(([, v]) => v !== undefined && v !== null && v !== '');
  const qs = entries.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join('&');
  return qs ? `#${view}?${qs}` : `#${view}`;
}

function decodeViewHash(hash) {
  if (!hash || hash === '#' || hash === '') return { view: 'home', params: {} };
  const noHash = hash.startsWith('#') ? hash.slice(1) : hash;
  const qMark = noHash.indexOf('?');
  const view = qMark === -1 ? noHash : noHash.slice(0, qMark);
  const params = {};
  if (qMark !== -1) {
    new URLSearchParams(noHash.slice(qMark + 1)).forEach((v, k) => { params[k] = v; });
  }
  return { view: view || 'home', params };
}

function goBack() {
  clearEventsRefresh();
  const prev = navHistory.pop();
  if (prev) {
    _renderView(prev.view, prev.params);
  } else {
    _renderView('home', {});
  }
}

function navigate(view, params = {}) {
  clearEventsRefresh();
  navHistory.push({ view: currentView, params: currentParams });
  _renderView(view, params);
}

function _renderView(view, params) {
  currentView = view;
  currentParams = params;

  const hash = encodeViewHash(view, params);
  if (window.location.hash !== hash) {
    history.replaceState(null, '', hash);
  }
  if (activeObjectUrl) {
    URL.revokeObjectURL(activeObjectUrl);
    activeObjectUrl = null;
  }

  // Update sidebar active state
  document.querySelectorAll('.sidebar__link').forEach(btn => {
    const isActive = btn.dataset.view === view;
    btn.classList.toggle('sidebar__link--active', isActive);
  });

  // Update top-bar title
  const title = VIEW_TITLES[view] || view;
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = title;

  // Clear refresh indicator
  setRefreshIndicator('');

  switch (view) {
    case 'home':
      renderHomeView();
      break;
    case 'loads':
      renderLoadsView(params);
      break;
    case 'load-detail':
      renderLoadDetailView(params.id);
      break;
    case 'drivers':
      renderDriversView();
      break;
    case 'driver-detail':
      renderDriverDetailView(params.id);
      break;
    case 'trips':
      renderTripsView(params);
      break;
    case 'trip-detail':
      renderTripDetailView(params.id);
      break;
    case 'events':
      renderEventsView();
      break;
    case 'documents':
      renderDocumentsView(params);
      break;
    case 'document':
      renderDocumentDetailView(params.id);
      break;
    default:
      renderLoadsView({});
  }
}

// ─── Utility: badge ──────────────────────────────────────────

function badge(status) {
  if (!status) return '';
  const slug = status.toLowerCase().replace(/[^a-z0-9_]/g, '_');
  return `<span class="badge badge--${slug}">${escHtml(status)}</span>`;
}

// ─── Utility: short id ───────────────────────────────────────

function shortId(id) {
  if (!id) return '—';
  return id.slice(0, 8);
}

// ─── Utility: local date ─────────────────────────────────────

function fmtDate(isoStr) {
  if (!isoStr) return '—';
  try {
    return new Date(isoStr).toLocaleString();
  } catch {
    return isoStr;
  }
}

function fmtArrivalWindow(start, end) {
  if (!start) return '—';
  if (!end) {
    try { return new Date(start).toLocaleString(); } catch { return start; }
  }
  try {
    const s = new Date(start);
    const e = new Date(end);
    const sameDay = s.toDateString() === e.toDateString();
    if (sameDay) {
      const sStr = s.toLocaleString();
      const eStr = e.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      return `${sStr}–${eStr}`;
    }
    return `${s.toLocaleString()} – ${e.toLocaleString()}`;
  } catch {
    return start;
  }
}

function fmtBytes(n) {
  if (!n) return '—';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function escHtml(s) {
  if (!s) return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

const BLOB_NOISE_EVENTS = new Set([
  'processing_started', 'processing_completed', 'processing_failed',
]);

function humanizeEventType(type) {
  const map = {
    'trip.assigned':     'Trip Assigned',
    'trip.unassigned':   'Trip Unassigned',
    'trip.dispatched':   'Trip Dispatched',
    'trip.undispatched': 'Trip Undispatched',
    'trip.in_transit':   'Trip In Transit',
    'trip.delivered':    'Trip Delivered',
    'trip_completed':    'Trip Completed',
    'trip.cancelled':    'Trip Cancelled',
    'stop.arrived':      'Stop Arrived',
    'stop.departed':     'Stop Departed',
    'stop.late':         'Stop Late',
    'check_call':        'Check Call',
    'driver_available':  'Driver Available',
    'truck_available':   'Truck Available',
    'trailer_available': 'Trailer Available',
  };
  return map[type] || String(type).replace(/[_.]/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
}

// ─── Utility: set main content ───────────────────────────────

function setContent(html) {
  document.getElementById('main-content').innerHTML = html;
}

// ─── Utility: refresh indicator ──────────────────────────────

function setRefreshIndicator(msg) {
  const el = document.getElementById('refresh-indicator');
  if (el) el.textContent = msg;
}

// ─── Home view ───────────────────────────────────────────────

async function renderHomeView() {
  const kpis = [
    { label: 'Open Loads',        endpoint: `${API_BASE}/loads/count`,   view: 'loads'     },
    { label: 'Active Drivers',    endpoint: `${API_BASE}/drivers/count`, view: 'drivers'   },
    { label: 'Pending Documents', endpoint: `${API_BASE}/blobs/count`,   view: 'documents' },
    { label: 'Events Today',      endpoint: `${API_BASE}/events/count`,  view: 'events'    },
  ];

  setContent(`
    <div class="home-view">
      <div class="kpi-row" id="kpi-row">
        ${kpis.map((_, i) => `
          <div class="kpi-tile" id="kpi-tile-${i}">
            <div class="kpi-tile__count">—</div>
            <div class="kpi-tile__label">${escHtml(_.label)}</div>
          </div>
        `).join('')}
      </div>
    </div>
  `);

  kpis.forEach(async (kpi, i) => {
    try {
      const res = await apiFetch(kpi.endpoint);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const tile = document.getElementById(`kpi-tile-${i}`);
      if (tile) {
        tile.querySelector('.kpi-tile__count').textContent = data.count ?? '—';
        tile.style.cursor = 'pointer';
        tile.addEventListener('click', () => navigate(kpi.view));
      }
    } catch (err) {
      console.error(`KPI fetch failed for ${kpi.label}:`, err);
    }
  });
}

// ─── Loads view ──────────────────────────────────────────────

async function renderLoadsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  let filterStatus = params.status || '';

  const buildContent = (loads, filterStatus, capTotal = null) => {
    const capBanner = capTotal !== null
      ? `<div style="background:var(--color-warning-soft);border:1px solid var(--color-warning);border-radius:var(--radius);padding:var(--space-3) var(--space-4);margin-bottom:var(--space-4);font-size:var(--text-sm);color:var(--color-text);">
           Showing the most recent ${escHtml(String(loads.length))} of ${escHtml(String(capTotal))} loads. Use the status filter to narrow results.
         </div>`
      : '';

    const statusOptions = [
      '', 'planned', 'assigned', 'dispatched', 'in_transit',
      'delivered', 'invoiced', 'settled', 'cancelled',
    ];

    const selectHtml = `
      <select class="form-select" id="status-filter">
        ${statusOptions.map(s =>
          `<option value="${s}" ${s === filterStatus ? 'selected' : ''}>${s || 'All Statuses'}</option>`
        ).join('')}
      </select>
    `;

    const sorted = [...loads].sort((a, b) => {
      const ta = a.stops && a.stops[0] ? new Date(a.stops[0].scheduled_arrive || 0).getTime() : 0;
      const tb = b.stops && b.stops[0] ? new Date(b.stops[0].scheduled_arrive || 0).getTime() : 0;
      if (ta === 0 && tb === 0) return 0;
      if (ta === 0) return 1;
      if (tb === 0) return -1;
      return tb - ta;
    });

    let rows = '';
    if (sorted.length === 0) {
      rows = `<tr><td colspan="6" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No loads found</td></tr>`;
    } else {
      rows = sorted.map(load => {
        const stops = load.stops || [];
        const last = stops.length - 1;
        const origin = stops[0]?.name || '—';
        const dest = stops[last]?.name || '—';
        return `
        <tr data-load-id="${load.id}">
          <td style="font-variant-numeric: tabular-nums;">${escHtml(load.load_number || shortId(load.id))}</td>
          <td>${badge(load.status)}</td>
          <td>${escHtml(load.customer_name || '—')}</td>
          <td>${escHtml(origin)} → ${escHtml(dest)}</td>
          <td>${fmtArrivalWindow(stops[0]?.scheduled_arrive, stops[0]?.scheduled_arrive_end)}</td>
          <td>${fmtArrivalWindow(stops[last]?.scheduled_arrive, stops[last]?.scheduled_arrive_end)}</td>
        </tr>
      `;
      }).join('');
    }

    return `
      ${capBanner}<div class="page-header">
        <h1 class="page-title">Loads</h1>
        <div class="page-controls">
          ${selectHtml}
        </div>
      </div>
      <div class="table-wrapper">
        <table class="data-table">
          <thead>
            <tr>
              <th>Load #</th>
              <th>Status</th>
              <th>Customer</th>
              <th>Route</th>
              <th>Pickup</th>
              <th>Delivery</th>
            </tr>
          </thead>
          <tbody id="loads-tbody">
            ${rows}
          </tbody>
        </table>
      </div>
    `;
  };

  const fetchAndRender = async (status) => {
    try {
      const qs = status ? `?status=${encodeURIComponent(status)}` : '';
      const res = await apiFetch(`${API_BASE}/loads${qs}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const loads = data.loads || data.items || (Array.isArray(data) ? data : []);
      const returned = typeof data.returned === 'number' ? data.returned : null;
      // The server caps scans at LOAD_SCAN_CAP (2000). Only show the cap
      // banner when we're actually at that ceiling — otherwise it fires on
      // every normal paginated result where total exceeds page size.
      const LOAD_SCAN_CAP = 2000;
      const capTotal = returned !== null && returned >= LOAD_SCAN_CAP ? returned : null;
      setContent(buildContent(loads, status, capTotal));

      // Bind filter change
      const filterEl = document.getElementById('status-filter');
      if (filterEl) {
        filterEl.addEventListener('change', () => {
          navigate('loads', { status: filterEl.value });
        });
      }

      // Bind row clicks
      document.querySelectorAll('#loads-tbody tr[data-load-id]').forEach(row => {
        row.addEventListener('click', () => {
          navigate('load-detail', { id: row.dataset.loadId });
        });
      });
    } catch (err) {
      if (err.message !== 'Unauthorized — please sign in again.') {
        setContent(`<div class="state-error">Failed to load data: ${err.message}</div>`);
      }
    }
  };

  await fetchAndRender(filterStatus);
}

// ─── Load detail view ─────────────────────────────────────────

async function renderLoadDetailView(id) {
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = 'Load';

  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const [loadRes, tripsRes] = await Promise.all([
      apiFetch(`${API_BASE}/loads/${id}`),
      apiFetch(`${API_BASE}/trips?load_id=${id}`),
    ]);

    if (!loadRes.ok) throw new Error(`Load fetch HTTP ${loadRes.status}`);
    const load = await loadRes.json();

    if (topbarTitle) topbarTitle.textContent = `Load ${load.load_number || shortId(id)}`;

    let trips = [];
    if (tripsRes.ok) {
      const tripsData = await tripsRes.json();
      trips = tripsData.trips || tripsData.items || (Array.isArray(tripsData) ? tripsData : []);
    }

    // Build stops section if load has stops
    let stopsHtml = '';
    const stops = load.stops || [];
    if (stops.length > 0) {
      const stopRows = stops.map((stop, i) => `
        <tr>
          <td>${i + 1}</td>
          <td>${escHtml(stop.facility_name || '—')}</td>
          <td>${escHtml(stop.stop_type || '—')}</td>
          <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
          <td>${fmtDate(stop.actual_arrive)}</td>
          <td>${fmtDate(stop.actual_depart)}</td>
        </tr>
      `).join('');

      stopsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Stops</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead>
                <tr>
                  <th>#</th>
                  <th>Facility</th>
                  <th>Type</th>
                  <th>Scheduled Arrive</th>
                  <th>Actual Arrive</th>
                  <th>Actual Depart</th>
                </tr>
              </thead>
              <tbody>${stopRows}</tbody>
            </table>
          </div>
        </div>
      `;
    }

    // Build trips section
    let tripsHtml = '';
    if (trips.length > 0) {
      const tripRows = trips.map(trip => `
        <tr data-trip-id="${trip.id}">
          <td style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || shortId(trip.id))}</td>
          <td>${badge(trip.status)}</td>
          <td>${escHtml(trip.driver_name || '—')}</td>
          <td>${escHtml(trip.truck_unit || '—')}</td>
        </tr>
      `).join('');

      tripsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Trips</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Trip #</th>
                  <th>Status</th>
                  <th>Driver</th>
                  <th>Truck</th>
                </tr>
              </thead>
              <tbody>${tripRows}</tbody>
            </table>
          </div>
        </div>
      `;
    } else {
      tripsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Trips</div>
          <div class="state-empty" style="min-height: 80px;">No trips for this load</div>
        </div>
      `;
    }

    // Build documents section if load has blob_ids
    let docsHtml = '';
    const blobIds = load.blob_ids || [];
    if (blobIds.length > 0) {
      const blobResults = await Promise.all(
        blobIds.map(bid =>
          apiFetch(`${API_BASE}/blob/${bid}`, {
            headers: { Accept: 'application/json' },
          })
            .then(r => (r.ok ? r.json() : null))
            .catch(() => null)
        )
      );
      const validBlobs = blobResults.filter(Boolean);
      if (validBlobs.length > 0) {
        const docRows = validBlobs
          .map(
            b => `
          <tr class="doc-row" data-blob-id="${b.id}" style="cursor:pointer;">
            <td>${escHtml(b.name) || '—'}</td>
            <td style="font-size:var(--text-sm);color:var(--color-text-muted);">${escHtml((b.mime_type || '').split('/').pop())}</td>
            <td>${fmtBytes(b.size)}</td>
            <td>${badge(b.status)}</td>
            <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(b.summary) || '—'}</td>
          </tr>
        `
          )
          .join('');

        docsHtml = `
          <div class="detail-card">
            <div class="detail-card__title">Documents</div>
            <div class="table-wrapper">
              <table class="data-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Type</th>
                    <th>Size</th>
                    <th>Status</th>
                    <th>Summary</th>
                  </tr>
                </thead>
                <tbody>${docRows}</tbody>
              </table>
            </div>
          </div>
        `;
      }
    }

    const html = `
      <button class="back-link" id="back-to-loads">← Back to Loads</button>

      <div class="detail-card">
        <div class="detail-card__title">Load Details</div>
        <div class="detail-grid">
          <div class="detail-item">
            <div class="detail-item__label">Load #</div>
            <div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${escHtml(load.load_number || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(load.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Customer</div>
            <div class="detail-item__value">${escHtml(load.customer || load.customer_name || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Created</div>
            <div class="detail-item__value">${fmtDate(load.created_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Updated</div>
            <div class="detail-item__value">${fmtDate(load.updated_at)}</div>
          </div>
          ${load.invoice_number ? `
          <div class="detail-item">
            <div class="detail-item__label">Invoice #</div>
            <div class="detail-item__value">${escHtml(load.invoice_number)}</div>
          </div>` : ''}
          ${load.cancel_reason ? `
          <div class="detail-item">
            <div class="detail-item__label">Cancel Reason</div>
            <div class="detail-item__value">${escHtml(load.cancel_reason)}</div>
          </div>` : ''}
        </div>
      </div>

      ${stopsHtml}
      ${tripsHtml}
      ${docsHtml}
    `;

    setContent(html);

    document.getElementById('back-to-loads').addEventListener('click', goBack);

    document.querySelectorAll('.doc-row').forEach(row => {
      row.addEventListener('click', () => {
        navigate('document', { id: row.dataset.blobId });
      });
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load data: ${err.message}</div>`);
    }
  }
}

// ─── Drivers view ─────────────────────────────────────────────

async function renderDriversView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const res = await apiFetch(`${API_BASE}/drivers`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const drivers = data.drivers || data.items || (Array.isArray(data) ? data : []);

    let rows = '';
    if (drivers.length === 0) {
      rows = `<tr><td colspan="3" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No drivers found</td></tr>`;
    } else {
      rows = drivers.map(driver => {
        const isAvailable = driver.status === 'available';
        const rowClass = isAvailable ? 'row--available' : '';
        return `
          <tr${rowClass ? ` class="${rowClass}"` : ''} data-driver-id="${driver.id}" style="cursor:pointer;">
            <td>${escHtml(driver.name || '—')}</td>
            <td>${badge(driver.status)}</td>
            <td>${escHtml(driver.phone || '—')}</td>
          </tr>
        `;
      }).join('');
    }

    const html = `
      <div class="page-header">
        <h1 class="page-title">Drivers</h1>
      </div>
      <div class="table-wrapper">
        <table class="data-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Status</th>
              <th>Phone</th>
            </tr>
          </thead>
          <tbody>
            ${rows}
          </tbody>
        </table>
      </div>
    `;

    setContent(html);

    document.querySelectorAll('tr[data-driver-id]').forEach(row => {
      row.addEventListener('click', () => navigate('driver-detail', { id: row.dataset.driverId }));
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load data: ${err.message}</div>`);
    }
  }
}

// ─── Trips view ──────────────────────────────────────────────

async function renderTripsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const qs = params.status ? `?status=${encodeURIComponent(params.status)}` : '';
    const res = await apiFetch(`${API_BASE}/trips${qs}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const trips = data.items || data.trips || (Array.isArray(data) ? data : []);

    const statusOptions = ['', 'planned', 'assigned', 'dispatched', 'in_transit', 'delivered', 'completed', 'cancelled'];
    const filterStatus = params.status || '';
    const selectHtml = `<select class="form-select" id="trip-status-filter">${statusOptions.map(s => `<option value="${s}" ${s === filterStatus ? 'selected' : ''}>${s || 'All Statuses'}</option>`).join('')}</select>`;

    const sorted = [...trips].sort((a, b) => {
      const ta = a.stops && a.stops[0] ? new Date(a.stops[0].scheduled_arrive || 0).getTime() : 0;
      const tb = b.stops && b.stops[0] ? new Date(b.stops[0].scheduled_arrive || 0).getTime() : 0;
      if (ta === 0 && tb === 0) return 0;
      if (ta === 0) return 1;
      if (tb === 0) return -1;
      return tb - ta;
    });

    let rows = '';
    if (sorted.length === 0) {
      rows = `<tr><td colspan="7" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No trips found</td></tr>`;
    } else {
      rows = sorted.map(trip => {
        const lastStop = trip.stops && trip.stops.length > 0 ? trip.stops[trip.stops.length - 1] : null;
        const origin = trip.stops && trip.stops[0] ? (trip.stops[0].name || '—') : '—';
        const dest = lastStop ? (lastStop.name || '—') : '—';
        const pickup = fmtArrivalWindow(
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null,
          trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive_end : null,
        );
        const delivery = fmtArrivalWindow(
          lastStop ? lastStop.scheduled_arrive : null,
          lastStop ? lastStop.scheduled_arrive_end : null,
        );
        return `<tr data-trip-id="${trip.id}" style="cursor:pointer;"><td style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || shortId(trip.id))}</td><td>${escHtml(trip.load_number || '—')}</td><td>${badge(trip.status)}</td><td>${escHtml(trip.driver_name || '—')}</td><td>${escHtml(origin)} → ${escHtml(dest)}</td><td>${pickup}</td><td>${delivery}</td></tr>`;
      }).join('');
    }

    setContent(`
      <div class="page-header"><h1 class="page-title">Trips</h1><div class="page-controls">${selectHtml}</div></div>
      <div class="table-wrapper">
        <table class="data-table">
          <thead><tr><th>Trip #</th><th>Load #</th><th>Status</th><th>Driver</th><th>Route</th><th>Pickup</th><th>Delivery</th></tr></thead>
          <tbody id="trips-tbody">${rows}</tbody>
        </table>
      </div>
    `);

    const filterEl = document.getElementById('trip-status-filter');
    if (filterEl) filterEl.addEventListener('change', () => navigate('trips', { status: filterEl.value }));
    document.querySelectorAll('#trips-tbody tr[data-trip-id]').forEach(row => {
      row.addEventListener('click', () => navigate('trip-detail', { id: row.dataset.tripId }));
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trips: ${err.message}</div>`);
    }
  }
}

// ─── Trip detail view ─────────────────────────────────────────

async function renderTripDetailView(id) {
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = 'Trip Detail';
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/trips/${id}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const trip = await res.json();

    if (topbarTitle) topbarTitle.textContent = `Trip ${trip.trip_number || shortId(id)}`;

    const stopRows = (trip.stops || []).map((stop, i) => `
      <tr>
        <td>${i + 1}</td>
        <td>${escHtml(stop.name || '—')}</td>
        <td>${escHtml(stop.stop_type || '—')}</td>
        <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
        <td>${fmtDate(stop.actual_arrive)}</td>
        <td>${fmtDate(stop.actual_depart)}</td>
      </tr>
    `).join('');

    setContent(`
      <button class="back-link" id="back-to-trips">← Back to Trips</button>
      <div class="detail-card">
        <div class="detail-card__title">Trip ${escHtml(trip.trip_number || shortId(trip.id))}</div>
        <div class="detail-grid">
          <div class="detail-item"><div class="detail-item__label">Trip #</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Status</div><div class="detail-item__value">${badge(trip.status)}</div></div>
          <div class="detail-item"><div class="detail-item__label">Driver</div><div class="detail-item__value">${escHtml(trip.driver_name || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Truck</div><div class="detail-item__value">${escHtml(trip.truck_unit || '—')}</div></div>
          <div class="detail-item"><div class="detail-item__label">Trailer</div><div class="detail-item__value">${escHtml((trip.trailer_units || []).join(', ') || '—')}</div></div>
        </div>
      </div>
      <div class="detail-card">
        <div class="detail-card__title">Stops</div>
        <div class="table-wrapper">
          <table class="data-table">
            <thead><tr><th>#</th><th>Facility</th><th>Type</th><th>Scheduled Arrive</th><th>Actual Arrive</th><th>Actual Depart</th></tr></thead>
            <tbody>${stopRows || '<tr><td colspan="6" style="text-align:center; padding: var(--space-4); color: var(--color-text-muted);">No stops</td></tr>'}</tbody>
          </table>
        </div>
      </div>
    `);

    document.getElementById('back-to-trips').addEventListener('click', goBack);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load trip: ${err.message}</div>`);
    }
  }
}

// ─── Driver detail view ───────────────────────────────────────

async function renderDriverDetailView(id) {
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = 'Driver Detail';
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const [driverRes, tripsRes] = await Promise.all([
      apiFetch(`${API_BASE}/drivers/${id}`),
      apiFetch(`${API_BASE}/trips?driver_id=${id}`),
    ]);
    if (!driverRes.ok) throw new Error(`Driver fetch HTTP ${driverRes.status}`);
    const driver = await driverRes.json();
    let trips = [];
    if (tripsRes.ok) {
      const tripsData = await tripsRes.json();
      trips = tripsData.items || tripsData.trips || (Array.isArray(tripsData) ? tripsData : []);
    }

    const tripRows = trips.map(trip => `
      <tr data-trip-id="${trip.id}" style="cursor:pointer;">
        <td style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || shortId(trip.id))}</td>
        <td>${badge(trip.status)}</td>
        <td>${fmtArrivalWindow(trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null, trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive_end : null)}</td>
      </tr>
    `).join('');

    setContent(`
      <button class="back-link" id="back-to-drivers">← Back to Drivers</button>
      <div class="detail-card">
        <div class="detail-card__title">${escHtml(driver.name || '—')}</div>
        <div class="detail-grid">
          <div class="detail-item"><div class="detail-item__label">Status</div><div class="detail-item__value">${badge(driver.status)}</div></div>
          <div class="detail-item"><div class="detail-item__label">Phone</div><div class="detail-item__value">${escHtml(driver.phone || '—')}</div></div>
        </div>
      </div>
      <div class="detail-card">
        <div class="detail-card__title">Trips</div>
        <div class="table-wrapper">
          <table class="data-table">
            <thead><tr><th>Trip #</th><th>Status</th><th>First Stop Scheduled</th></tr></thead>
            <tbody id="driver-trips-tbody">${tripRows || '<tr><td colspan="3" style="text-align:center; padding: var(--space-4); color: var(--color-text-muted);">No trips</td></tr>'}</tbody>
          </table>
        </div>
      </div>
    `);

    document.getElementById('back-to-drivers').addEventListener('click', goBack);
    document.querySelectorAll('#driver-trips-tbody tr[data-trip-id]').forEach(row => {
      row.addEventListener('click', () => navigate('trip-detail', { id: row.dataset.tripId }));
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load driver: ${err.message}</div>`);
    }
  }
}

// ─── Events view ─────────────────────────────────────────────

function clearEventsRefresh() {
  if (eventsRefreshTimer !== null) {
    clearInterval(eventsRefreshTimer);
    eventsRefreshTimer = null;
  }
}

async function fetchAndRenderEvents() {
  try {
    const res = await apiFetch(`${API_BASE}/events`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const events = data.events || data.items || (Array.isArray(data) ? data : []);

    const filtered = events.filter(ev => !BLOB_NOISE_EVENTS.has(ev.event_type));

    // Most recent first, using occurred_at
    const sorted = [...filtered].sort((a, b) =>
      new Date(b.occurred_at || 0).getTime() - new Date(a.occurred_at || 0).getTime()
    );

    setRefreshIndicator(`Updated ${new Date().toLocaleTimeString()}`);

    if (sorted.length === 0) {
      const listEl = document.getElementById('events-list');
      if (listEl) {
        listEl.innerHTML = '<div class="state-empty" style="min-height:120px;">No events found</div>';
      }
      return;
    }

    const items = sorted.map(ev => {
      const entityType = (ev.entity_type || '').toLowerCase().replace(/[^a-z0-9_]/g, '_');
      const entityLabel = entityType.charAt(0).toUpperCase() + entityType.slice(1);

      let payload = {};
      try {
        payload = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : (ev.payload || {});
      } catch (_) {}
      const stopName = payload.facility_name || payload.stop_name ||
        (payload.sequence != null ? `Stop ${payload.sequence}` : null);
      const stopSuffix = stopName ? ` · ${escHtml(stopName)}` : '';

      const badgeHtml = entityType
        ? `<span class="badge badge--${entityType}">${escHtml(entityLabel)}</span> `
        : '';

      return `
      <div class="event-item">
        ${badgeHtml}<span class="event-item__type">${escHtml(humanizeEventType(ev.event_type || ''))}</span>${stopSuffix}
        <span class="event-item__time">${fmtDate(ev.occurred_at)}</span>
      </div>
    `;
    }).join('');

    const listEl = document.getElementById('events-list');
    if (listEl) {
      listEl.innerHTML = items;
    }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      const listEl = document.getElementById('events-list');
      if (listEl) {
        listEl.innerHTML = `<div class="state-error" style="min-height:80px;">Failed to load events: ${err.message}</div>`;
      }
      setRefreshIndicator('Error');
    }
  }
}

async function renderEventsView() {
  // Initial skeleton so the list element exists before fetch
  setContent(`
    <div class="page-header">
      <h1 class="page-title">Events</h1>
      <span style="font-size: 0.8125rem; color: var(--color-text-subtle);">Auto-refreshes every 30s</span>
    </div>
    <div class="events-list" id="events-list">
      <div class="state-loading"><div class="spinner"></div></div>
    </div>
  `);

  await fetchAndRenderEvents();

  // Auto-refresh every 30s
  eventsRefreshTimer = setInterval(fetchAndRenderEvents, 30_000);
}

// ─── Documents view ──────────────────────────────────────────

async function renderDocumentsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const offset = params.offset || 0;
  const filterName = params.name || '';

  try {
    const qs = new URLSearchParams({ limit: 20, offset });
    if (filterName) qs.set('name', filterName);

    const resp = await apiFetch(`${API_BASE}/blobs?${qs}`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const data = await resp.json();
    const blobs = data.items || [];

    const filterHtml = `
      <div style="display:flex;gap:var(--space-2);margin-bottom:var(--space-3);">
        <input class="form-input" id="doc-filter-name" type="text"
          placeholder="Filter by name…" value="${escHtml(filterName)}" style="max-width:240px;">
        <button class="btn btn--secondary" id="doc-filter-apply">Search</button>
      </div>
    `;

    let tableHtml = '';
    if (blobs.length === 0 && offset === 0) {
      tableHtml = '<div class="state-empty">No documents found</div>';
    } else {
      const rows = blobs.map(b => `
        <tr class="doc-row" data-blob-id="${b.id}" style="cursor:pointer;">
          <td>${escHtml(b.name) || '—'}</td>
          <td style="font-size:var(--text-sm);color:var(--color-text-muted);">${escHtml((b.mime_type || '').split('/').pop())}</td>
          <td>${fmtBytes(b.size)}</td>
          <td>${badge(b.status)}</td>
          <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(b.summary) || '—'}</td>
          <td>${fmtDate(b.created_at)}</td>
        </tr>
      `).join('');

      tableHtml = `
        <div class="table-wrapper">
          <table class="data-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Type</th>
                <th>Size</th>
                <th>Status</th>
                <th>Summary</th>
                <th>Uploaded</th>
              </tr>
            </thead>
            <tbody>${rows}</tbody>
          </table>
        </div>
        ${blobs.length === 20 ? `
          <div style="text-align:center;margin-top:var(--space-3);">
            <button class="btn btn--secondary" id="doc-load-more">Load more</button>
          </div>` : ''}
      `;
    }

    setContent(filterHtml + tableHtml);

    document.getElementById('doc-filter-apply')?.addEventListener('click', () => {
      const name = document.getElementById('doc-filter-name').value.trim();
      navigate('documents', { name });
    });
    document.getElementById('doc-filter-name')?.addEventListener('keydown', e => {
      if (e.key === 'Enter') navigate('documents', { name: e.target.value.trim() });
    });
    document.getElementById('doc-load-more')?.addEventListener('click', () => {
      navigate('documents', { name: filterName, offset: offset + 20 });
    });

    document.querySelectorAll('.doc-row').forEach(row => {
      row.addEventListener('click', () => {
        navigate('document', { id: row.dataset.blobId });
      });
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load documents: ${err.message}</div>`);
    }
  }
}

// ─── Document detail view ────────────────────────────────────

async function renderDocumentDetailView(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const metaRes = await apiFetch(`${API_BASE}/blob/${id}`, {
      headers: { Accept: 'application/json' },
    });
    if (!metaRes.ok) throw new Error(`HTTP ${metaRes.status}`);
    const doc = await metaRes.json();

    const tags = (doc.tags || []).map(t => escHtml(t)).join(', ') || '—';
    const errorRow = doc.status === 'failed' && doc.error
      ? `<div class="detail-item" style="grid-column: 1 / -1;">
           <div class="detail-item__label">Error</div>
           <div class="detail-item__value" style="color:var(--color-danger);">${escHtml(doc.error)}</div>
         </div>`
      : '';

    const html = `
      <button class="back-link" id="doc-back">&#x2190; Back</button>

      <div class="detail-card">
        <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:var(--space-4);padding-bottom:var(--space-3);border-bottom:1px solid var(--color-border);">
          <div style="font-size:1rem;font-weight:700;color:var(--color-text);">${escHtml(doc.name || 'Document')}</div>
          <button class="btn btn--secondary" id="doc-download">Download</button>
        </div>
        <div class="detail-grid">
          <div class="detail-item">
            <div class="detail-item__label">Type</div>
            <div class="detail-item__value">${escHtml(doc.mime_type || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Size</div>
            <div class="detail-item__value">${fmtBytes(doc.size)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(doc.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Uploaded</div>
            <div class="detail-item__value">${fmtDate(doc.created_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Updated</div>
            <div class="detail-item__value">${fmtDate(doc.updated_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Tags</div>
            <div class="detail-item__value">${tags}</div>
          </div>
          ${doc.summary ? `
          <div class="detail-item" style="grid-column: 1 / -1;">
            <div class="detail-item__label">Summary</div>
            <div class="detail-item__value">${escHtml(doc.summary)}</div>
          </div>` : ''}
          ${errorRow}
        </div>
      </div>

      <div class="detail-card">
        <div class="detail-card__title">Preview</div>
        <div id="doc-viewer"><div class="state-loading"><div class="spinner"></div></div></div>
      </div>
    `;

    setContent(html);

    document.getElementById('doc-back').addEventListener('click', goBack);

    document.getElementById('doc-download').addEventListener('click', async () => {
      try {
        const fileResp = await apiFetch(`${API_BASE}/blob/${id}`);
        if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
        const blob = await fileResp.blob();
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = doc.name || 'document';
        a.click();
        URL.revokeObjectURL(url);
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          alert(`Download failed: ${err.message}`);
        }
      }
    });

    const viewerEl = document.getElementById('doc-viewer');
    const mt = doc.mime_type || '';
    const isPdf = mt === 'application/pdf';
    const isImage = mt.startsWith('image/');
    const isPlainText = mt === 'text/plain';
    const canPreview = isPdf || isImage || isPlainText;

    if (!canPreview) {
      const msg = document.createElement('div');
      msg.className = 'state-empty';
      msg.style.minHeight = '80px';
      msg.textContent = "This document type can't be previewed — use the Download button above.";
      viewerEl.textContent = '';
      viewerEl.appendChild(msg);
    } else {
      try {
        const fileResp = await apiFetch(`${API_BASE}/blob/${id}`);
        if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
        const blob = await fileResp.blob();
        viewerEl.textContent = '';
        if (isPdf) {
          const url = URL.createObjectURL(blob);
          activeObjectUrl = url;
          const iframe = document.createElement('iframe');
          iframe.src = url;
          iframe.style.cssText = 'width:100%;height:600px;border:none;';
          iframe.title = doc.name || 'preview';
          viewerEl.appendChild(iframe);
        } else if (isImage) {
          const url = URL.createObjectURL(blob);
          activeObjectUrl = url;
          const img = document.createElement('img');
          img.src = url;
          img.alt = doc.name || 'preview';
          img.style.cssText = 'max-width:100%;height:auto;display:block;';
          viewerEl.appendChild(img);
        } else if (isPlainText) {
          const text = await blob.text();
          const pre = document.createElement('pre');
          pre.style.cssText = 'white-space:pre-wrap;word-break:break-word;max-height:600px;overflow:auto;margin:0;padding:12px;background:var(--color-surface-2);border-radius:4px;';
          pre.textContent = text;
          viewerEl.appendChild(pre);
        }
      } catch (err) {
        if (err.message !== 'Unauthorized — please sign in again.') {
          viewerEl.textContent = `Preview failed: ${err.message}`;
        }
      }
    }
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent('<div class="state-error">Failed to load document.</div>');
    }
  }
}

// ─── Login form ──────────────────────────────────────────────

function initLoginForm() {
  const form = document.getElementById('login-form');
  if (!form) return;

  form.addEventListener('submit', async (e) => {
    e.preventDefault();

    const alertEl = document.getElementById('login-alert');
    const submitBtn = document.getElementById('login-submit');
    const email = document.getElementById('login-email').value.trim();
    const password = document.getElementById('login-password').value;

    alertEl.hidden = true;
    alertEl.className = 'alert';
    alertEl.textContent = '';
    submitBtn.disabled = true;
    submitBtn.textContent = 'Signing in…';

    try {
      const res = await fetch(`${AUTH_BASE}/login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, password }),
      });

      if (res.ok) {
        const data = await res.json();
        saveToken(data.token || data.access_token);
        showApp();
        navigate('home');
        return;
      }

      if (res.status === 423) {
        const data = await res.json().catch(() => ({}));
        const until = data.locked_until ? ` Account locked until ${fmtDate(data.locked_until)}.` : '';
        showAlert(alertEl, 'alert--warning', `Account is locked.${until}`);
        return;
      }

      if (res.status === 401) {
        showAlert(alertEl, 'alert--error', 'Invalid credentials. Please try again.');
        return;
      }

      showAlert(alertEl, 'alert--error', `Login failed (HTTP ${res.status}). Please try again.`);
    } catch (err) {
      showAlert(alertEl, 'alert--error', `Network error: ${err.message}`);
    } finally {
      submitBtn.disabled = false;
      submitBtn.textContent = 'Sign in';
    }
  });
}

function showAlert(el, cls, msg) {
  el.className = `alert ${cls}`;
  el.textContent = msg;
  el.hidden = false;
}

// ─── Sidebar & logout ────────────────────────────────────────

function initSidebar() {
  document.querySelectorAll('.sidebar__link[data-view]').forEach(btn => {
    btn.addEventListener('click', () => {
      navigate(btn.dataset.view);
    });
  });

  const logoutBtn = document.getElementById('logout-btn');
  if (logoutBtn) {
    logoutBtn.addEventListener('click', () => {
      clearToken();
      clearEventsRefresh();
      showLogin();
    });
  }
}

// ─── Boot ────────────────────────────────────────────────────

function boot() {
  initLoginForm();
  initSidebar();

  if (isAuthenticated()) {
    showApp();
    const { view, params } = decodeViewHash(window.location.hash);
    _renderView(view, params);
  } else {
    showLogin();
  }
}

document.addEventListener('DOMContentLoaded', boot);
