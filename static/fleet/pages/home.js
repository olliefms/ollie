import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, navigate } from '../utils/dom.js';

export async function renderHomeView() {
  const kpis = [
    { label: 'Open Loads',        endpoint: `${API_BASE}/loads/count`,   view: 'loads'     },
    { label: 'Active Drivers',    endpoint: `${API_BASE}/drivers/count`, view: 'drivers'   },
    { label: 'Pending Documents', endpoint: `${API_BASE}/blobs/count`,   view: 'documents' },
    { label: 'Events Today',      endpoint: `${API_BASE}/events/count`,  view: 'events'    },
  ];

  setContent(`
    <div class="home-view">
      <div class="kpi-row" id="kpi-row">
        ${kpis.map((_, i) => `
          <div class="kpi-tile" id="kpi-tile-${i}">
            <div class="kpi-tile__count">—</div>
            <div class="kpi-tile__label">${escHtml(_.label)}</div>
          </div>
        `).join('')}
      </div>
    </div>
  `);

  kpis.forEach(async (kpi, i) => {
    try {
      const res = await apiFetch(kpi.endpoint);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const tile = document.getElementById(`kpi-tile-${i}`);
      if (tile) {
        tile.querySelector('.kpi-tile__count').textContent = data.count ?? '—';
        tile.style.cursor = 'pointer';
        tile.addEventListener('click', () => navigate(kpi.view));
      }
    } catch (err) {
      console.error(`KPI fetch failed for ${kpi.label}:`, err);
    }
  });
}
