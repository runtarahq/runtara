import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { InvocationHistoryTable } from '../../components/InvocationHistoryTable';

export function InvocationHistory() {
  usePageTitle('Invocation History');

  return (
    <div className="min-h-screen bg-slate-50/50 dark:bg-background">
      {/* Header */}
      <header className="sticky top-0 z-10 bg-slate-50/80 backdrop-blur-sm border-b border-slate-200/60 dark:bg-background/80 dark:border-slate-800/60">
        <div className="px-8 py-5">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-xs font-semibold text-blue-600 uppercase tracking-wider mb-1 dark:text-blue-400">
                History
              </p>
              <h1 className="text-xl font-semibold text-slate-900 dark:text-slate-100">
                Invocation History
              </h1>
            </div>
          </div>
        </div>
      </header>

      {/* Content */}
      <div className="px-8 py-6">
        <InvocationHistoryTable />
      </div>
    </div>
  );
}
