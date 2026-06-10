/* ============================================================
   Ollie Fleet — SPA
   ES-module entry. Shared logic lives in utils/ + components/.
   Read-only views (home, events, documents, account, login) and
   the navigation/DOM helpers live in pages/ + utils/. The entity
   views below (loads/trips) are still inline and migrate to
   pages/ + CRUD in their own phases.
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
import { renderDriversView } from './pages/drivers.js';
import { renderDriverDetail } from './pages/driver-detail.js';
import { renderDriverForm } from './pages/driver-form.js';
import { renderFacilitiesView } from './pages/facilities.js';
import { renderFacilityDetail } from './pages/facility-detail.js';
import { renderFacilityForm } from './pages/facility-form.js';
import { renderLoadForm } from './pages/load-form.js';
import { renderTripForm } from './pages/trip-form.js';
import { renderLoadsView } from './pages/loads.js';
import { renderLoadDetail } from './pages/load-detail.js';
import { renderTripsView } from './pages/trips.js';

// ─── Navigation ──────────────────────────────────────────────

const VIEW_TITLES = {
  home: 'Home',
  loads: 'Loads',
  'load-new': 'New Load',
  'load-edit': 'Edit Load',
  'load-detail': 'Load Detail',
  drivers: 'Drivers',
  'driver-new': 'New Driver',
  'driver-detail': 'Driver',
  'driver-edit': 'Edit Driver',
  trips: 'Trips',
  'trip-new': 'New Trip',
  'trip-edit': 'Edit Trip',
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
  'facility-new': 'New Facility',
  'facility-detail': 'Facility',
  'facility-edit': 'Edit Facility',
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

  switch (name) {
    case 'home': renderHomeView(); break;
    case 'loads': renderLoadsView(params); break;
    case 'load-new': renderLoadForm(null); break;
    case 'load-edit': renderLoadForm(params.id); break;
    case 'load-detail': renderLoadDetail(params.id); break;
    case 'drivers': renderDriversView(); break;
    case 'driver-new': renderDriverForm(null); break;
    case 'driver-detail': renderDriverDetail(params.id); break;
    case 'driver-edit': renderDriverForm(params.id); break;
    case 'trips': renderTripsView(params); break;
    case 'trip-new': renderTripForm(null); break;
    case 'trip-edit': renderTripForm(params.id); break;
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
    case 'facilities': renderFacilitiesView(); break;
    case 'facility-new': renderFacilityForm(null); break;
    case 'facility-detail': renderFacilityDetail(params.id); break;
    case 'facility-edit': renderFacilityForm(params.id); break;
    case 'account': renderAccountView(); break;
    default: replaceNavigate('/fleet/home');
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
