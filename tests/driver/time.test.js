import { test } from 'node:test';
import assert from 'node:assert/strict';
import { nowInZone, convertNaive } from '../../static/driver/utils/time.js';

test('nowInZone returns YYYY-MM-DDTHH:MM:SS', () => {
  const s = nowInZone('America/Los_Angeles');
  assert.match(s, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/);
});

test('nowInZone falls back when tz is null', () => {
  const s = nowInZone(null);
  assert.match(s, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/);
});

test('convertNaive same tz is identity', () => {
  const v = convertNaive('2026-05-09T15:42:00', 'America/Los_Angeles', 'America/Los_Angeles');
  assert.equal(v, '2026-05-09T15:42:00');
});

test('convertNaive PST -> EST shifts by 3 hours', () => {
  const v = convertNaive('2026-05-09T10:00:00', 'America/Los_Angeles', 'America/New_York');
  assert.equal(v, '2026-05-09T13:00:00');
});
