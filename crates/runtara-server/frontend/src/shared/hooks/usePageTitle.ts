import { useEffect } from 'react';

export function usePageTitle(title: string) {
  useEffect(() => {
    const fullTitle = title ? `SyncMyOrders - ${title}` : 'SyncMyOrders';
    document.title = fullTitle;

    // Reset title on unmount
    return () => {
      document.title = 'SyncMyOrders';
    };
  }, [title]);
}
