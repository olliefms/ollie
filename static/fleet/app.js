/* ============================================================
   Ollie Fleet — SPA
   ES-module entry. Shared logic lives in utils/ + components/.
   Read-only views (home, events, documents, account, login) and
   the navigation/DOM helpers live in pages/ + utils/. The entity
   views below (loads/trips/drivers/terminals) are still inline and
   migrate to pages/ + CRUD in their own phases.
   ============================================================ */

import { isAuthenticated, clearToken } from './utils/auth.js';
import {
  apiFetch, tryRefresh, API_BASE,
  loadMe, clearMe, setOnUnauthorized,
} from './utils/api.js';
import {
  escHtml, badge, shortId, fmtDate, fmtArrivalWindow,
  fmtBytes, fmtUSD, fmtMiles,
} from './utils/format.js';
import {
  matchRoute, replaceNavigate, startRouter,
} from './router.js';
import {
  setContent, setRefreshIndicator, navigate, goBack,
} from './utils/dom.js';
import { renderPlaceholder } from './pages/placeholder.js';
import { renderHomeView } from './pages/home.js';
import { renderEventsView, clearEventsRefresh } from './pages/events.js';
import { renderDocumentsView } from './pages/documents.js';
import { renderDocumentDetailView, revokeActiveObjectUrl } from './pages/document-detail.js';
import { renderAccountView } from './pages/account.js';
import {
  showLogin, showLoginOrSetup, initLoginForm, initSetupForm,
} from './pages/login.js';
import { renderTerminalsView } from './pages/terminals.js';
import { renderTerminalDetail } from './pages/terminal-detail.js';
import { renderTerminalForm } from './pages/terminal-form.js';
import { renderTrucksView } from './pages/trucks.js';
import { renderTruckDetail } from './pages/truck-detail.js';
import { renderTruckForm } from './pages/truck-form.js';
import { renderTrailersView } from './pages/trailers.js';
import { renderTrailerDetail } from './pages/trailer-detail.js';
import { renderTrailerForm } from './pages/trailer-form.js';

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
  'document-detail': 'Document',
  terminals: 'Terminals',
  'terminal-new': 'New Terminal',
  'terminal-detail': 'Terminal',
  'terminal-edit': 'Edit Terminal',
  trucks: 'Trucks',
  'truck-new': 'New Truck',
  'truck-detail': 'Truck',
  'truck-edit': 'Edit Truck',
  trailers: 'Trailers',
  'trailer-new': 'New Trailer',
  'trailer-detail': 'Trailer',
  'trailer-edit': 'Edit Trailer',
  facilities: 'Facilities',
  account: 'Account',
};

// ─── pushState routing ───────────────────────────────────────

let routerStarted = false;

// Show the app shell and (idempotently) start the router. After the first call,
// re-render the current route instead of re-wiring popstate/click listeners.
function enterApp() {
  showApp();
  if (!routerStarted) {
    routerStarted = true;
    startRouter(renderRoute);
  } else {
    renderRoute(matchRoute(window.location.pathname + window.location.search));
  }
}

function showApp() {
  document.getElementById('login-view').hidden = true;
  document.getElementById('app-shell').hidden = false;
}

function renderRoute({ name, params }) {
  clearEventsRefresh();
  revokeActiveObjectUrl();

  // Active sidebar link by current path.
  document.querySelectorAll('.sidebar__link[href]').forEach((a) => {
    a.classList.toggle('sidebar__link--active', a.getAttribute('href') === window.location.pathname);
  });

  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = VIEW_TITLES[name] || name;
  setRefreshIndicator('');

  const main = document.getElementById('main-content');

  switch (name) {
    case 'home': renderHomeView(); break;
    case 'loads': renderLoadsView(params); break;
    case 'load-detail': renderLoadDetailView(params.id); break;
    case 'drivers': renderDriversView(); break;
    case 'driver-detail': renderDriverDetailView(params.id); break;
    case 'trips': renderTripsView(params); break;
    case 'trip-detail': renderTripDetailView(params.id); break;
    case 'events': renderEventsView(); break;
    case 'documents': renderDocumentsView(params); break;
    case 'document-detail': renderDocumentDetailView(params.id); break;
    case 'terminals': renderTerminalsView(); break;
    case 'terminal-new': renderTerminalForm(null); break;
    case 'terminal-detail': renderTerminalDetail(params.id); break;
    case 'terminal-edit': renderTerminalForm(params.id); break;
    case 'trucks': renderTrucksView(); break;
    case 'truck-new': renderTruckForm(null); break;
    case 'truck-detail': renderTruckDetail(params.id); break;
    case 'truck-edit': renderTruckForm(params.id); break;
    case 'trailers': renderTrailersView(); break;
    case 'trailer-new': renderTrailerForm(null); break;
    case 'trailer-detail': renderTrailerDetail(params.id); break;
    case 'trailer-edit': renderTrailerForm(params.id); break;
    case 'facilities': renderPlaceholder(main, 'Facilities'); break;
    case 'account': renderAccountView(); break;
    default: replaceNavigate('/fleet/home');
  }
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
      const legs = (load.mileage_summary && load.mileage_summary.legs) || [];
      const stopRows = stops.map((stop, i) => {
        // For loads, legs[0] = stop_1 → stop_2. Stop 1 (i=0) has no inbound miles.
        const milesCell = i === 0 ? '—' : fmtMiles(legs[i - 1] ? legs[i - 1].miles : null);
        return `
        <tr>
          <td>${i + 1}</td>
          <td>${escHtml(stop.facility_name || '—')}</td>
          <td>${escHtml(stop.stop_type || '—')}</td>
          <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
          <td>${fmtDate(stop.actual_arrive)}</td>
          <td>${fmtDate(stop.actual_depart)}</td>
          <td style="text-align:right; font-variant-numeric: tabular-nums;">${milesCell}</td>
        </tr>
      `;
      }).join('');

      const totalMiles = load.mileage_summary ? fmtMiles(load.mileage_summary.total_miles) : '—';

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
                  <th style="text-align:right;">Miles</th>
                </tr>
              </thead>
              <tbody>${stopRows}</tbody>
              <tfoot>
                <tr><td colspan="6" style="font-weight:600;">Total Miles</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${totalMiles}</td></tr>
              </tfoot>
            </table>
          </div>
        </div>
      `;
    }

    // Build trips section
    let tripsHtml = '';
    if (trips.length > 0) {
      const tripRows = trips.map(trip => `
        <tr data-trip-id="${trip.id}" style="cursor:pointer;">
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
              <tbody id="load-trips-tbody">${tripRows}</tbody>
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

    let rateHtml = '';
    const rateItems = load.rate_items || [];
    if (rateItems.length > 0) {
      const rateRows = rateItems.map(r => {
        const negStyle = r.amount_usd < 0 ? ' style="color: var(--color-danger, #b91c1c);"' : '';
        return `
          <tr>
            <td>${escHtml(r.description || '—')}</td>
            <td${negStyle} style="text-align:right; font-variant-numeric: tabular-nums;${r.amount_usd < 0 ? ' color: var(--color-danger, #b91c1c);' : ''}">${fmtUSD(r.amount_usd)}</td>
          </tr>
        `;
      }).join('');
      rateHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Rate</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead><tr><th>Description</th><th style="text-align:right;">Amount</th></tr></thead>
              <tbody>${rateRows}</tbody>
              <tfoot>
                <tr><td style="font-weight:600;">Total</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${fmtUSD(load.total_rate_usd)}</td></tr>
              </tfoot>
            </table>
          </div>
        </div>
      `;
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

      ${rateHtml}
      ${stopsHtml}
      ${tripsHtml}
      ${docsHtml}
    `;

    setContent(html);

    document.getElementById('back-to-loads').addEventListener('click', goBack);

    document.querySelectorAll('#load-trips-tbody tr[data-trip-id]').forEach(row => {
      row.addEventListener('click', () => navigate('trip-detail', { id: row.dataset.tripId }));
    });

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

    const ms = trip.mileage_summary;
    const hasOrigin = !!(ms && ms.origin);
    const legs = (ms && ms.legs) || [];

    // Leg-index contract:
    //  - origin present: legs[0] is deadhead (origin → stop_1), legs[1+] loaded between stops
    //    => stop i (1-based) inbound miles = legs[i-1]
    //  - origin absent: legs[0] is stop_1 → stop_2
    //    => stop i (1-based, i>1) inbound miles = legs[i-2]; stop 1 has none
    const milesForStop = (i /* 0-based stop index */) => {
      if (hasOrigin) {
        return fmtMiles(legs[i] ? legs[i].miles : null);
      }
      if (i === 0) return '—';
      return fmtMiles(legs[i - 1] ? legs[i - 1].miles : null);
    };

    const originRow = hasOrigin ? `
      <tr>
        <td>0</td>
        <td>${escHtml(ms.origin.facility_name || '—')}${ms.origin.address ? ` — ${escHtml(ms.origin.address)}` : ''}</td>
        <td>origin</td>
        <td>—</td>
        <td>—</td>
        <td>—</td>
        <td style="text-align:right; font-variant-numeric: tabular-nums;">—</td>
      </tr>
    ` : '';

    const stopRows = (trip.stops || []).map((stop, i) => `
      <tr>
        <td>${i + 1}</td>
        <td>${escHtml(stop.name || '—')}</td>
        <td>${escHtml(stop.stop_type || '—')}</td>
        <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
        <td>${fmtDate(stop.actual_arrive)}</td>
        <td>${fmtDate(stop.actual_depart)}</td>
        <td style="text-align:right; font-variant-numeric: tabular-nums;">${milesForStop(i)}</td>
      </tr>
    `).join('');

    const totalMiles = ms ? fmtMiles(ms.total_miles) : '—';
    const bodyRows = (originRow + stopRows) || '<tr><td colspan="7" style="text-align:center; padding: var(--space-4); color: var(--color-text-muted);">No stops</td></tr>';

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
            <thead><tr><th>#</th><th>Facility</th><th>Type</th><th>Scheduled Arrive</th><th>Actual Arrive</th><th>Actual Depart</th><th style="text-align:right;">Miles</th></tr></thead>
            <tbody>${bodyRows}</tbody>
            <tfoot>
              <tr><td colspan="6" style="font-weight:600;">Total Miles</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${totalMiles}</td></tr>
            </tfoot>
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

// ─── Sidebar & logout ────────────────────────────────────────

function initSidebar() {
  // Sidebar items are <a data-link href> — the router intercepts their clicks.
  const logoutBtn = document.getElementById('logout-btn');
  if (logoutBtn) {
    logoutBtn.addEventListener('click', async () => {
      await fetch('/fleet/auth/logout', {
        method: 'POST',
        credentials: 'same-origin',
      }).catch(() => {});
      clearToken();
      clearMe();
      clearEventsRefresh();
      showLogin();
    });
  }
}

// ─── Boot ────────────────────────────────────────────────────

async function boot() {
  initLoginForm(enterApp);
  initSetupForm(enterApp);
  initSidebar();

  // A 401 from any apiFetch (after a failed refresh) drops back to the login pane.
  setOnUnauthorized(() => {
    clearMe();
    showLogin();
  });

  // Keep effective scopes fresh while the tab is open: reload /me when the tab
  // regains visibility (token refresh already reloads scopes via login flow).
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && isAuthenticated()) {
      loadMe();
    }
  });

  if (isAuthenticated()) {
    await loadMe();
    enterApp();
  } else {
    const refreshed = await tryRefresh();
    if (refreshed) {
      await loadMe();
      enterApp();
    } else {
      await showLoginOrSetup();
    }
  }
}

document.addEventListener('DOMContentLoaded', boot);
