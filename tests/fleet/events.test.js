import { describe, it, expect } from 'vitest';
import { eventContext, jumpHref, eventRowHtml, eventsListHtml, attachEventHandlers, applyAttentionFilter } from '../../static/fleet/pages/events.js';

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
  it('blob row jump link reads "Go to document"', () => {
    expect(eventRowHtml({ ...base, entity_type: 'blob', entity_id: 'b1' })).toContain('Go to document');
  });
});

describe('eventsListHtml', () => {
  it('renders one row per event', () => {
    const html = eventsListHtml([base, { ...base, id: 'e2' }]);
    expect((html.match(/data-event-id=/g) || []).length).toBe(2);
  });
});

describe('attachEventHandlers (expand)', () => {
  it('toggles the detail panel on row click', () => {
    const root = document.createElement('div');
    root.innerHTML = eventsListHtml([base]);
    document.body.appendChild(root);
    attachEventHandlers(root);

    const detail = root.querySelector('.event-item__detail');
    expect(detail.hidden).toBe(true);
    root.querySelector('.event-item__line').click();
    expect(detail.hidden).toBe(false);
    root.querySelector('.event-item__line').click();
    expect(detail.hidden).toBe(true);
    root.remove();
  });

  it('does not toggle when clicking the jump link', () => {
    const root = document.createElement('div');
    root.innerHTML = eventsListHtml([base]);
    document.body.appendChild(root);
    attachEventHandlers(root);
    const detail = root.querySelector('.event-item__detail');
    const link = root.querySelector('.event-item__jump');
    if (link) link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(detail.hidden).toBe(true);
    root.remove();
  });
});

describe('applyAttentionFilter', () => {
  it('keeps only exception rows when on', () => {
    const evs = [base, { ...base, id: 'x', severity: 'exception' }];
    expect(applyAttentionFilter(evs, true).map(e => e.id)).toEqual(['x']);
  });
  it('returns all rows when off', () => {
    const evs = [base, { ...base, id: 'x', severity: 'exception' }];
    expect(applyAttentionFilter(evs, false).length).toBe(2);
  });
});
