import path from 'path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
  // Relative base so the emitted bundle works under any mount path
  // (`/ui/`, `/ui/<tenant-id>/`, etc.) without rebuilding. The runtime
  // base href is injected into index.html by the Rust server.
  base: './',
  plugins: [
    react(),
    VitePWA({
      registerType: 'prompt',
      includeAssets: ['logo-icon.png', 'log.png'],
      manifest: {
        name: 'Runtara',
        short_name: 'Runtara',
        description: 'Durable workflow runtime',
        theme_color: '#3b82f6',
        background_color: '#ffffff',
        display: 'standalone',
        orientation: 'portrait-primary',
        scope: './',
        start_url: './',
        icons: [
          {
            src: 'icons/icon-192x192.png',
            sizes: '192x192',
            type: 'image/png',
          },
          {
            src: 'icons/icon-512x512.png',
            sizes: '512x512',
            type: 'image/png',
          },
          {
            src: 'icons/icon-512x512-maskable.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'maskable',
          },
        ],
      },
      workbox: {
        globPatterns: ['**/*.{js,css,html,ico,png,svg,woff,woff2}'],
        // Ensure new service worker takes control immediately
        skipWaiting: true,
        clientsClaim: true,
        // Clean old caches on update
        cleanupOutdatedCaches: true,
        // Don't cache navigation requests - let them go to network
        // This prevents issues with OAuth redirects and other navigation redirects
        navigateFallback: null,
        runtimeCaching: [
          {
            urlPattern: /^https:\/\/.+\/api\/.*/i,
            handler: 'NetworkFirst',
            options: {
              cacheName: 'api-cache',
              networkTimeoutSeconds: 10,
              expiration: {
                maxEntries: 50,
                maxAgeSeconds: 60 * 5, // 5 minutes - short cache for API
              },
              cacheableResponse: {
                statuses: [200],
              },
            },
          },
          {
            urlPattern: /^https:\/\/fonts\.googleapis\.com\/.*/i,
            handler: 'CacheFirst',
            options: {
              cacheName: 'google-fonts-cache',
              expiration: {
                maxEntries: 10,
                maxAgeSeconds: 60 * 60 * 24 * 365,
              },
            },
          },
        ],
        navigateFallbackDenylist: [/^\/api/, /^\/oauth/],
      },
      devOptions: {
        enabled: true,
        type: 'module',
      },
    }),
  ],

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
          // Workflow chunk: xyflow + dagre + their transitive deps (zustand/immer)
          // to avoid circular chunks between workflow and state-vendor
          if (/node_modules\/(@xyflow\/|@dagrejs\/dagre|dagre)\//.test(id))
            return 'workflow';
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
