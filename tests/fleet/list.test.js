import { describe, it, expect, beforeEach } from 'vitest';
import { clearMe } from '../../static/fleet/utils/api.js';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { renderEntityList } from '../../static/fleet/pages/_list.js';

beforeEach(() => {
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
});

describe('renderEntityList', () => {
  const cols = [{ header: 'Name', cell: r => r.name }];
  const rows = [{ id: '1', name: 'Alpha' }];

  it('does not emit an in-content page title or page-header', () => {
    renderEntityList({ title: 'Widgets', columns: cols, rows, detailView: 'x' });
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-header');
    expect(main).not.toContain('page-title');
    expect(main).not.toContain('Widgets');
  });

  it('renders the table rows into #main-content', () => {
    renderEntityList({ title: 'Widgets', columns: cols, rows, detailView: 'x' });
    expect(document.getElementById('main-content').innerHTML).toContain('Alpha');
  });

  it('puts extraControls into #topbar-controls (not main-content)', () => {
    renderEntityList({
      title: 'Widgets', columns: cols, rows, detailView: 'x',
      extraControls: (slot) => {
        const s = document.createElement('select');
        s.id = 'probe-filter';
        slot.appendChild(s);
      },
    });
    expect(document.querySelector('#topbar-controls #probe-filter')).toBeTruthy();
    expect(document.querySelector('#main-content #probe-filter')).toBeFalsy();
  });

  it('shows the empty state in #main-content when there are no rows', () => {
    renderEntityList({
      title: 'Widgets', columns: cols, rows: [], detailView: 'x',
      emptyText: 'Nothing here.',
    });
    expect(document.getElementById('main-content').innerHTML).toContain('Nothing here.');
  });
});
