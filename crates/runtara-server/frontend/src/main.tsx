import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { AuthProvider } from 'react-oidc-context';
import { ReactFlowProvider } from '@xyflow/react';
import { Toaster } from '@/shared/components/ui/sonner';
import { oidcConfig } from '@/shared/config/oidcConfig';
import { initAnalytics } from '@/shared/analytics/plausible';
import App from '@/App';

import './index.css';

initAnalytics();

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000 * 60 * 5, // 5 minutes - data stays fresh for 5 minutes
      gcTime: 1000 * 60 * 10, // 10 minutes - cache garbage collection time
      retry: 1, // Only retry once on failure instead of 3 times
      refetchOnWindowFocus: false, // Already set in useCustomQuery but good as default
    },
  },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AuthProvider {...oidcConfig}>
      <QueryClientProvider client={queryClient}>
        <ReactFlowProvider>
          <App />
        </ReactFlowProvider>
        <Toaster richColors toastOptions={{}} />
      </QueryClientProvider>
    </AuthProvider>
  </StrictMode>
);
