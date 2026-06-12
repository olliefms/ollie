import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';
import { categoryLabel, money, CATEGORY_OPTIONS } from '../utils/maintenance-meta.js';

export async function renderMaintenanceView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const qs = new URLSearchParams();
  if (params.equipment_type) qs.set('equipment_type', params.equipment_type);
  if (params.equipment_id) qs.set('equipment_id', params.equipment_id);
  if (params.category) qs.set('category', params.category);
  const suffix = qs.toString() ? `?${qs.toString()}` : '';

  try {
    const res = await apiFetch(`${API_BASE}/maintenance${suffix}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Maintenance',
      createView: 'maintenance-new',
      createScope: 'maintenance:write',
      createLabel: '+ Add Maintenance',
      detailView: 'maintenance-detail',
      emptyText: 'No maintenance entries found.',
      columns: [
        { header: 'Date',        cell: m => m.service_date || '—' },
        { header: 'Equipment',   cell: m => m.equipment_type || '—' },
        { header: 'Category',    cell: m => categoryLabel(m.category) },
        { header: 'Description', cell: m => m.description || '—' },
        { header: 'Cost',        cell: m => money(m.cost) },
        { header: 'Vendor',      cell: m => m.vendor || '—' },
      ],
      rows: items,
      extraControls: (controlsEl) => {
        // ── Equipment Type select ──────────────────────────────
        const typeSel = document.createElement('select');
        typeSel.className = 'form-select';
        typeSel.setAttribute('aria-label', 'Filter by equipment type');

        for (const { value, label } of [
          { value: '', label: 'All Equipment' },
          { value: 'truck', label: 'Truck' },
          { value: 'trailer', label: 'Trailer' },
        ]) {
          const opt = document.createElement('option');
          opt.value = value;
          opt.textContent = label;
          if ((params.equipment_type || '') === value) opt.selected = true;
          typeSel.appendChild(opt);
        }

        // ── Unit select ────────────────────────────────────────
        const unitSel = document.createElement('select');
        unitSel.className = 'form-select';
        unitSel.setAttribute('aria-label', 'Filter by unit');
        unitSel.disabled = !params.equipment_type;

        const allUnitOpt = document.createElement('option');
        allUnitOpt.value = '';
        allUnitOpt.textContent = 'All Units';
        unitSel.appendChild(allUnitOpt);

        async function populateUnits(type, selectedId) {
          if (!type) {
            unitSel.disabled = true;
            return;
          }
          try {
            const endpoint = type === 'truck' ? `${API_BASE}/trucks` : `${API_BASE}/trailers`;
            const r = await apiFetch(endpoint);
            if (!r.ok) return;
            const d = await r.json();
            const units = d.items || (Array.isArray(d) ? d : []);
            for (const u of units) {
              const o = document.createElement('option');
              o.value = u.id;
              o.textContent = u.unit_number;
              if (selectedId && String(u.id) === String(selectedId)) o.selected = true;
              unitSel.appendChild(o);
            }
            unitSel.disabled = false;
          } catch {
            // leave "All Units" only — don't break the page
          }
        }

        if (params.equipment_type) {
          populateUnits(params.equipment_type, params.equipment_id);
        }

        typeSel.addEventListener('change', () => {
          renderMaintenanceView({
            ...params,
            equipment_type: typeSel.value || undefined,
            equipment_id: undefined,
          });
        });

        unitSel.addEventListener('change', () => {
          renderMaintenanceView({
            ...params,
            equipment_id: unitSel.value || undefined,
          });
        });

        // ── Category select ────────────────────────────────────
        const sel = document.createElement('select');
        sel.className = 'form-select';
        sel.setAttribute('aria-label', 'Filter by category');

        const allOpt = document.createElement('option');
        allOpt.value = '';
        allOpt.textContent = 'All Categories';
        sel.appendChild(allOpt);

        for (const { value, label } of CATEGORY_OPTIONS) {
          const opt = document.createElement('option');
          opt.value = value;
          opt.textContent = label;
          if (params.category === value) opt.selected = true;
          sel.appendChild(opt);
        }

        sel.addEventListener('change', () => {
          renderMaintenanceView({ ...params, category: sel.value || undefined });
        });

        // Insert order: typeSel first, then unitSel, then category (appended last)
        controlsEl.insertBefore(sel, controlsEl.firstChild);
        controlsEl.insertBefore(unitSel, controlsEl.firstChild);
        controlsEl.insertBefore(typeSel, controlsEl.firstChild);
      },
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load maintenance: ${escHtml(err.message)}</div>`);
    }
  }
}
