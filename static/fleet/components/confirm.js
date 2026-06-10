/** Standard destructive-action confirm. Soft delete is reversible, so the
 *  copy says so. Returns the user's choice as a boolean. */
export function confirmDelete(what) {
  return confirm(`Delete ${what}? This can be undone by reactivating.`);
}
