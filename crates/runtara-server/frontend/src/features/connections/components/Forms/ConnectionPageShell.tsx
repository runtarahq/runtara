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
    <div className="min-h-screen flex flex-col bg-slate-50/50 dark:bg-background">
      <header className="sticky top-0 z-10 bg-slate-50/80 backdrop-blur-sm border-b border-slate-200/60 dark:bg-background/80 dark:border-slate-700/60">
        <div className="px-6 py-4">
          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-3 min-w-0">
              <Link
                to={backHref}
                aria-label="Back to connections"
                className="p-1.5 -ml-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-colors dark:hover:text-slate-300 dark:hover:bg-slate-800"
              >
                <ArrowLeft className="w-5 h-5" />
              </Link>
              <div className="flex items-center gap-3 min-w-0">
                {integrationIcon}
                <div className="min-w-0">
                  <h1 className="text-lg font-semibold text-slate-900 truncate dark:text-slate-100">
                    {title}
                  </h1>
                  {(integrationName || integrationCategory) && (
                    <div className="flex items-center gap-2">
                      {integrationName && (
                        <span className="text-sm text-slate-500 dark:text-slate-400">
                          {integrationName}
                        </span>
                      )}
                      {integrationName && integrationCategory && (
                        <span className="w-1 h-1 bg-slate-300 rounded-full dark:bg-slate-600" />
                      )}
                      {integrationCategory && (
                        <span className="inline-flex items-center gap-1 text-xs text-slate-500 bg-slate-100 px-1.5 py-0.5 rounded dark:bg-slate-700 dark:text-slate-400">
                          <CategoryIcon className="w-3 h-3" />
                          {getCategoryLabel(integrationCategory)}
                        </span>
                      )}
                    </div>
                  )}
                </div>
              </div>
            </div>
            {headerActions && (
              <div className="flex items-center gap-2 flex-shrink-0">
                {headerActions}
              </div>
            )}
          </div>
        </div>
      </header>

      <div className="mx-auto w-full max-w-2xl flex-1 px-4 sm:px-6 py-6">
        {children}
      </div>

      {footer}
    </div>
  );
}
