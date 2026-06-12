import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { navigate } from '../utils/dom.js';
import { categoryLabel, money } from '../utils/maintenance-meta.js';

// Appends a "Maintenance History" section to #main-content for the given
// equipment. Call AFTER renderDetailPage() has populated the page.
export async function appendMaintenanceHistory(equipmentType, equipmentId) {
  const host = document.getElementById('main-content');
  if (!host) return;

  const section = document.createElement('div');
  section.className = 'detail-card';
  section.style.marginTop = 'var(--space-4)';
  section.innerHTML = `
    <div class="detail-card__title">Maintenance History</div>
    <div id="mnt-history-body"><div class="spinner"></div></div>`;
  host.appendChild(section);

  const canWrite = hasScope('maintenance:write');

  try {
    const res = await apiFetch(
      `${API_BASE}/maintenance?equipment_type=${encodeURIComponent(equipmentType)}&equipment_id=${encodeURIComponent(equipmentId)}`
    );
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || [];

    const rows = items.map(m => `
      <tr data-row-id="${escHtml(m.id)}" style="cursor:pointer;">
        <td>${escHtml(m.service_date || '—')}</td>
        <td>${escHtml(categoryLabel(m.category))}</td>
        <td>${escHtml(m.description || '—')}</td>
        <td>${escHtml(money(m.cost))}</td>
        <td>${escHtml(m.vendor || '—')}</td>
      </tr>`).join('');

    const table = items.length
      ? `<div class="table-wrapper"><table class="data-table">
           <thead><tr><th>Date</th><th>Category</th><th>Description</th><th>Cost</th><th>Vendor</th></tr></thead>
           <tbody>${rows}</tbody>
         </table></div>`
      : `<div class="state-empty">No maintenance entries.</div>`;

    const addBtn = canWrite
      ? `<button class="btn btn--secondary" id="mnt-add-btn">+ Add maintenance</button>`
      : '';

    document.getElementById('mnt-history-body').innerHTML =
      `${table}<div style="margin-top:var(--space-3);">${addBtn}</div>`;

    section.querySelectorAll('tr[data-row-id]').forEach(tr => {
      tr.addEventListener('click', () => navigate('maintenance-detail', { id: tr.dataset.rowId }));
    });
    const btn = document.getElementById('mnt-add-btn');
    if (btn) {
      btn.addEventListener('click', () => {
        navigate('maintenance-new', { equipment_type: equipmentType, equipment_id: equipmentId });
      });
    }
  } catch (err) {
    const body = document.getElementById('mnt-history-body');
    if (body) body.innerHTML = `<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`;
  }
}
