function fmtNaiveSV(date, tz) {
  const opts = tz ? { timeZone: tz, hour12: false } : { hour12: false };
  return new Intl.DateTimeFormat('sv-SE', {
    ...opts,
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  }).format(date).replace(' ', 'T');
}

export function nowInZone(tz) {
  return fmtNaiveSV(new Date(), tz);
}

export function convertNaive(value, fromTz, toTz) {
  if (!value) return value;
  if (fromTz === toTz) return value;
  const [datePart, timePart] = value.split('T');
  const [y, mo, d] = datePart.split('-').map(Number);
  const [hh, mm, ss = 0] = timePart.split(':').map(Number);
  let guess = new Date(Date.UTC(y, mo - 1, d, hh, mm, ss));
  for (let i = 0; i < 2; i++) {
    const rendered = fmtNaiveSV(guess, fromTz);
    const [rd, rt] = rendered.split('T');
    const [ry, rmo, rdy] = rd.split('-').map(Number);
    const [rhh, rmm, rss] = rt.split(':').map(Number);
    const renderedUtcMs = Date.UTC(ry, rmo - 1, rdy, rhh, rmm, rss);
    const targetUtcMs = Date.UTC(y, mo - 1, d, hh, mm, ss);
    const diffMs = targetUtcMs - renderedUtcMs;
    if (diffMs === 0) break;
    guess = new Date(guess.getTime() + diffMs);
  }
  return fmtNaiveSV(guess, toTz);
}
