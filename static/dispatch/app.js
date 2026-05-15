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
  drivers: 'Drivers',
  'driver-detail': 'Driver Detail',
  trips: 'Trips',
  events: 'Events',
  documents: 'Documents',
};

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
    default:
      renderLoadsView({});
  }
}

// ─── Utility: badge ──────────────────────────────────────────

function badge(status) {
  if (!status) return '';
  const slug = status.toLowerCase().replace(/[^a-z0-9_]/g, '_');
  return `<span class="badge badge--${slug}">${status}</span>`;
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
      const data = await apiFetch(kpi.endpoint);
      const tile = document.getElementById(`kpi-tile-${i}`);
      if (tile) {
        tile.querySelector('.kpi-tile__count').textContent = data.count ?? '—';
        tile.style.cursor = 'pointer';
        tile.addEventListener('click', () => navigate(kpi.view));
      }
    } catch {
      // leave as —
    }
  });
}

// ─── Loads view ──────────────────────────────────────────────

async function renderLoadsView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  let filterStatus = params.status || '';

  const buildContent = (loads, filterStatus) => {
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

    let rows = '';
    if (loads.length === 0) {
      rows = `<tr><td colspan="3" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No loads found</td></tr>`;
    } else {
      rows = loads.map(load => `
        <tr data-load-id="${load.id}">
          <td style="font-variant-numeric: tabular-nums;">${load.load_number || shortId(load.id)}</td>
          <td>${badge(load.status)}</td>
          <td>${fmtDate(load.created_at)}</td>
        </tr>
      `).join('');
    }

    return `
      <div class="page-header">
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
              <th>Created</th>
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
      setContent(buildContent(loads, status));

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
          <td>${stop.facility_name || '—'}</td>
          <td>${stop.stop_type || '—'}</td>
          <td>${fmtDate(stop.scheduled_arrive)}</td>
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
          <td style="font-variant-numeric: tabular-nums;">${trip.trip_number || shortId(trip.id)}</td>
          <td>${badge(trip.status)}</td>
          <td>${trip.driver_name || '—'}</td>
          <td>${trip.truck_unit || '—'}</td>
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
          <tr class="doc-row" data-blob-id="${b.id}" data-blob-name="${escHtml(b.name || 'download')}" style="cursor:pointer;">
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
            <div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${load.load_number || '—'}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(load.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Customer</div>
            <div class="detail-item__value">${load.customer || load.customer_name || '—'}</div>
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
            <div class="detail-item__value">${load.invoice_number}</div>
          </div>` : ''}
          ${load.cancel_reason ? `
          <div class="detail-item">
            <div class="detail-item__label">Cancel Reason</div>
            <div class="detail-item__value">${load.cancel_reason}</div>
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
      row.addEventListener('click', async () => {
        const blobId = row.dataset.blobId;
        const fileName = row.dataset.blobName;
        try {
          const fileResp = await apiFetch(`${API_BASE}/blob/${blobId}`);
          if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
          const blob = await fileResp.blob();
          const url = URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = url;
          a.download = fileName;
          a.click();
          URL.revokeObjectURL(url);
        } catch (err) {
          if (err.message !== 'Unauthorized — please sign in again.') {
            alert(`Download failed: ${err.message}`);
          }
        }
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
            <td>${driver.name || '—'}</td>
            <td>${badge(driver.status)}</td>
            <td>${driver.phone || '—'}</td>
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

    let rows = '';
    if (trips.length === 0) {
      rows = `<tr><td colspan="4" style="text-align:center; padding: var(--space-5); color: var(--color-text-muted);">No trips found</td></tr>`;
    } else {
      rows = trips.map(trip => {
        const origin = trip.stops && trip.stops[0] ? (trip.stops[0].name || '—') : '—';
        const dest = trip.stops && trip.stops.length > 1 ? (trip.stops[trip.stops.length - 1].name || '—') : '—';
        return `<tr data-trip-id="${trip.id}" style="cursor:pointer;"><td style="font-variant-numeric: tabular-nums;">${trip.trip_number || shortId(trip.id)}</td><td>${badge(trip.status)}</td><td>${origin} → ${dest}</td><td>${trip.driver_name || '—'}</td></tr>`;
      }).join('');
    }

    setContent(`
      <div class="page-header"><h1 class="page-title">Trips</h1><div class="page-controls">${selectHtml}</div></div>
      <div class="table-wrapper">
        <table class="data-table">
          <thead><tr><th>Trip #</th><th>Status</th><th>Route</th><th>Driver</th></tr></thead>
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
        <td>${stop.name || '—'}</td>
        <td>${stop.stop_type || '—'}</td>
        <td>${fmtDate(stop.scheduled_arrive)}</td>
        <td>${fmtDate(stop.actual_arrive)}</td>
        <td>${fmtDate(stop.actual_depart)}</td>
      </tr>
    `).join('');

    setContent(`
      <button class="back-link" id="back-to-trips">← Back to Trips</button>
      <div class="detail-card">
        <div class="detail-card__title">Trip ${trip.trip_number || shortId(trip.id)}</div>
        <div class="detail-grid">
          <div class="detail-item"><div class="detail-item__label">Trip #</div><div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${trip.trip_number || '—'}</div></div>
          <div class="detail-item"><div class="detail-item__label">Status</div><div class="detail-item__value">${badge(trip.status)}</div></div>
          <div class="detail-item"><div class="detail-item__label">Driver</div><div class="detail-item__value">${trip.driver_name || '—'}</div></div>
          <div class="detail-item"><div class="detail-item__label">Truck</div><div class="detail-item__value">${trip.truck_unit || '—'}</div></div>
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
        <td style="font-variant-numeric: tabular-nums;">${trip.trip_number || shortId(trip.id)}</td>
        <td>${badge(trip.status)}</td>
        <td>${fmtDate(trip.stops && trip.stops[0] ? trip.stops[0].scheduled_arrive : null)}</td>
      </tr>
    `).join('');

    setContent(`
      <button class="back-link" id="back-to-drivers">← Back to Drivers</button>
      <div class="detail-card">
        <div class="detail-card__title">${driver.name || '—'}</div>
        <div class="detail-grid">
          <div class="detail-item"><div class="detail-item__label">Status</div><div class="detail-item__value">${badge(driver.status)}</div></div>
          <div class="detail-item"><div class="detail-item__label">Phone</div><div class="detail-item__value">${driver.phone || '—'}</div></div>
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

    // Most recent first
    const sorted = [...events].sort((a, b) => {
      const ta = new Date(a.created_at || a.timestamp || 0).getTime();
      const tb = new Date(b.created_at || b.timestamp || 0).getTime();
      return tb - ta;
    });

    setRefreshIndicator(`Updated ${new Date().toLocaleTimeString()}`);

    if (sorted.length === 0) {
      const listEl = document.getElementById('events-list');
      if (listEl) {
        listEl.innerHTML = '<div class="state-empty" style="min-height:120px;">No events found</div>';
      }
      return;
    }

    const items = sorted.map(ev => `
      <div class="event-item">
        <span class="event-item__type">${ev.event_type || ev.type || '—'}</span>
        <span class="event-item__entity">${shortId(ev.entity_id || ev.id)}</span>
        <span class="event-item__time">${fmtDate(ev.created_at || ev.timestamp)}</span>
      </div>
    `).join('');

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
        <tr class="doc-row" data-blob-id="${b.id}" data-blob-name="${escHtml(b.name || 'download')}" style="cursor:pointer;">
          <td>${escHtml(b.name) || '—'}</td>
          <td style="font-size:var(--text-sm);color:var(--color-text-muted);">${escHtml((b.mime_type || '').split('/').pop())}</td>
          <td>${fmtBytes(b.size)}</td>
          <td>${badge(b.status)}</td>
          <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(b.summary) || '—'}</td>
          <td>${escHtml((b.tags || []).join(', ')) || '—'}</td>
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
                <th>Tags</th>
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
      row.addEventListener('click', async () => {
        const blobId = row.dataset.blobId;
        const fileName = row.dataset.blobName;
        try {
          const fileResp = await apiFetch(`${API_BASE}/blob/${blobId}`);
          if (!fileResp.ok) throw new Error(`HTTP ${fileResp.status}`);
          const blob = await fileResp.blob();
          const url = URL.createObjectURL(blob);
          const a = document.createElement('a');
          a.href = url;
          a.download = fileName;
          a.click();
          URL.revokeObjectURL(url);
        } catch (err) {
          if (err.message !== 'Unauthorized — please sign in again.') {
            alert(`Download failed: ${err.message}`);
          }
        }
      });
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load documents: ${err.message}</div>`);
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
        navigate('loads');
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
    navigate('home');
  } else {
    showLogin();
  }
}

document.addEventListener('DOMContentLoaded', boot);
