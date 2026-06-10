import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'happy-dom',
    include: ['tests/fleet/**/*.test.js'],
    setupFiles: ['tests/fleet/setup.js'],
  },
});
