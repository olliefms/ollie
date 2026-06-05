// Route table: each entry maps a name to a path regex. Detail routes capture id.
// `query` (raw query string) is surfaced in params when present.
export const ROUTES = [
  { name: 'home',            re: /^\/fleet\/home$/ },
  { name: 'loads',           re: /^\/fleet\/loads$/ },
  { name: 'load-detail',     re: /^\/fleet\/loads\/([^/]+)$/, id: true },
  { name: 'trips',           re: /^\/fleet\/trips$/ },
  { name: 'trip-detail',     re: /^\/fleet\/trips\/([^/]+)$/, id: true },
  { name: 'drivers',         re: /^\/fleet\/drivers$/ },
  { name: 'driver-detail',   re: /^\/fleet\/drivers\/([^/]+)$/, id: true },
  { name: 'terminals',       re: /^\/fleet\/terminals$/ },
  { name: 'trucks',          re: /^\/fleet\/trucks$/ },
  { name: 'trailers',        re: /^\/fleet\/trailers$/ },
  { name: 'facilities',      re: /^\/fleet\/facilities$/ },
  { name: 'events',          re: /^\/fleet\/events$/ },
  { name: 'documents',       re: /^\/fleet\/documents$/ },
  { name: 'document-detail', re: /^\/fleet\/documents\/([^/]+)$/, id: true },
  { name: 'account',         re: /^\/fleet\/account$/ },
];

/** Pure: map a path (with optional ?query) to { name, params }. */
export function matchRoute(rawPath) {
  const qIdx = rawPath.indexOf('?');
  const path = qIdx === -1 ? rawPath : rawPath.slice(0, qIdx);
  const query = qIdx === -1 ? '' : rawPath.slice(qIdx + 1);

  if (path === '/fleet' || path === '/fleet/') return { name: 'home', params: {} };

  for (const r of ROUTES) {
    const m = path.match(r.re);
    if (m) {
      const params = {};
      if (r.id) params.id = m[1];
      if (query) params.query = query;
      return { name: r.name, params };
    }
  }
  return { name: 'notfound', params: {} };
}

/** pushState navigate, then run the registered handler. */
let _onRoute = () => {};
export function navigate(path) {
  history.pushState({}, '', path);
  _onRoute(matchRoute(path));
}
export function replaceNavigate(path) {
  history.replaceState({}, '', path);
  _onRoute(matchRoute(path));
}

/** Wire popstate + intercept same-origin /fleet link clicks; fire onRoute now. */
export function startRouter(onRoute) {
  _onRoute = onRoute;
  window.addEventListener('popstate', () => _onRoute(matchRoute(location.pathname + location.search)));
  document.addEventListener('click', (e) => {
    const a = e.target.closest && e.target.closest('a[data-link]');
    if (!a) return;
    const href = a.getAttribute('href');
    if (href && href.startsWith('/fleet')) {
      e.preventDefault();
      navigate(href);
    }
  });
  _onRoute(matchRoute(location.pathname + location.search));
}
