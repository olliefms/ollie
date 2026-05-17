import { clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { renderTripCard } from '../components/trip-card.js';
import { renderWeekStepper } from '../components/week-stepper.js';
import { attachHorizontalSwipe } from '../components/swipe.js';
import { sundayOf } from '../utils/week.js';

export async function renderPastPane(container, options = {}) {
  let currentWeek = options.weekStart || sundayOf(new Date()).toISOString().slice(0, 10);
  let weekInfo = null;

  container.replaceChildren();
  const stepperSlot = document.createElement('div');
  stepperSlot.className = 'past-pane__stepper-slot';
  const list = document.createElement('div');
  list.className = 'trip-list past-pane__list';
  container.appendChild(stepperSlot);
  container.appendChild(list);

  function urlForWeek(week) {
    const u = new URL(location.href);
    u.searchParams.set('tab', 'past');
    u.searchParams.set('week_start', week);
    return u.pathname + '?' + u.searchParams.toString();
  }

  async function load(weekStart) {
    currentWeek = weekStart;
    history.replaceState({}, '', urlForWeek(weekStart));
    list.replaceChildren();
    const spinner = document.createElement('div');
    spinner.className = 'trips-loading';
    const sp = document.createElement('div');
    sp.className = 'spinner';
    spinner.appendChild(sp);
    list.appendChild(spinner);
    try {
      const data = await apiFetch(`/trips?tab=past&week_start=${weekStart}`);
      list.replaceChildren();
      weekInfo = data.week || null;
      const items = (data && data.items) || [];
      renderStepper();
      if (items.length === 0) {
        const empty = document.createElement('div');
        empty.className = 'trips-empty';
        empty.textContent = 'No trips delivered this week.';
        list.appendChild(empty);
        return;
      }
      items.forEach(t => list.appendChild(renderTripCard(t, 'past')));
    } catch (err) {
      if (err.status === 401) { clearAuth(); window.location.replace('/driver'); return; }
      list.replaceChildren();
      const e = document.createElement('div');
      e.className = 'trips-error';
      e.textContent = err.message || 'Failed to load trips';
      list.appendChild(e);
    }
  }

  function renderStepper() {
    stepperSlot.replaceChildren();
    if (!weekInfo) return;
    const todayWeek = sundayOf(new Date()).toISOString().slice(0, 10);
    const stepper = renderWeekStepper({
      week: weekInfo,
      todayWeekStart: todayWeek,
      onChange: load,
    });
    stepperSlot.appendChild(stepper);
  }

  attachHorizontalSwipe(list, {
    onPrev: () => weekInfo && weekInfo.has_prev && stepPrev(),
    onNext: () => weekInfo && weekInfo.has_next && stepNext(),
    canPrev: () => weekInfo && weekInfo.has_prev,
    canNext: () => weekInfo && weekInfo.has_next,
  });

  function stepPrev() {
    const [y, m, d] = currentWeek.split('-').map(Number);
    const dt = new Date(Date.UTC(y, m - 1, d));
    dt.setUTCDate(dt.getUTCDate() - 7);
    load(dt.toISOString().slice(0, 10));
  }
  function stepNext() {
    const [y, m, d] = currentWeek.split('-').map(Number);
    const dt = new Date(Date.UTC(y, m - 1, d));
    dt.setUTCDate(dt.getUTCDate() + 7);
    load(dt.toISOString().slice(0, 10));
  }

  await load(currentWeek);
}
