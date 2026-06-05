import { saveToken } from '../utils/auth.js';
import { loadMe, API_BASE, AUTH_BASE } from '../utils/api.js';
import { fmtDate } from '../utils/format.js';
import { clearEventsRefresh } from './events.js';

// ─── View/Auth toggle ────────────────────────────────────────

export function showLogin() {
  document.getElementById('login-view').hidden = false;
  document.getElementById('app-shell').hidden = true;
  // Default to the sign-in pane; showLoginOrSetup flips to setup if needed.
  const loginPane = document.getElementById('login-pane');
  const setupPane = document.getElementById('setup-pane');
  if (loginPane) loginPane.hidden = false;
  if (setupPane) setupPane.hidden = true;
  clearEventsRefresh();
}

export function showSetup() {
  document.getElementById('login-view').hidden = false;
  document.getElementById('app-shell').hidden = true;
  const loginPane = document.getElementById('login-pane');
  const setupPane = document.getElementById('setup-pane');
  if (loginPane) loginPane.hidden = true;
  if (setupPane) setupPane.hidden = false;
  clearEventsRefresh();
}

// Show the setup pane when no users exist yet, otherwise the sign-in pane.
export async function showLoginOrSetup() {
  try {
    const res = await fetch(`${API_BASE}/setup/status`);
    if (res.ok) {
      const data = await res.json();
      if (data.needs_setup) {
        showSetup();
        return;
      }
    }
  } catch (_) { /* fall through to login */ }
  showLogin();
}

export function showApp() {
  document.getElementById('login-view').hidden = true;
  document.getElementById('app-shell').hidden = false;
}

function showAlert(el, cls, msg) {
  el.className = `alert ${cls}`;
  el.textContent = msg;
  el.hidden = false;
}

// ─── Login + setup forms ─────────────────────────────────────
// `enterApp` is supplied by app.js (it owns the router/render dispatch).

export function initLoginForm(enterApp) {
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
        await loadMe();
        enterApp();
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

export function initSetupForm(enterApp) {
  const form = document.getElementById('setup-form');
  if (!form) return;

  form.addEventListener('submit', async (e) => {
    e.preventDefault();

    const alertEl = document.getElementById('setup-alert');
    const submitBtn = document.getElementById('setup-submit');
    const name = document.getElementById('setup-name').value.trim();
    const email = document.getElementById('setup-email').value.trim();
    const password = document.getElementById('setup-password').value;

    alertEl.hidden = true;
    alertEl.className = 'alert';
    alertEl.textContent = '';
    submitBtn.disabled = true;
    submitBtn.textContent = 'Creating…';

    try {
      const res = await fetch('/fleet/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'same-origin',
        body: JSON.stringify({ name, email, password }),
      });

      if (res.ok) {
        const data = await res.json();
        saveToken(data.token || data.access_token);
        await loadMe();
        enterApp();
        return;
      }

      if (res.status === 409 || res.status === 410) {
        showAlert(alertEl, 'alert--warning', 'Setup has already been completed. Please sign in.');
        showLogin();
        return;
      }

      showAlert(alertEl, 'alert--error', `Setup failed (HTTP ${res.status}). Please try again.`);
    } catch (err) {
      showAlert(alertEl, 'alert--error', `Network error: ${err.message}`);
    } finally {
      submitBtn.disabled = false;
      submitBtn.textContent = 'Create owner account';
    }
  });
}
