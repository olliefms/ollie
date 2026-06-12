import {
  homeIcon, loadsIcon, tripsIcon, eventsIcon, driversIcon,
  trucksIcon, trailersIcon, maintenanceIcon, facilitiesIcon, terminalsIcon, documentsIcon,
} from './icons.js';
import { scopeGranted } from './scope-gate.js';

export const NAV_GROUPS = [
  { label: null, items: [
    { label: 'Home', path: '/fleet/home', icon: homeIcon },
  ] },
  { label: 'Operations', items: [
    { label: 'Loads',  path: '/fleet/loads',  icon: loadsIcon,  scope: 'loads:read' },
    { label: 'Trips',  path: '/fleet/trips',  icon: tripsIcon,  scope: 'trips:read' },
    { label: 'Events', path: '/fleet/events', icon: eventsIcon, scope: 'events:read' },
  ] },
  { label: 'Fleet', items: [
    { label: 'Drivers',  path: '/fleet/drivers',  icon: driversIcon,  scope: 'drivers:read' },
    { label: 'Trucks',   path: '/fleet/trucks',   icon: trucksIcon,   scope: 'trucks:read' },
    { label: 'Trailers', path: '/fleet/trailers', icon: trailersIcon, scope: 'trailers:read' },
    { label: 'Maintenance', path: '/fleet/maintenance', icon: maintenanceIcon, scope: 'maintenance:read' },
  ] },
  { label: 'Network', items: [
    { label: 'Facilities', path: '/fleet/facilities', icon: facilitiesIcon, scope: 'facilities:read' },
    { label: 'Terminals',  path: '/fleet/terminals',  icon: terminalsIcon,  scope: 'terminals:read' },
  ] },
  { label: 'Admin', items: [
    { label: 'Documents', path: '/fleet/documents', icon: documentsIcon, scope: 'blobs:read' },
  ] },
];

export function visibleGroups(scopes) {
  return NAV_GROUPS
    .map(g => ({
      label: g.label,
      items: g.items.filter(it => !it.scope || scopeGranted(scopes, it.scope)),
    }))
    .filter(g => g.items.length > 0);
}

export function renderSidebar(container, { scopes = [], pathname = '' } = {}) {
  container.replaceChildren();
  for (const group of visibleGroups(scopes)) {
    if (group.label) {
      const header = document.createElement('div');
      header.className = 'sidebar__group-label';
      header.textContent = group.label;
      container.appendChild(header);
    }
    for (const item of group.items) {
      const a = document.createElement('a');
      a.className = 'sidebar__link';
      a.dataset.link = '';
      a.setAttribute('href', item.path);
      if (item.path === pathname) a.classList.add('sidebar__link--active');

      const iconWrap = document.createElement('span');
      iconWrap.className = 'sidebar__icon';
      iconWrap.appendChild(item.icon());

      const label = document.createElement('span');
      label.textContent = item.label;

      a.append(iconWrap, label);
      container.appendChild(a);
    }
  }
}
