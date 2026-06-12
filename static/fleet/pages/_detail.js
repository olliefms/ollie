import { escHtml } from '../utils/format.js';
import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import { setContent, goBack, navigate } from '../utils/dom.js';

/**
 * Build an anchor that navigates to another entity's detail page. Use inside a
 * field's `html` so related records (assigned driver, attached equipment) are
 * clickable. Click handling is wired centrally in renderDetailPage.
 */
export function detailLink(view, id, text) {
  return `<a href="#" class="detail-link" data-nav-view="${escHtml(view)}" data-nav-id="${escHtml(id)}">${escHtml(text)}</a>`;
}

/**
 * Equipment → driver is a reverse lookup: the assignment lives on the driver
 * (current_truck_id / current_trailer_ids), so scan the drivers list for the
 * one matching `matches`. Returns a clickable driver link, or '—' if unassigned.
 */
export async function assignedDriverLink(matches) {
  try {
    const res = await apiFetch(`${API_BASE}/drivers`);
    if (res.ok) {
      const data = await res.json();
      const drivers = data.items || (Array.isArray(data) ? data : []);
      const d = drivers.find(matches);
      if (d) return detailLink('driver-detail', d.id, d.name || d.id);
    }
  } catch (_) { /* fall through to unassigned */ }
  return '—';
}

/**
 * Render a standard entity detail page: back link + field grid + a row of
 * scope-gated action buttons + an inline status alert (for surfacing 409
 * referrer-conflict messages from delete).
 *
 * opts: {
 *   title,
 *   fields:  [{ label, value, html }],   // html wins over value; '—' for blank
 *   actions: [{ label, scope, className, onClick(statusEl) }],
 * }
 */
export function renderDetailPage({ title, fields, actions = [] }) {
  const grid = fields.map(f => {
    const value = f.html != null
      ? f.html
      : escHtml(f.value == null || f.value === '' ? '—' : String(f.value));
    return `
      <div class="detail-item">
        <div class="detail-item__label">${escHtml(f.label)}</div>
        <div class="detail-item__value">${value}</div>
      </div>`;
  }).join('');

  setContent(`
    <button class="back-link" id="detail-back">← Back</button>
    <div class="detail-card">
      <div class="detail-card__title">${escHtml(title)}</div>
      <div class="detail-grid">${grid}</div>
      <div class="form-panel__actions" id="detail-actions"></div>
    </div>
    <div id="detail-status" class="alert" hidden style="margin-top:var(--space-3);"></div>
  `);

  document.getElementById('detail-back').addEventListener('click', goBack);

  for (const a of document.querySelectorAll('.detail-link[data-nav-view]')) {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      navigate(a.dataset.navView, { id: a.dataset.navId });
    });
  }

  const actionsEl = document.getElementById('detail-actions');
  const statusEl = document.getElementById('detail-status');
  for (const a of actions) {
    if (a.scope && !hasScope(a.scope)) continue;
    const btn = document.createElement('button');
    btn.className = a.className || 'btn btn--secondary';
    btn.textContent = a.label;
    btn.addEventListener('click', () => a.onClick(statusEl));
    actionsEl.appendChild(btn);
  }
  if (!actionsEl.children.length) actionsEl.remove();
}
