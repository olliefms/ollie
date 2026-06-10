// Deterministic in-memory localStorage for fleet UI tests.
//
// Node 22+ exposes an experimental `localStorage` global that throws unless
// `--localstorage-file` is passed, and it shadows the one happy-dom would
// provide. Installing our own simple, synchronous store makes the auth/api
// tests behave identically on local (Node 26) and CI (Node 20) regardless of
// what the runtime exposes.
const store = new Map();

globalThis.localStorage = {
  getItem(key) {
    return store.has(key) ? store.get(key) : null;
  },
  setItem(key, value) {
    store.set(key, String(value));
  },
  removeItem(key) {
    store.delete(key);
  },
  clear() {
    store.clear();
  },
  key(index) {
    return [...store.keys()][index] ?? null;
  },
  get length() {
    return store.size;
  },
};
