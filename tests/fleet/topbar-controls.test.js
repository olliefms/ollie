import { describe, it, expect, beforeEach } from 'vitest';
import { setTopbarControls, clearTopbarControls } from '../../static/fleet/utils/dom.js';

beforeEach(() => {
  document.body.innerHTML = '<div id="topbar-controls"></div>';
});

describe('topbar controls slot', () => {
  it('setTopbarControls populates the slot via the builder', () => {
    setTopbarControls((slot) => {
      const btn = document.createElement('button');
      btn.id = 'probe';
      slot.appendChild(btn);
    });
    expect(document.querySelector('#topbar-controls #probe')).toBeTruthy();
  });

  it('setTopbarControls clears prior content before building', () => {
    setTopbarControls((slot) => { slot.appendChild(document.createElement('span')); });
    setTopbarControls((slot) => {
      const b = document.createElement('button');
      b.id = 'second';
      slot.appendChild(b);
    });
    const slot = document.getElementById('topbar-controls');
    expect(slot.querySelectorAll('span').length).toBe(0);
    expect(slot.querySelector('#second')).toBeTruthy();
  });

  it('clearTopbarControls empties the slot', () => {
    setTopbarControls((slot) => { slot.appendChild(document.createElement('button')); });
    clearTopbarControls();
    expect(document.getElementById('topbar-controls').children.length).toBe(0);
  });

  it('helpers no-op safely when the slot is absent', () => {
    document.body.innerHTML = '';
    expect(() => clearTopbarControls()).not.toThrow();
    expect(() => setTopbarControls(() => {})).not.toThrow();
  });
});
