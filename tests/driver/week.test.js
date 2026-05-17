import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sundayOf, formatWeekRange, formatDeliveredAt } from '../../static/driver/utils/week.js';

test('sundayOf returns same date for a Sunday', () => {
  const sun = new Date(Date.UTC(2026, 4, 10));
  const r = sundayOf(sun);
  assert.equal(r.toISOString().slice(0, 10), '2026-05-10');
});

test('sundayOf returns previous Sunday for a Wednesday', () => {
  const wed = new Date(Date.UTC(2026, 4, 13));
  const r = sundayOf(wed);
  assert.equal(r.toISOString().slice(0, 10), '2026-05-10');
});

test('formatWeekRange spans Sun–Sat with one year', () => {
  const s = formatWeekRange('2026-05-10');
  assert.equal(s, 'May 10 – 16, 2026');
});

test('formatWeekRange crosses month boundary', () => {
  const s = formatWeekRange('2026-05-31');
  assert.equal(s, 'May 31 – Jun 6, 2026');
});

test('formatDeliveredAt returns non-empty string', () => {
  const s = formatDeliveredAt('2026-05-09T15:42:00', 'America/Los_Angeles');
  assert.ok(s.length > 0);
});
