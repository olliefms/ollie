/**
 * Pure scope matcher mirroring the backend `scope_granted`:
 * a required scope `r:a` is satisfied by the global `*`, an exact match,
 * or the per-resource wildcard `r:*`.
 */
export function scopeGranted(scopes, required) {
  if (!Array.isArray(scopes) || scopes.length === 0) return false;
  const colon = required.indexOf(':');
  const resourceWildcard = colon === -1 ? null : `${required.slice(0, colon)}:*`;
  return scopes.some(s => s === '*' || s === required || s === resourceWildcard);
}

/** Show/hide a control by grant. Fail-safe: a null element is a no-op. */
export function gate(el, granted) {
  if (!el) return;
  el.hidden = !granted;
}
