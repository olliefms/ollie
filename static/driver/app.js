import { isAuthenticated } from './utils/auth.js';
import { renderLogin } from './pages/login.js';

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

  if (path.startsWith('/driver/trips')) {
    if (!isAuthenticated()) {
      window.location.replace('/driver');
      return;
    }
    // #53 will add renderTripList; stub for now
    const stubDiv = document.createElement('div');
    stubDiv.style.padding = '2rem';
    const p = document.createElement('p');
    p.textContent = 'Loading trips...';
    stubDiv.appendChild(p);
    app.appendChild(stubDiv);
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
