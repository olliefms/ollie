/* ============================================================
   Ollie Fleet — SPA
   ES-module entry. Shell + router + boot only: every view lives in
   pages/, shared logic in utils/ + components/. This file wires the
   route table to the page modules and owns the login gate.
   ============================================================ */

import { isAuthenticated, clearToken } from './utils/auth.js';
import {
  tryRefresh, loadMe, clearMe, setOnUnauthorized, getScopes, getIdentity,
} from './utils/api.js';
import {
  matchRoute, replaceNavigate, startRouter,
} from './router.js';
import {
  setRefreshIndicator,
} from './utils/dom.js';
import { renderSidebar } from './components/nav.js';
import { renderAccountFooter } from './components/account-footer.js';
import { initTheme } from './utils/theme.js';
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
import { renderTripDetail } from './pages/trip-detail.js';

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
let meRefreshing = false;

// Show the app shell and (idempotently) start the router. After the first call,
// re-render the current route instead of re-wiring popstate/click listeners.
function enterApp() {
  showApp();
  renderChrome();
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
    case 'trip-detail': renderTripDetail(params.id); break;
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

// ─── Sidebar & account footer ────────────────────────────────

async function signOut() {
  await fetch('/fleet/auth/logout', {
    method: 'POST',
    credentials: 'same-origin',
  }).catch(() => {});
  clearToken();
  clearMe();
  clearEventsRefresh();
  showLogin();
}

// Render the scope-gated nav + account footer from the current /me snapshot.
// Safe to call repeatedly (boot, login, tab refocus).
function renderChrome() {
  const scopes = getScopes();
  const navEl = document.getElementById('sidebar-nav');
  const footerEl = document.getElementById('sidebar-footer');
  if (navEl) {
    renderSidebar(navEl, { scopes, pathname: window.location.pathname });
  }
  if (footerEl) {
    renderAccountFooter(footerEl, {
      identity: getIdentity(),
      scopes,
      onSignOut: signOut,
    });
  }
}

// ─── Boot ────────────────────────────────────────────────────

async function boot() {
  initTheme();
  initLoginForm(enterApp);
  initSetupForm(enterApp);

  // A 401 from any apiFetch (after a failed refresh) drops back to the login pane.
  setOnUnauthorized(() => {
    clearMe();
    showLogin();
  });

  // Keep effective scopes fresh while the tab is open: reload /me when the tab
  // regains visibility, then re-render the scope-gated chrome.
  document.addEventListener('visibilitychange', async () => {
    if (document.visibilityState === 'visible' && isAuthenticated() && !meRefreshing) {
      meRefreshing = true;
      try {
        await loadMe();
        renderChrome();
      } finally {
        meRefreshing = false;
      }
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
