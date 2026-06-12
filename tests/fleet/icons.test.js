import { describe, it, expect } from 'vitest';
import * as icons from '../../static/fleet/components/icons.js';

const NAMES = [
  'homeIcon', 'loadsIcon', 'tripsIcon', 'eventsIcon', 'driversIcon',
  'trucksIcon', 'trailersIcon', 'facilitiesIcon', 'terminalsIcon',
  'documentsIcon', 'keyIcon', 'chevronUpIcon', 'themeIcon', 'logoutIcon',
];

describe('fleet icons', () => {
  it('exports a factory for every nav + footer icon', () => {
    for (const name of NAMES) {
      expect(typeof icons[name]).toBe('function');
    }
  });

  it('each factory returns a fresh <svg> element', () => {
    for (const name of NAMES) {
      const a = icons[name]();
      const b = icons[name]();
      expect(a.tagName.toLowerCase()).toBe('svg');
      expect(a).not.toBe(b);
    }
  });

  it('icons use currentColor so they inherit link color', () => {
    expect(icons.homeIcon().getAttribute('stroke')).toBe('currentColor');
  });
});
