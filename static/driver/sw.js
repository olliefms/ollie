// CACHE VERSIONING: Bump this string whenever any file in STATIC_ASSETS changes.
// Format: 'driver-vYYYY-MM-DD' or 'driver-vN'. Failure to bump causes returning
// users to be served stale JS until the browser evicts the old cache.
// See release checklist in AGENTS.md.
const CACHE_NAME = 'ollie-v1.6.0';
const STATIC_ASSETS = [
  '/driver',
  '/driver/app.js',
  '/driver/css/base.css',
  '/driver/css/components.css',
  '/driver/pages/login.js',
  '/driver/pages/settings.js',
  '/driver/pages/trip-detail.js',
  '/driver/pages/stop-detail.js',
  '/driver/utils/api.js',
  '/driver/utils/auth.js',
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
