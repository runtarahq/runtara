import { type ReactNode } from 'react';
import { Link } from 'react-router';
import { useWatch } from 'react-hook-form';
import { ArrowLeft } from 'lucide-react';

import {
  getCategoryIcon,
  getCategoryLabel,
} from '@/features/connections/utils/category-icons';

type ConnectionPageShellProps = {
  mode: 'create' | 'edit';
  backHref?: string;
  integrationIcon?: ReactNode;
  integrationName?: string;
  integrationCategory?: string;
  /** Interim header actions (Reconnect/Delete) until the status card and danger zone land. */
  headerActions?: ReactNode;
  /** Sticky bottom save bar. */
  footer?: ReactNode;
  children: ReactNode;
};

/**
 * Page chrome for the connection editor: sticky header with the live
 * connection title (the Title field doubles as the rename affordance),
 * a centered content column, and a sticky footer slot for the save bar.
 */
export function ConnectionPageShell({
  mode,
  backHref = '/connections',
  integrationIcon,
  integrationName,
  integrationCategory,
  headerActions,
  footer,
  children,
}: ConnectionPageShellProps) {
  const watchedTitle = useWatch({ name: 'title' }) as string | undefined;
  const title =
    watchedTitle?.trim() ||
    (mode === 'create' ? 'New connection' : 'Connection');
  const CategoryIcon = getCategoryIcon(integrationCategory);

  return (
    <div className="flex min-h-screen flex-col bg-muted/30 dark:bg-background">
      <header className="sticky top-0 z-10 border-b border-border bg-muted/50 backdrop-blur-sm dark:bg-background/80">
        <div className="px-6 py-4">
          <div className="flex items-center justify-between gap-4">
            <div className="flex min-w-0 items-center gap-3">
              <Link
                to={backHref}
                aria-label="Back to connections"
                className="-ml-1.5 rounded-lg p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                <ArrowLeft className="h-5 w-5" />
              </Link>
              <div className="flex min-w-0 items-center gap-3">
                {integrationIcon}
                <div className="min-w-0">
                  <h1 className="truncate text-lg font-semibold text-foreground">
                    {title}
                  </h1>
                  {(integrationName || integrationCategory) && (
                    <div className="flex items-center gap-2">
                      {integrationName && (
                        <span className="text-sm text-muted-foreground">
                          {integrationName}
                        </span>
                      )}
                      {integrationName && integrationCategory && (
                        <span className="h-1 w-1 rounded-full bg-border" />
                      )}
                      {integrationCategory && (
                        <span className="inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                          <CategoryIcon className="h-3 w-3" />
                          {getCategoryLabel(integrationCategory)}
                        </span>
                      )}
                    </div>
                  )}
                </div>
              </div>
            </div>
            {headerActions && (
              <div className="flex flex-shrink-0 items-center gap-2">
                {headerActions}
              </div>
            )}
          </div>
        </div>
      </header>

      <div className="mx-auto w-full max-w-2xl flex-1 px-4 py-6 sm:px-6">
        {children}
      </div>

      {footer}
    </div>
  );
}
