import { describe, it, expect } from 'vitest';
import { eventContext, jumpHref, eventRowHtml, eventsListHtml } from '../../static/fleet/pages/events.js';

const base = {
  id: 'e1', entity_type: 'trip', entity_id: 't1', event_type: 'stop.arrived',
  occurred_at: new Date().toISOString(), severity: 'normal',
  subject: 'Trip 1042 · Acme → Dallas', payload: { seq: 2, stop_name: "Love's #212" },
};

describe('eventContext', () => {
  it('uses stop_name for stop events', () => {
    expect(eventContext(base.payload, 'stop.arrived')).toBe("Love's #212");
  });
  it('uses location for check_call', () => {
    expect(eventContext({ location: 'I-40 nr Amarillo' }, 'check_call')).toBe('I-40 nr Amarillo');
  });
  it('empty when nothing useful', () => {
    expect(eventContext({}, 'trip.dispatched')).toBe('');
  });
});

describe('jumpHref', () => {
  it('maps entity types to detail routes', () => {
    expect(jumpHref('trip', 't1')).toBe('/fleet/trips/t1');
    expect(jumpHref('driver', 'd1')).toBe('/fleet/drivers/d1');
    expect(jumpHref('truck', 'k1')).toBe('/fleet/trucks/k1');
    expect(jumpHref('trailer', 'r1')).toBe('/fleet/trailers/r1');
    expect(jumpHref('blob', 'b1')).toBe('/fleet/documents/b1');
  });
  it('null for unknown', () => {
    expect(jumpHref('mystery', 'x')).toBe(null);
  });
});

describe('eventRowHtml', () => {
  it('renders subject, humanized verb, and context', () => {
    const html = eventRowHtml(base);
    expect(html).toContain('Trip 1042 · Acme → Dallas');
    expect(html).toContain('Stop Arrived');
    expect(html).toContain("Love's #212");
  });
  it('adds exception class for exception severity', () => {
    expect(eventRowHtml({ ...base, severity: 'exception', event_type: 'stop.late' }))
      .toContain('event-item--exception');
  });
  it('adds system class for system severity', () => {
    expect(eventRowHtml({ ...base, severity: 'system', entity_type: 'blob', event_type: 'processing_failed' }))
      .toContain('event-item--system');
  });
  it('falls back to short id when subject missing', () => {
    expect(eventRowHtml({ ...base, subject: null })).toContain('t1'.slice(0, 8));
  });
  it('escapes HTML in subject and actor', () => {
    const html = eventRowHtml({ ...base, subject: '<b>bold</b>', actor: '"quoted"' });
    expect(html).not.toContain('<b>bold</b>');
    expect(html).toContain('&lt;b&gt;');
    expect(html).toContain('&quot;quoted&quot;');
  });
});

describe('eventsListHtml', () => {
  it('renders one row per event', () => {
    const html = eventsListHtml([base, { ...base, id: 'e2' }]);
    expect((html.match(/data-event-id=/g) || []).length).toBe(2);
  });
});
