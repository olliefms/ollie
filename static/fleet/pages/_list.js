import { escHtml } from '../utils/format.js';
import { renderTable } from '../components/table.js';
import { hasScope } from '../utils/api.js';
import { setContent, navigate } from '../utils/dom.js';

/**
 * Render a standard entity list page: header + optional scope-gated Create
 * button + a clickable renderTable. Rows navigate to `detailView`.
 *
 * opts: { title, columns, rows, detailView,
 *         createView, createScope, createLabel, emptyText }
 */
export function renderEntityList({
  title, columns, rows, detailView,
  createView, createScope, createLabel, emptyText,
}) {
  setContent(`
    <div class="page-header">
      <h1 class="page-title">${escHtml(title)}</h1>
      <div class="page-controls" id="list-controls"></div>
    </div>
    <div id="list-table"></div>
  `);

  if (createView && hasScope(createScope)) {
    const btn = document.createElement('button');
    btn.className = 'btn btn--primary';
    btn.textContent = createLabel || '+ Create';
    btn.addEventListener('click', () => navigate(createView));
    document.getElementById('list-controls').appendChild(btn);
  }

  const tableEl = document.getElementById('list-table');
  if (!rows.length) {
    tableEl.innerHTML = `<div class="state-empty">${escHtml(emptyText || 'No records found.')}</div>`;
    return;
  }
  renderTable(tableEl, {
    columns,
    rows,
    onRowClick: (id) => navigate(detailView, { id }),
  });
}
