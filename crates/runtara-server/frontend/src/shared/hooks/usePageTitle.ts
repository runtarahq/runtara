import { useEffect } from 'react';

export function usePageTitle(title: string) {
  useEffect(() => {
    const fullTitle = title ? `Runtara - ${title}` : 'Runtara';
    document.title = fullTitle;

    // Reset title on unmount
    return () => {
      document.title = 'Runtara';
    };
  }, [title]);
}
