import { formatWeekRange, sundayOf } from '../utils/week.js';
import { chevronLeftIcon, chevronRightIcon } from './icons.js';

function labelFor(weekStart, todayWeekStart) {
  if (weekStart === todayWeekStart) return 'This week';
  const [y, m, d] = todayWeekStart.split('-').map(Number);
  const today = new Date(Date.UTC(y, m - 1, d));
  today.setUTCDate(today.getUTCDate() - 7);
  const prevWeek = today.toISOString().slice(0, 10);
  if (weekStart === prevWeek) return 'Last week';
  return formatWeekRange(weekStart);
}

export function renderWeekStepper({ week, todayWeekStart, onChange }) {
  const bar = document.createElement('div');
  bar.className = 'week-stepper';

  const prev = document.createElement('button');
  prev.type = 'button';
  prev.className = 'week-stepper__chev';
  prev.setAttribute('aria-label', 'Previous week');
  prev.appendChild(chevronLeftIcon());
  prev.disabled = !week.has_prev;
  prev.addEventListener('click', () => {
    if (!week.has_prev) return;
    const [y, m, d] = week.start.split('-').map(Number);
    const dt = new Date(Date.UTC(y, m - 1, d));
    dt.setUTCDate(dt.getUTCDate() - 7);
    onChange(dt.toISOString().slice(0, 10));
  });

  const labelBtn = document.createElement('button');
  labelBtn.type = 'button';
  labelBtn.className = 'week-stepper__label';
  labelBtn.textContent = labelFor(week.start, todayWeekStart);

  const date = document.createElement('input');
  date.type = 'date';
  date.className = 'week-stepper__date';
  date.value = week.start;
  if (week.earliest_week_start) date.min = week.earliest_week_start;
  if (week.latest_week_start) date.max = week.latest_week_start;
  date.addEventListener('change', () => {
    if (!date.value) return;
    const [y, m, d] = date.value.split('-').map(Number);
    const picked = new Date(Date.UTC(y, m - 1, d));
    const sun = sundayOf(picked).toISOString().slice(0, 10);
    onChange(sun);
  });
  labelBtn.addEventListener('click', () => {
    if (typeof date.showPicker === 'function') date.showPicker();
    else date.focus();
  });

  const next = document.createElement('button');
  next.type = 'button';
  next.className = 'week-stepper__chev';
  next.setAttribute('aria-label', 'Next week');
  next.appendChild(chevronRightIcon());
  next.disabled = !week.has_next;
  next.addEventListener('click', () => {
    if (!week.has_next) return;
    const [y, m, d] = week.start.split('-').map(Number);
    const dt = new Date(Date.UTC(y, m - 1, d));
    dt.setUTCDate(dt.getUTCDate() + 7);
    onChange(dt.toISOString().slice(0, 10));
  });

  bar.appendChild(prev);
  bar.appendChild(labelBtn);
  bar.appendChild(date);
  bar.appendChild(next);
  return bar;
}
