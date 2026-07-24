import { useSyncExternalStore } from 'react';
import { create } from 'zustand';
import { devtools, persist } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';

type Theme = 'light' | 'dark' | 'system';

interface ThemeState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  getEffectiveTheme: () => 'light' | 'dark';
}

export const useThemeStore = create<ThemeState>()(
  devtools(
    persist(
      immer((set, get) => ({
        theme: 'system',
        setTheme: (theme) => {
          set({ theme });
          updateThemeClass(theme);
        },
        getEffectiveTheme: () => {
          const { theme } = get();
          if (theme === 'system') {
            return window.matchMedia('(prefers-color-scheme: dark)').matches
              ? 'dark'
              : 'light';
          }
          return theme;
        },
      })),
      {
        name: 'theme-storage',
      }
    )
  )
);

/**
 * Reactive dark-mode flag. Unlike reading
 * `document.documentElement.classList.contains('dark')` during render (which
 * goes stale when the theme is toggled), this re-renders the consumer on both
 * explicit theme changes and OS-level scheme flips while in 'system' mode.
 */
export function useIsDarkTheme(): boolean {
  const theme = useThemeStore((s) => s.theme);
  const systemDark = useSyncExternalStore(subscribeToSystemScheme, () =>
    typeof window === 'undefined'
      ? false
      : window.matchMedia('(prefers-color-scheme: dark)').matches
  );
  return theme === 'dark' || (theme === 'system' && systemDark);
}

function subscribeToSystemScheme(callback: () => void) {
  const mq = window.matchMedia('(prefers-color-scheme: dark)');
  mq.addEventListener('change', callback);
  return () => mq.removeEventListener('change', callback);
}

function updateThemeClass(theme: Theme) {
  const root = document.documentElement;
  const isDark =
    theme === 'dark' ||
    (theme === 'system' &&
      window.matchMedia('(prefers-color-scheme: dark)').matches);

  if (isDark) {
    root.classList.add('dark');
  } else {
    root.classList.remove('dark');
  }
}

// Initialize theme on load. Guard every browser API separately — test
// environments may provide window without localStorage/matchMedia.
if (
  typeof window !== 'undefined' &&
  typeof localStorage !== 'undefined' &&
  typeof window.matchMedia === 'function'
) {
  const stored = localStorage.getItem('theme-storage');
  if (stored) {
    try {
      const { state } = JSON.parse(stored);
      updateThemeClass(state.theme);
    } catch {
      updateThemeClass('system');
    }
  } else {
    updateThemeClass('system');
  }

  // Listen for system theme changes
  window
    .matchMedia('(prefers-color-scheme: dark)')
    .addEventListener('change', () => {
      const currentTheme = useThemeStore.getState().theme;
      if (currentTheme === 'system') {
        updateThemeClass('system');
      }
    });
}
