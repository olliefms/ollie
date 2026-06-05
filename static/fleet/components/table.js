import { escHtml } from '../utils/format.js';

/**
 * Render a clickable list table into `container`.
 * opts: { columns: [{header, cell(row)->string}], rows: [{id, ...}], onRowClick(id) }
 */
export function renderTable(container, { columns, rows, onRowClick }) {
  const head = columns.map(c => `<th>${escHtml(c.header)}</th>`).join('');
  const body = rows.map(r => {
    const cells = columns.map(c => `<td>${escHtml(String(c.cell(r) ?? ''))}</td>`).join('');
    return `<tr data-row-id="${escHtml(String(r.id))}">${cells}</tr>`;
  }).join('');

  container.innerHTML = `<table class="table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;

  if (onRowClick) {
    container.querySelectorAll('tbody tr').forEach(tr => {
      tr.addEventListener('click', () => onRowClick(tr.dataset.rowId));
    });
  }
}
