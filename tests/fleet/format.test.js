import { describe, it, expect } from 'vitest';
import { escHtml, shortId, fmtUSD, fmtMiles, fmtBytes, badge, humanizeEventType } from '../../static/fleet/utils/format.js';

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
