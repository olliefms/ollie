import { describe, it, expect } from 'vitest';
import { escHtml, shortId, fmtUSD, fmtMiles, fmtBytes, badge, humanizeEventType, fmtRelative } from '../../static/fleet/utils/format.js';

describe('escHtml', () => {
  it('escapes HTML metacharacters', () => {
    expect(escHtml('<a href="x">&')).toBe('&lt;a href=&quot;x&quot;&gt;&amp;');
  });
  it('returns empty string for falsy', () => {
    expect(escHtml('')).toBe('');
    expect(escHtml(null)).toBe('');
  });
});

describe('shortId', () => {
  it('takes first 8 chars', () => {
    expect(shortId('abcdef1234567890')).toBe('abcdef12');
  });
  it('em-dash for empty', () => {
    expect(shortId('')).toBe('—');
  });
});

describe('fmtUSD', () => {
  it('formats positive with 2 decimals', () => {
    expect(fmtUSD(1234.5)).toBe('$1,234.50');
  });
  it('formats negative with leading minus', () => {
    expect(fmtUSD(-5)).toBe('-$5.00');
  });
  it('em-dash for null/undefined', () => {
    expect(fmtUSD(null)).toBe('—');
    expect(fmtUSD(undefined)).toBe('—');
  });
  it('keeps zero as $0.00 (not em-dash)', () => {
    expect(fmtUSD(0)).toBe('$0.00');
  });
});

describe('fmtMiles', () => {
  it('one decimal + unit', () => {
    expect(fmtMiles(12)).toBe('12.0 mi');
  });
  it('em-dash for null', () => {
    expect(fmtMiles(null)).toBe('—');
  });
});

describe('fmtBytes', () => {
  it('B / KB / MB thresholds', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(2048)).toBe('2.0 KB');
    expect(fmtBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('badge', () => {
  it('slugifies status into a badge span', () => {
    expect(badge('In Transit')).toBe('<span class="badge badge--in_transit">In Transit</span>');
  });
  it('empty string for falsy', () => {
    expect(badge(null)).toBe('');
  });
});

describe('humanizeEventType', () => {
  it('maps known types', () => {
    expect(humanizeEventType('trip.assigned')).toBe('Trip Assigned');
  });
  it('title-cases unknown types', () => {
    expect(humanizeEventType('some_custom.event')).toBe('Some Custom Event');
  });
});

describe('humanizeEventType additions', () => {
  it('maps equipment + trailer change events', () => {
    expect(humanizeEventType('driver.equipment_changed')).toBe('Driver Equipment Changed');
    expect(humanizeEventType('driver.trailer_changed')).toBe('Driver Trailer Changed');
  });
});

describe('fmtRelative', () => {
  const now = 1_000_000_000_000;
  it('seconds / minutes / hours / days', () => {
    expect(fmtRelative(new Date(now - 5_000).toISOString(), now)).toBe('5s');
    expect(fmtRelative(new Date(now - 120_000).toISOString(), now)).toBe('2m');
    expect(fmtRelative(new Date(now - 3 * 3600_000).toISOString(), now)).toBe('3h');
    expect(fmtRelative(new Date(now - 2 * 86400_000).toISOString(), now)).toBe('2d');
  });
  it('holds tier at the boundary (no early rollover)', () => {
    expect(fmtRelative(new Date(now - 59_000).toISOString(), now)).toBe('59s');
    expect(fmtRelative(new Date(now - 60_000).toISOString(), now)).toBe('1m');
    expect(fmtRelative(new Date(now - 59 * 60_000).toISOString(), now)).toBe('59m');
    expect(fmtRelative(new Date(now - 23 * 3600_000).toISOString(), now)).toBe('23h');
  });
  it('em-dash for falsy/invalid', () => {
    expect(fmtRelative('', now)).toBe('—');
    expect(fmtRelative(null, now)).toBe('—');
    expect(fmtRelative('not-a-date', now)).toBe('—');
  });
});
