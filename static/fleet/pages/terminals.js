import { apiFetch, API_BASE } from '../utils/api.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';

const money = v => (v != null ? `$${Number(v).toFixed(2)}` : '—');

export async function renderTerminalsView() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/terminals`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const terminals = await res.json();

    renderEntityList({
      title: 'Terminals',
      createView: 'terminal-new',
      createScope: 'terminals:write',
      createLabel: '+ Create Terminal',
      detailView: 'terminal-detail',
      emptyText: 'No terminals found.',
      columns: [
        { header: 'Name',             cell: t => t.name },
        { header: 'Timezone',         cell: t => t.timezone },
        { header: 'Default',          cell: t => (t.is_default ? 'Yes' : 'No') },
        { header: 'Loaded Rate/Mi',   cell: t => money(t.loaded_rate_per_mile) },
        { header: 'Deadhead Rate/Mi', cell: t => money(t.deadhead_rate_per_mile) },
        { header: 'Free Dwell (min)', cell: t => (t.free_dwell_minutes != null ? t.free_dwell_minutes : '—') },
      ],
      rows: terminals || [],
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load terminals: ${err.message}</div>`);
    }
  }
}
