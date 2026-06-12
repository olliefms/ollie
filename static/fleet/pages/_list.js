import { escHtml } from '../utils/format.js';
import { renderTable } from '../components/table.js';
import { hasScope } from '../utils/api.js';
import { setContent, navigate, setTopbarControls } from '../utils/dom.js';

/**
 * Render a standard entity list page. The title is shown by the shell topbar
 * (set in app.js from VIEW_TITLES); this helper puts the page's filters
 * (extraControls) and the scope-gated Create button into the topbar controls
 * slot, and renders only the table into the content area.
 *
 * opts: { title, columns, rows, detailView,
 *         createView, createScope, createLabel, emptyText,
 *         rowClass, extraControls }
 * `title` is accepted for call-site compatibility but not rendered in-content.
 */
export function renderEntityList({
  columns, rows, detailView,
  createView, createScope, createLabel, emptyText,
  rowClass, extraControls,
}) {
  // Filters first, primary action (Create) last so it anchors the far right.
  setTopbarControls((slot) => {
    if (extraControls) extraControls(slot);
    if (createView && hasScope(createScope)) {
      const btn = document.createElement('button');
      btn.className = 'btn btn--primary';
      btn.textContent = createLabel || '+ Create';
      btn.addEventListener('click', () => navigate(createView));
      slot.appendChild(btn);
    }
  });

  setContent('<div id="list-table"></div>');

  const tableEl = document.getElementById('list-table');
  if (!rows.length) {
    tableEl.innerHTML = `<div class="state-empty">${escHtml(emptyText || 'No records found.')}</div>`;
    return;
  }
  renderTable(tableEl, {
    columns,
    rows,
    onRowClick: (id) => navigate(detailView, { id }),
    rowClass,
  });
}
