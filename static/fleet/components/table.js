import { escHtml } from '../utils/format.js';

/**
 * Render a clickable list table into `container`.
 * opts: { columns: [{header, cell(row)->string}], rows: [{id, ...}], onRowClick(id) }
 *
 * NOTE: `cell(row)` must return PLAIN TEXT — its output is HTML-escaped by
 * default. To put rich content (e.g. a status badge) in a cell, set the
 * column's `html: true` flag; the cell's output is then trusted as HTML, so it
 * must come from a safe source (e.g. `badge()`, which escapes its own input).
 */
export function renderTable(container, { columns, rows, onRowClick }) {
  const head = columns.map(c => `<th>${escHtml(c.header)}</th>`).join('');
  const body = rows.map(r => {
    const cells = columns.map(c => {
      const out = c.cell(r);
      return `<td>${c.html ? String(out ?? '') : escHtml(String(out ?? ''))}</td>`;
    }).join('');
    return `<tr data-row-id="${escHtml(String(r.id))}">${cells}</tr>`;
  }).join('');

  container.innerHTML = `<table class="table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;

  if (onRowClick) {
    container.querySelectorAll('tbody tr').forEach(tr => {
      tr.addEventListener('click', () => onRowClick(tr.dataset.rowId));
    });
  }
}
