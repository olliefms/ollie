let cached = null;

export async function getAppVersion() {
  if (cached !== null) return cached;
  try {
    const r = await fetch('/version');
    if (!r.ok) return cached = '—';
    const data = await r.json();
    return cached = (data && typeof data.version === 'string') ? `v${data.version}` : '—';
  } catch (_) {
    return cached = '—';
  }
}
