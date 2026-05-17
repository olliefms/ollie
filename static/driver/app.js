import { isAuthenticated } from './utils/auth.js';
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

  if (path === '/driver/settings') {
    if (!isAuthenticated()) {
      replaceNavigate('/driver');
      return;
    }
    const { renderSettings } = await import('./pages/settings.js');
    renderSettings(app);
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
