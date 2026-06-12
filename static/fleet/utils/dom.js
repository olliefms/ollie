/* ============================================================
   Shared DOM + navigation helpers for the fleet SPA.
   Extracted from app.js (Phase 0b-ii Task 5) so that pages/
   modules can navigate and write content without importing
   from app.js (which would create an import cycle).
   ============================================================ */

import { navigate as routerNavigate } from '../router.js';

// Map a legacy view name (+ params) to a /fleet path, so view code can keep
// calling navigate('load-detail', { id }) and entity pages can use the same
// helper for create/edit/detail routes.
export const VIEW_PATHS = {
  home: () => '/fleet/home',
  loads: () => '/fleet/loads',
  'load-new': () => '/fleet/loads/new',
  'load-edit': (p) => `/fleet/loads/${p.id}/edit`,
  'load-detail': (p) => `/fleet/loads/${p.id}`,
  drivers: () => '/fleet/drivers',
  'driver-new': () => '/fleet/drivers/new',
  'driver-detail': (p) => `/fleet/drivers/${p.id}`,
  'driver-edit': (p) => `/fleet/drivers/${p.id}/edit`,
  trips: () => '/fleet/trips',
  'trip-new': () => '/fleet/trips/new',
  'trip-edit': (p) => `/fleet/trips/${p.id}/edit`,
  'trip-detail': (p) => `/fleet/trips/${p.id}`,
  events: () => '/fleet/events',
  documents: () => '/fleet/documents',
  document: (p) => `/fleet/documents/${p.id}`,
  terminals: () => '/fleet/terminals',
  'terminal-new': () => '/fleet/terminals/new',
  'terminal-detail': (p) => `/fleet/terminals/${p.id}`,
  'terminal-edit': (p) => `/fleet/terminals/${p.id}/edit`,
  trucks: () => '/fleet/trucks',
  'truck-new': () => '/fleet/trucks/new',
  'truck-detail': (p) => `/fleet/trucks/${p.id}`,
  'truck-edit': (p) => `/fleet/trucks/${p.id}/edit`,
  trailers: () => '/fleet/trailers',
  'trailer-new': () => '/fleet/trailers/new',
  'trailer-detail': (p) => `/fleet/trailers/${p.id}`,
  'trailer-edit': (p) => `/fleet/trailers/${p.id}/edit`,
  maintenance: () => '/fleet/maintenance',
  'maintenance-new': (p) => p && p.equipment_type
    ? `/fleet/maintenance/new?equipment_type=${encodeURIComponent(p.equipment_type)}&equipment_id=${encodeURIComponent(p.equipment_id)}`
    : '/fleet/maintenance/new',
  'maintenance-detail': (p) => `/fleet/maintenance/${p.id}`,
  'maintenance-edit': (p) => `/fleet/maintenance/${p.id}/edit`,
  facilities: () => '/fleet/facilities',
  'facility-new': () => '/fleet/facilities/new',
  'facility-detail': (p) => `/fleet/facilities/${p.id}`,
  'facility-edit': (p) => `/fleet/facilities/${p.id}/edit`,
  account: () => '/fleet/account',
};

/** Navigate by legacy view name (+ params), translating to a pushState path. */
export function navigate(view, params = {}) {
  const fn = VIEW_PATHS[view];
  routerNavigate(fn ? fn(params) : '/fleet/home');
}

/** Browser back. Router popstate handler re-renders. */
export function goBack() {
  history.back();
}

/** Replace the main content area's HTML. */
export function setContent(html) {
  document.getElementById('main-content').innerHTML = html;
}

/** Set the topbar refresh indicator text. */
export function setRefreshIndicator(msg) {
  const el = document.getElementById('refresh-indicator');
  if (el) el.textContent = msg;
}

/** Empty the topbar controls slot. Safe no-op if the slot is absent. */
export function clearTopbarControls() {
  const el = document.getElementById('topbar-controls');
  if (el) el.replaceChildren();
}

/**
 * Populate the topbar controls slot. Clears it first, then calls
 * `builderFn(slotEl)` so the caller can append its filter/select/buttons.
 * Safe no-op if the slot is absent.
 */
export function setTopbarControls(builderFn) {
  const el = document.getElementById('topbar-controls');
  if (!el) return;
  el.replaceChildren();
  if (builderFn) builderFn(el);
}
