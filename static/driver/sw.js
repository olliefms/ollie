// CACHE VERSIONING: Bump this string whenever any file in STATIC_ASSETS changes.
// Format: 'driver-vYYYY-MM-DD' or 'driver-vN'. Failure to bump causes returning
// users to be served stale JS until the browser evicts the old cache.
// See release checklist in AGENTS.md.
const CACHE_NAME = 'ollie-v1.14.0';
const STATIC_ASSETS = [
  '/driver',
  '/driver/app.js?v=1.14.0',
  '/driver/css/base.css?v=1.14.0',
  '/driver/css/components.css?v=1.14.0',
  '/driver/pages/login.js?v=1.14.0',
  '/driver/pages/account.js?v=1.14.0',
  '/driver/pages/pay.js?v=1.14.0',
  '/driver/pages/trips.js?v=1.14.0',
  '/driver/pages/trips-current.js?v=1.14.0',
  '/driver/pages/trips-upcoming.js?v=1.14.0',
  '/driver/pages/trips-past.js?v=1.14.0',
  '/driver/pages/trip-detail.js?v=1.14.0',
  '/driver/pages/stop-detail.js?v=1.14.0',
  '/driver/utils/api.js?v=1.14.0',
  '/driver/utils/auth.js?v=1.14.0',
  '/driver/utils/format.js?v=1.14.0',
  '/driver/utils/week.js?v=1.14.0',
  '/driver/utils/time.js?v=1.14.0',
  '/driver/utils/version.js?v=1.14.0',
  '/driver/components/app-bar.js?v=1.14.0',
  '/driver/components/bottom-nav.js?v=1.14.0',
  '/driver/components/icons.js?v=1.14.0',
  '/driver/components/swipe.js?v=1.14.0',
  '/driver/components/trip-card.js?v=1.14.0',
  '/driver/components/week-stepper.js?v=1.14.0',
  '/driver/manifest.json',
  '/driver/icon-192.png',
  '/driver/icon-512.png',
];

self.addEventListener('install', event => {
  event.waitUntil(
    caches.open(CACHE_NAME)
      .then(c => c.addAll(STATIC_ASSETS))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', event => {
  event.waitUntil(
    caches.keys().then(keys =>
      Promise.all(keys.filter(k => k !== CACHE_NAME).map(k => caches.delete(k)))
    )
  );
  self.clients.claim();
});

self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  // Never cache API calls
  if (url.pathname.startsWith('/driver/api/v1/')) return;

  event.respondWith(
    caches.match(event.request).then(cached => {
      if (cached) return cached;
      return fetch(event.request).then(response => {
        if (response.ok && response.type === 'basic') {
          const clone = response.clone();
          caches.open(CACHE_NAME).then(c => c.put(event.request, clone));
        }
        return response;
      });
    })
  );
});
