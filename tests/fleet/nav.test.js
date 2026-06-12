import { describe, it, expect, beforeEach } from 'vitest';
import { NAV_GROUPS, visibleGroups, renderSidebar } from '../../static/fleet/components/nav.js';

const ALL = ['*'];
const DISPATCHER = [
  'loads:read', 'trips:read', 'events:read', 'drivers:read',
  'trucks:read', 'trailers:read', 'facilities:read', 'terminals:read', 'blobs:read',
];

describe('visibleGroups', () => {
  it('shows Home only when no scopes are present', () => {
    const groups = visibleGroups([]);
    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe(null);
    expect(groups[0].items.map(i => i.label)).toEqual(['Home']);
  });

  it('superuser sees every group and item', () => {
    const groups = visibleGroups(ALL);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations', 'Fleet', 'Network', 'Admin']);
    const labels = groups.flatMap(g => g.items.map(i => i.label));
    expect(labels).toEqual([
      'Home', 'Loads', 'Trips', 'Events', 'Drivers', 'Trucks',
      'Trailers', 'Facilities', 'Terminals', 'Documents',
    ]);
  });

  it('drops a group whose every item is scope-hidden', () => {
    const scopes = ['loads:read'];
    const groups = visibleGroups(scopes);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations']);
    expect(groups.find(g => g.label === 'Operations').items.map(i => i.label)).toEqual(['Loads']);
  });

  it('dispatcher sees all operational groups', () => {
    const groups = visibleGroups(DISPATCHER);
    expect(groups.map(g => g.label)).toEqual([null, 'Operations', 'Fleet', 'Network', 'Admin']);
  });
});

describe('renderSidebar', () => {
  let host;
  beforeEach(() => { host = document.createElement('div'); });

  it('renders data-link anchors with icon + label', () => {
    renderSidebar(host, { scopes: ALL, pathname: '/fleet/loads' });
    const links = host.querySelectorAll('a.sidebar__link');
    expect(links.length).toBe(10);
    const loads = [...links].find(a => a.getAttribute('href') === '/fleet/loads');
    expect(loads.hasAttribute('data-link')).toBe(true);
    expect(loads.querySelector('svg')).not.toBe(null);
    expect(loads.textContent).toContain('Loads');
  });

  it('marks the current path active', () => {
    renderSidebar(host, { scopes: ALL, pathname: '/fleet/loads' });
    const active = host.querySelectorAll('.sidebar__link--active');
    expect(active.length).toBe(1);
    expect(active[0].getAttribute('href')).toBe('/fleet/loads');
  });

  it('renders group headers only for non-empty labelled groups', () => {
    renderSidebar(host, { scopes: ['loads:read'], pathname: '/fleet/home' });
    const headers = [...host.querySelectorAll('.sidebar__group-label')].map(h => h.textContent);
    expect(headers).toEqual(['Operations']);
  });

  it('clears prior content on re-render', () => {
    renderSidebar(host, { scopes: ALL, pathname: '' });
    renderSidebar(host, { scopes: [], pathname: '' });
    expect(host.querySelectorAll('a.sidebar__link').length).toBe(1);
  });
});
