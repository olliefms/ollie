import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function doc(mime_type, overrides = {}) {
  return {
    id: 'b1',
    name: 'notes.md',
    mime_type,
    size: 128,
    status: 'ready',
    created_at: '2026-07-01T00:00:00Z',
    updated_at: '2026-07-01T00:00:00Z',
    tags: [],
    ...overrides,
  };
}

// The detail view makes two calls: metadata (Accept: application/json), then
// the raw bytes for the preview.
function stubFetch(mime_type, body, overrides = {}) {
  const fetchMock = vi.fn().mockImplementation((_url, opts) => {
    const wantsJson = opts?.headers?.Accept === 'application/json';
    if (wantsJson) {
      return Promise.resolve({ ok: true, status: 200, json: async () => doc(mime_type, overrides) });
    }
    return Promise.resolve({
      ok: true,
      status: 200,
      blob: async () => ({ text: async () => body }),
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
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

async function render() {
  const { renderDocumentDetailView } = await import('../../static/fleet/pages/document-detail.js');
  await renderDocumentDetailView('b1');
}

describe('document detail text preview', () => {
  // The pipeline extracts every text/* type as text (src/ai/extract.rs), so the
  // viewer must not gate on text/plain alone — text/markdown regressed on that.
  for (const mt of ['text/plain', 'text/markdown', 'text/csv', 'application/json']) {
    it(`renders ${mt} into a pre block`, async () => {
      stubFetch(mt, '# Heading\n\nbody text');
      await render();
      const pre = document.querySelector('#doc-viewer pre');
      expect(pre).toBeTruthy();
      expect(pre.textContent).toBe('# Heading\n\nbody text');
    });
  }

  it('still refuses to preview a genuinely binary type', async () => {
    stubFetch('application/zip', 'PK');
    await render();
    expect(document.querySelector('#doc-viewer pre')).toBeFalsy();
    expect(document.getElementById('doc-viewer').textContent).toContain("can't be previewed");
  });

  // AGENTS.md: "text/html -> drop from preview, force download (the only
  // previewable type that can execute scripts)". Broadening the text gate to
  // text/* must not walk that back — see #184/#240.
  for (const htmlMime of ['text/html', 'text/html; charset=utf-8', 'TEXT/HTML']) {
    it(`refuses to preview ${htmlMime}`, async () => {
      stubFetch(htmlMime, '<img src=x onerror=alert(1)>');
      await render();
      expect(document.querySelector('#doc-viewer pre')).toBeFalsy();
      expect(document.getElementById('doc-viewer').textContent).toContain("can't be previewed");
    });
  }

  it('matches on the bare type when the mime carries parameters', async () => {
    stubFetch('application/json; charset=utf-8', '{"a":1}');
    await render();
    expect(document.querySelector('#doc-viewer pre').textContent).toBe('{"a":1}');
  });

  it('truncates an oversized text file and says so', async () => {
    stubFetch('text/plain', 'x'.repeat(1_000_050));
    await render();
    const pre = document.querySelector('#doc-viewer pre');
    expect(pre.textContent).toHaveLength(1_000_000);
    expect(document.getElementById('doc-preview-truncated')).toBeTruthy();
  });

  it('leaves a file at the cap untruncated', async () => {
    stubFetch('text/plain', 'x'.repeat(1_000_000));
    await render();
    expect(document.getElementById('doc-preview-truncated')).toBeFalsy();
  });
});

// #406 added permanently_failed. The error string is the only account an
// operator gets of why a blob was written off, so the row that carries it must
// not stay keyed to 'failed' alone.
describe('document detail failure reporting', () => {
  for (const status of ['failed', 'permanently_failed']) {
    it(`shows the error for a ${status} document`, async () => {
      stubFetch('text/plain', 'body', { status, error: 'ollama unreachable' });
      await render();
      expect(document.getElementById('main-content').textContent)
        .toContain('ollama unreachable');
    });

  }

  it('shows no error row for a ready document', async () => {
    stubFetch('text/plain', 'body');
    await render();
    expect(document.getElementById('main-content').textContent).not.toContain('Error');
  });
});
