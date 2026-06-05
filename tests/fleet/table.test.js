import { describe, it, expect, vi } from 'vitest';
import { renderTable } from '../../static/fleet/components/table.js';

describe('renderTable', () => {
  it('renders headers and a row per item, escaping cell content', () => {
    const container = document.createElement('div');
    renderTable(container, {
      columns: [{ header: 'Name', cell: r => r.name }, { header: 'Status', cell: r => r.status }],
      rows: [{ id: '1', name: '<b>A</b>', status: 'active' }, { id: '2', name: 'B', status: 'inactive' }],
      onRowClick: () => {},
    });
    expect(container.querySelectorAll('thead th').length).toBe(2);
    expect(container.querySelectorAll('tbody tr').length).toBe(2);
    expect(container.querySelector('tbody tr td').innerHTML).toBe('&lt;b&gt;A&lt;/b&gt;');
  });

  it('trusts cell output as HTML when the column sets html: true', () => {
    const container = document.createElement('div');
    renderTable(container, {
      columns: [{ header: 'Status', cell: () => '<span class="badge">ok</span>', html: true }],
      rows: [{ id: '1' }],
    });
    expect(container.querySelector('tbody td .badge')).not.toBe(null);
  });

  it('invokes onRowClick with the row id', () => {
    const container = document.createElement('div');
    const onRowClick = vi.fn();
    renderTable(container, {
      columns: [{ header: 'Name', cell: r => r.name }],
      rows: [{ id: 'abc', name: 'A' }],
      onRowClick,
    });
    container.querySelector('tbody tr').click();
    expect(onRowClick).toHaveBeenCalledWith('abc');
  });
});
