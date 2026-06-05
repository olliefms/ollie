import { beforeEach } from 'vitest';

// Mock localStorage if not available (for happy-dom environment)
if (typeof localStorage === 'undefined') {
  const store = {};
  global.localStorage = {
    getItem(key) {
      return store[key] || null;
    },
    setItem(key, value) {
      store[key] = String(value);
    },
    removeItem(key) {
      delete store[key];
    },
    clear() {
      for (const key in store) {
        delete store[key];
      }
    },
    key(index) {
      const keys = Object.keys(store);
      return keys[index] || null;
    },
    get length() {
      return Object.keys(store).length;
    },
  };
}
