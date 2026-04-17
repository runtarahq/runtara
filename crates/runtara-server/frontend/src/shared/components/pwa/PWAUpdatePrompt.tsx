import { useRegisterSW } from 'virtual:pwa-register/react';
import { Button } from '@/shared/components/ui/button';
import { RefreshCw } from 'lucide-react';

async function clearAllCaches() {
  if ('caches' in window) {
    const cacheNames = await caches.keys();
    await Promise.all(cacheNames.map((cacheName) => caches.delete(cacheName)));
  }
}

function delay(ms: number) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

export function PWAUpdatePrompt() {
  const {
    needRefresh: [needRefresh, setNeedRefresh],
    updateServiceWorker,
  } = useRegisterSW({
    onRegistered(r) {
      console.log('SW registered:', r);
    },
    onRegisterError(error) {
      console.log('SW registration error:', error);
    },
  });

  const handleUpdate = async () => {
    setNeedRefresh(false);

    try {
      await clearAllCaches();
    } catch (error) {
      console.warn('Failed to clear caches before update:', error);
    }

    try {
      await Promise.race([updateServiceWorker(true), delay(1500)]);
    } catch (error) {
      console.warn('Service worker update failed, forcing reload:', error);
    }

    window.location.reload();
  };

  if (!needRefresh) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 bg-card border rounded-lg shadow-lg p-4 max-w-sm">
      <p className="text-sm mb-3">
        A new version is available. Reload to update.
      </p>
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          onClick={() => setNeedRefresh(false)}
        >
          Dismiss
        </Button>
        <Button size="sm" onClick={handleUpdate}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Update
        </Button>
      </div>
    </div>
  );
}
