const MONTHS = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];

export function sundayOf(date) {
  const d = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate()));
  d.setUTCDate(d.getUTCDate() - d.getUTCDay());
  return d;
}

export function formatWeekRange(isoSundayDate) {
  const [y, m, d] = isoSundayDate.split('-').map(Number);
  const start = new Date(Date.UTC(y, m - 1, d));
  const end = new Date(start);
  end.setUTCDate(end.getUTCDate() + 6);
  const sM = MONTHS[start.getUTCMonth()];
  const eM = MONTHS[end.getUTCMonth()];
  if (sM === eM) {
    return `${sM} ${start.getUTCDate()} – ${end.getUTCDate()}, ${end.getUTCFullYear()}`;
  }
  return `${sM} ${start.getUTCDate()} – ${eM} ${end.getUTCDate()}, ${end.getUTCFullYear()}`;
}

export function formatDeliveredAt(naive, tz) {
  if (!naive) return '';
  const [datePart, timePart] = naive.split('T');
  if (!datePart || !timePart) return naive;
  const [y, mo, d] = datePart.split('-').map(Number);
  const [hh, mm] = timePart.split(':').map(Number);
  const date = new Date(Date.UTC(y, mo - 1, d, hh, mm));
  const dow = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'][date.getUTCDay()];
  const mname = MONTHS[date.getUTCMonth()];
  const hour12 = ((hh + 11) % 12) + 1;
  const ampm = hh >= 12 ? 'PM' : 'AM';
  return `Delivered ${dow}, ${mname} ${d} · ${hour12}:${String(mm).padStart(2,'0')} ${ampm}`;
}
