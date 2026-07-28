import { describe, it, expect, vi, beforeEach } from 'vitest';
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

describe('renderEventsView header', () => {
  beforeEach(() => {
    document.body.innerHTML =
      '<div id="topbar-controls"></div><div id="main-content"></div>';
    localStorage.clear();
  });

  it('puts the attention toggle in the topbar, list in content, no page-title', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true, status: 200, json: async () => ({ events: [] }),
    });
    vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #events-attention')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(document.getElementById('events-list')).toBeTruthy();
    const { clearEventsRefresh } = await import('../../static/fleet/pages/events.js');
    clearEventsRefresh();
    vi.restoreAllMocks();
  });
});

describe('renderEventsView pagination', () => {
  beforeEach(() => {
    document.body.innerHTML =
      '<div id="topbar-controls"></div><div id="main-content"></div><div id="refresh-indicator"></div>';
    localStorage.clear();
  });

  function eventPage(count, startAt = 0) {
    return Array.from({ length: count }, (_, i) => ({
      ...base,
      id: `ev${startAt + i}`,
      subject: `Event ${startAt + i}`,
      occurred_at: new Date(Date.now() - (startAt + i) * 60_000).toISOString(),
    }));
  }

  it('requests a bounded window and grows it contiguously on Load more', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => ({ items: eventPage(20) }) })
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => ({ items: eventPage(25) }) });
    vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView, clearEventsRefresh } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    expect(fetchMock.mock.calls.at(-1)[0]).toContain('limit=20');
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('offset=0');

    const moreBtn = document.getElementById('events-load-more');
    expect(moreBtn).toBeTruthy();

    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    // The whole window is refetched from offset 0 with a larger limit, so
    // events arriving between fetches can never open a gap.
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('limit=40');
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('offset=0');
    const html = document.getElementById('events-list').innerHTML;
    expect((html.match(/data-event-id=/g) || []).length).toBe(25);
    expect(document.getElementById('events-load-more')).toBeFalsy();

    clearEventsRefresh();
    vi.restoreAllMocks();
  });

  it('keeps the loaded feed and re-enables Load more when a grow fails', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => ({ items: eventPage(20) }) })
      .mockResolvedValueOnce({ ok: false, status: 500, json: async () => ({}) });
    vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView, clearEventsRefresh } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    document.getElementById('events-load-more').click();
    await new Promise(r => setTimeout(r, 0));

    const html = document.getElementById('events-list').innerHTML;
    expect(html).not.toContain('state-error');
    expect((html.match(/data-event-id=/g) || []).length).toBe(20);
    const btn = document.getElementById('events-load-more');
    expect(btn).toBeTruthy();
    expect(btn.disabled).toBe(false);

    clearEventsRefresh();
    vi.restoreAllMocks();
  });

  it('stops at the server window cap with an explanatory note', async () => {
    const fetchMock = vi.fn().mockImplementation(async (url) => {
      const limit = Number(new URL(url, 'http://x').searchParams.get('limit'));
      return { ok: true, status: 200, json: async () => ({ items: eventPage(limit) }) };
    });
    vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView, clearEventsRefresh } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    for (let i = 0; i < 4; i++) {
      const btn = document.getElementById('events-load-more');
      expect(btn).toBeTruthy();
      btn.click();
      await new Promise(r => setTimeout(r, 0));
    }

    expect(fetchMock.mock.calls.at(-1)[0]).toContain('limit=100');
    expect(document.getElementById('events-load-more')).toBeFalsy();
    expect(document.getElementById('events-more').textContent)
      .toContain('Showing the 100 most recent events');

    clearEventsRefresh();
    vi.restoreAllMocks();
  });

  it('hides Load more when the first page is short', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true, status: 200, json: async () => ({ items: eventPage(3) }),
    });
    vi.stubGlobal('fetch', fetchMock);
    const { saveToken } = await import('../../static/fleet/utils/auth.js');
    saveToken('test-token');

    const { renderEventsView, clearEventsRefresh } = await import('../../static/fleet/pages/events.js');
    await renderEventsView();
    await Promise.resolve();

    expect(document.getElementById('events-load-more')).toBeFalsy();
    clearEventsRefresh();
    vi.restoreAllMocks();
  });
});
