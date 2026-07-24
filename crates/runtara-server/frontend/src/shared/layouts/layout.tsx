import React, { useEffect } from 'react';
import { Outlet } from 'react-router';
import {
  SidebarInset,
  SidebarProvider,
} from '@/shared/components/ui/sidebar.tsx';
import { Sidebar } from './Sidebar.tsx';
import { cleanupPointerEvents } from '@/lib/utils';
import { NavigationBlocker } from '@/shared/components/NavigationBlocker';

interface LayoutProps {
  children?: React.ReactNode;
}

export function Layout({ children }: LayoutProps) {
  // Global cleanup for pointer-events
  useEffect(() => {
    // Clean up on mount and periodically
    const cleanup = () => {
      cleanupPointerEvents();
    };

    // Clean up immediately
    cleanup();

    // Add event listeners to clean up after user interactions
    document.addEventListener('keydown', cleanup);
    document.addEventListener('mousedown', cleanup);

    // Clean up on unmount
    return () => {
      cleanup();
      document.removeEventListener('keydown', cleanup);
      document.removeEventListener('mousedown', cleanup);
    };
  }, []);

  return (
    <SidebarProvider>
      <div className="pt-safe flex min-h-dvh w-full overflow-hidden">
        <Sidebar />
        <SidebarInset className="flex min-w-0 flex-col overflow-y-auto overflow-x-hidden bg-muted/30 dark:bg-background">
          <div className="pb-safe flex-1">{children ?? <Outlet />}</div>
        </SidebarInset>
      </div>
      <NavigationBlocker />
    </SidebarProvider>
  );
}
