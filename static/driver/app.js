import { isAuthenticated } from './utils/auth.js';
import { renderLogin } from './pages/login.js';
import { renderTrips } from './pages/trips.js';

const app = document.getElementById('app');

function route() {
  const path = window.location.pathname;

  if (path === '/driver' || path === '/driver/') {
    if (isAuthenticated()) {
      window.location.replace('/driver/trips');
    } else {
      renderLogin(app);
    }
    return;
  }

  // Trip list page
  if (path === '/driver/trips' || path === '/driver/trips/') {
    if (!isAuthenticated()) {
      window.location.replace('/driver');
      return;
    }
    renderTrips(app);
    return;
  }

  // Trip detail page
  const tripDetailMatch = path.match(/^\/driver\/trips\/([a-f0-9-]+)$/);
  if (tripDetailMatch) {
    if (!isAuthenticated()) {
      window.location.replace('/driver');
      return;
    }
    const { renderTripDetail } = await import('./pages/trip-detail.js');
    renderTripDetail(app, tripDetailMatch[1]);
    return;
  }

  // Stop detail page
  const stopMatch = path.match(/^\/driver\/trips\/([a-f0-9-]+)\/stops\/(\d+)$/);
  if (stopMatch) {
    if (!isAuthenticated()) {
      window.location.replace('/driver');
      return;
    }
    const { renderStopDetail } = await import('./pages/stop-detail.js');
    renderStopDetail(app, stopMatch[1], parseInt(stopMatch[2], 10));
    return;
  }

  // 404 fallback
  const notFoundDiv = document.createElement('div');
  notFoundDiv.style.padding = '2rem';
  const p = document.createElement('p');
  p.textContent = 'Page not found.';
  notFoundDiv.appendChild(p);
  app.appendChild(notFoundDiv);
}

// Register service worker
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/driver/sw.js').catch(console.error);
}

// Handle browser navigation
window.addEventListener('popstate', route);
route();
