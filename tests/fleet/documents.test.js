import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';
import { matchRoute } from '../../static/fleet/router.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

function blobs(count, startAt = 0) {
  return Array.from({ length: count }, (_, i) => ({
    id: `b${startAt + i}`,
    name: `doc-${startAt + i}.pdf`,
    mime_type: 'application/pdf',
    size: 1024,
    status: 'ready',
    summary: 'Summary',
    created_at: '2026-07-01T00:00:00Z',
  }));
}

beforeEach(() => {
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
  vi.restoreAllMocks();
});

afterEach(() => vi.restoreAllMocks());

async function render(params) {
  const { renderDocumentsView } = await import('../../static/fleet/pages/documents.js');
  await renderDocumentsView(params);
  await Promise.resolve();
}

describe('renderDocumentsView pagination', () => {
  it('shows Load more only while the response total exceeds the loaded window', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 20, total: 55, items: blobs(20) }));
    vi.stubGlobal('fetch', fetchMock);
    await render({});
    expect(document.getElementById('doc-load-more')).toBeTruthy();

    fetchMock.mockResolvedValue(jsonResponse({ returned: 20, total: 20, items: blobs(20) }));
    await render({});
    expect(document.getElementById('doc-load-more')).toBeFalsy();
  });

  it('advances the offset numerically and requests the next page', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 20, total: 80, items: blobs(20) }));
    vi.stubGlobal('fetch', fetchMock);
    const pushSpy = vi.spyOn(history, 'pushState');

    // Arrive on page 2 the way the router hands params over: offset is a string.
    await render({ offset: '20' });
    const firstUrl = fetchMock.mock.calls.at(-1)[0];
    expect(firstUrl).toContain('offset=20');
    expect(firstUrl).toContain('limit=20');

    document.getElementById('doc-load-more').click();
    const pushed = pushSpy.mock.calls.at(-1)[2];
    expect(pushed).toBe('/fleet/documents?offset=40');
    expect(pushed).not.toContain('2020');

    await render(matchRoute(pushed).params);
    const nextUrl = fetchMock.mock.calls.at(-1)[0];
    expect(nextUrl).toContain('offset=40');
  });

  it('keeps the name filter when paging and when searching', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 20, total: 80, items: blobs(20) }));
    vi.stubGlobal('fetch', fetchMock);
    const pushSpy = vi.spyOn(history, 'pushState');

    await render({ name: 'rate', offset: '20' });
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('name=rate');

    document.getElementById('doc-load-more').click();
    expect(pushSpy.mock.calls.at(-1)[2]).toBe('/fleet/documents?name=rate&offset=40');

    document.getElementById('doc-filter-name').value = 'bol';
    document.getElementById('doc-filter-apply').click();
    expect(pushSpy.mock.calls.at(-1)[2]).toBe('/fleet/documents?name=bol');
  });

  it('renders an empty state on a deep page with no rows', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ returned: 0, total: 20, items: [] }));
    vi.stubGlobal('fetch', fetchMock);
    await render({ offset: '200' });

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('state-empty');
    expect(document.getElementById('doc-load-more')).toBeFalsy();
  });
});
