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
