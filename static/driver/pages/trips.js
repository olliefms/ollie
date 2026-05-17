import { isAuthenticated } from '../utils/auth.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { renderPastPane } from './trips-past.js';
import { renderCurrentPane } from './trips-current.js';
import { renderUpcomingPane } from './trips-upcoming.js';

const VALID_TABS = new Set(['past', 'current', 'upcoming']);

export async function renderTrips(container) {
  if (!isAuthenticated()) { window.location.replace('/driver'); return; }
  container.replaceChildren();
  const params = new URLSearchParams(location.search);
  const initial = VALID_TABS.has(params.get('tab')) ? params.get('tab') : 'current';
  let activeTab = initial;

  const page = document.createElement('div');
  page.className = 'page-with-nav';
  page.appendChild(renderAppBar({ title: 'My Trips' }));

  const tabBar = document.createElement('div');
  tabBar.className = 'tab-bar';
  const seg = document.createElement('div');
  seg.className = 'tab-seg';
  const tabEls = {};
  for (const id of ['past', 'current', 'upcoming']) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'tab' + (id === activeTab ? ' tab--active' : '');
    btn.textContent = id.charAt(0).toUpperCase() + id.slice(1);
    btn.addEventListener('click', () => switchTab(id));
    tabEls[id] = btn;
    seg.appendChild(btn);
  }
  tabBar.appendChild(seg);
  page.appendChild(tabBar);

  const pane = document.createElement('div');
  pane.className = 'trips-pane';
  page.appendChild(pane);

  page.appendChild(renderBottomNav('trips'));
  container.appendChild(page);

  async function switchTab(id) {
    if (id === activeTab) return;
    activeTab = id;
    Object.values(tabEls).forEach(el => el.classList.remove('tab--active'));
    tabEls[id].classList.add('tab--active');
    const u = new URL(location.href);
    u.searchParams.set('tab', id);
    if (id !== 'past') u.searchParams.delete('week_start');
    history.replaceState({}, '', u.pathname + '?' + u.searchParams.toString());
    await mount();
  }

  async function mount() {
    if (activeTab === 'past') {
      const weekStart = new URLSearchParams(location.search).get('week_start') || null;
      await renderPastPane(pane, { weekStart });
    } else if (activeTab === 'current') {
      await renderCurrentPane(pane);
    } else {
      await renderUpcomingPane(pane);
    }
  }

  await mount();
}
