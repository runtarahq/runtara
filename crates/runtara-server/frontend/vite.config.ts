import path from 'path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  // Relative base so the emitted bundle works under any mount path
  // (`/ui/`, `/ui/<tenant-id>/`, etc.) without rebuilding. The runtime
  // base href is injected into index.html by the Rust server.
  base: './',
  plugins: [react()],

  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },

  optimizeDeps: {
    include: ['react', 'react-dom', 'react-router'],
    exclude: ['@vite/client', '@vite/env'],
  },

  build: {
    sourcemap: process.env.NODE_ENV !== 'production',
    target: 'es2020',
    rollupOptions: {
      output: {
        manualChunks(id) {
          // Vendor chunks — order matters: first match wins
          if (/node_modules\/(react|react-dom|react-router)\//.test(id))
            return 'react-vendor';
          if (/node_modules\/@radix-ui\//.test(id)) return 'ui-vendor';
          if (/node_modules\/(react-hook-form|@hookform\/|zod)\//.test(id))
            return 'form-vendor';
          // Workflow chunk: xyflow and its transitive deps
          // to avoid circular chunks between workflow and state-vendor
          if (/node_modules\/@xyflow\//.test(id)) return 'workflow';
          if (
            /node_modules\/(@tanstack\/react-query|@tanstack\/react-table|zustand|immer)\//.test(
              id
            )
          )
            return 'state-vendor';
          if (/node_modules\/(oidc-client-ts|react-oidc-context)\//.test(id))
            return 'auth-vendor';
          if (
            /node_modules\/(axios|date-fns|clsx|tailwind-merge|class-variance-authority)\//.test(
              id
            )
          )
            return 'utils-vendor';
          if (/node_modules\/(lucide-react|@radix-ui\/react-icons)\//.test(id))
            return 'icons';
          if (
            /node_modules\/(sonner|vaul|react-resizable-panels|next-themes)\//.test(
              id
            )
          )
            return 'ui-ext_ras';
        },
      },
      treeshake: {
        preset: 'recommended',
      },
    },
    chunkSizeWarningLimit: 500,
    emptyOutDir: true,
    // Optimize for production using esbuild (default)
    minify: 'esbuild',
    reportCompressedSize: false,
  },

  server: {
    port: 8081,
    host: '0.0.0.0',
    allowedHosts: true,
  },
});
