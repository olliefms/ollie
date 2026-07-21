import { isAuthenticated } from './utils/auth.js';
import { tryRefresh } from './utils/api.js';
import { renderLogin } from './pages/login.js';
import { renderTrips } from './pages/trips.js';

const app = document.getElementById('app');

export function navigate(path) {
  history.pushState({}, '', path);
  route();
}

export function replaceNavigate(path) {
  history.replaceState({}, '', path);
  route();
}

async function route() {
  const path = window.location.pathname;

  // If no access token in storage, attempt a silent refresh via HttpOnly cookie
  // before deciding whether the user is authenticated.
  if (!isAuthenticated()) {
    await tryRefresh();
  }

  if (path === '/driver' || path === '/driver/') {
    if (isAuthenticated()) {
      replaceNavigate('/driver/trips');
    } else {
      renderLogin(app);
    }
    return;
  }

  if (path === '/driver/trips' || path === '/driver/trips/') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    renderTrips(app);
    return;
  }

  const tripDetailMatch = path.match(/^\/driver\/trips\/([a-f0-9-]+)$/);
  if (tripDetailMatch) {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderTripDetail } = await import('./pages/trip-detail.js');
    renderTripDetail(app, tripDetailMatch[1]);
    return;
  }

  const stopMatch = path.match(/^\/driver\/trips\/([a-f0-9-]+)\/stops\/(\d+)$/);
  if (stopMatch) {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderStopDetail } = await import('./pages/stop-detail.js');
    renderStopDetail(app, stopMatch[1], parseInt(stopMatch[2], 10));
    return;
  }

  if (path === '/driver/settings' || path === '/driver/settings/') {
    replaceNavigate('/driver/account');
    return;
  }

  if (path === '/driver/pay' || path === '/driver/pay/') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderPay } = await import('./pages/pay.js');
    renderPay(app);
    return;
  }

  if (path === '/driver/expenses' || path === '/driver/expenses/') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderExpenses } = await import('./pages/expenses.js');
    renderExpenses(app);
    return;
  }

  if (path === '/driver/equipment' || path === '/driver/equipment/') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderEquipment } = await import('./pages/equipment.js');
    renderEquipment(app);
    return;
  }

  if (path === '/driver/account' || path === '/driver/account/') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderAccount } = await import('./pages/account.js');
    renderAccount(app);
    return;
  }

  const notFoundDiv = document.createElement('div');
  notFoundDiv.style.padding = '2rem';
  const p = document.createElement('p');
  p.textContent = 'Page not found.';
  notFoundDiv.appendChild(p);
  app.appendChild(notFoundDiv);
}

if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/driver/sw.js').catch(console.error);
}

window.addEventListener('popstate', route);
route();
