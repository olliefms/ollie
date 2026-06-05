import { escHtml } from '../utils/format.js';
import { hasScope } from '../utils/api.js';
import { setContent, goBack } from '../utils/dom.js';

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
