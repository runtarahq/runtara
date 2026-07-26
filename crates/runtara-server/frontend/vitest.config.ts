import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  define: {
    'import.meta.env.VITE_RUNTARA_API_BASE_URL': JSON.stringify(
      'http://localhost:8080'
    ),
    'import.meta.env.VITE_RUNTARA_API_OBJECT_MODEL_BASE_URL': JSON.stringify(
      'http://localhost:8097'
    ),
    // runtimeConfig defaults authMode to 'oidc', so oidcConfig.ts throws at
    // module load unless all three of these are set. They come from an
    // untracked .env on a dev machine, so the suite passed locally and failed
    // in CI (NodeFormItem / StepPickerModal, which import it transitively).
    // Pin stubs here for the same reason the API URLs above are pinned: tests
    // must not depend on a developer's local environment. Nothing dials these
    // — no test invokes signinRedirect.
    'import.meta.env.VITE_OIDC_AUTHORITY': JSON.stringify(
      'https://oidc.test.invalid'
    ),
    'import.meta.env.VITE_OIDC_CLIENT_ID': JSON.stringify('runtara-test'),
    'import.meta.env.VITE_OIDC_AUDIENCE': JSON.stringify(
      'https://api.test.invalid'
    ),
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    // Exclude e2e tests - those are run with Playwright, not Vitest
    exclude: [
      '**/node_modules/**',
      '**/dist/**',
      '**/e2e/**',
      '**/*.e2e.ts',
      '**/*.e2e.tsx',
    ],
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
});
