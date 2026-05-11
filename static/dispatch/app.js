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
  loads: 'Loads',
  drivers: 'Drivers',
  events: 'Events',
};

function navigate(view, params = {}) {
  clearEventsRefresh();
  currentView = view;

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
    case 'loads':
      renderLoadsView(params);
      break;
    case 'load-detail':
      renderLoadDetailView(params.id);
      break;
    case 'drivers':
      renderDriversView();
      break;
    case 'events':
      renderEventsView();
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

// ─── Utility: set main content ───────────────────────────────

function setContent(html) {
  document.getElementById('main-content').innerHTML = html;
}

// ─── Utility: refresh indicator ──────────────────────────────

function setRefreshIndicator(msg) {
  const el = document.getElementById('refresh-indicator');
  if (el) el.textContent = msg;
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
  // Update top-bar title
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = `Load ${shortId(id)}`;

  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const [loadRes, tripsRes] = await Promise.all([
      apiFetch(`${API_BASE}/loads/${id}`),
      apiFetch(`${API_BASE}/trips?load_id=${id}`),
    ]);

    if (!loadRes.ok) throw new Error(`Load fetch HTTP ${loadRes.status}`);
    const load = await loadRes.json();

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
          <td>${stop.facility_name || stop.facility_id || '—'}</td>
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
          <td>${trip.driver_name || shortId(trip.driver_id) || '—'}</td>
          <td>${trip.truck_number || shortId(trip.truck_id) || '—'}</td>
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

    const html = `
      <button class="back-link" id="back-to-loads">← Back to Loads</button>

      <div class="detail-card">
        <div class="detail-card__title">Load Details</div>
        <div class="detail-grid">
          <div class="detail-item">
            <div class="detail-item__label">Load ID</div>
            <div class="detail-item__value" style="font-family: var(--font-mono); font-size: 0.875rem;">${load.id || '—'}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(load.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Customer</div>
            <div class="detail-item__value">${load.customer || '—'}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Trip Number</div>
            <div class="detail-item__value">${load.load_number || '—'}</div>
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
    `;

    setContent(html);

    document.getElementById('back-to-loads').addEventListener('click', () => {
      navigate('loads');
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
        const rowClass = isAvailable ? ' class="row--available"' : '';
        return `
          <tr${rowClass}>
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
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load data: ${err.message}</div>`);
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
    navigate('loads');
  } else {
    showLogin();
  }
}

document.addEventListener('DOMContentLoaded', boot);
