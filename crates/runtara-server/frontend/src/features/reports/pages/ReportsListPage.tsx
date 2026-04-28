import { Link } from 'react-router';
import { BarChart3, PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { Icons } from '@/shared/components/icons';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useReports } from '../hooks/useReports';

export function ReportsListPage() {
  usePageTitle('Reports');

  const { data: reports = [], isFetching, isError, error } = useReports();

  return (
    <TilesPage
      kicker="Reports"
      title="Reporting"
      action={
        <Link to="/reports/new" className="w-full sm:w-auto">
          <Button className="h-11 w-full rounded-full sm:w-auto sm:px-6">
            <PlusIcon className="mr-2 h-4 w-4" />
            New report
          </Button>
        </Link>
      }
    >
      {isFetching ? (
        <TileList>
          {[...Array(4)].map((_, index) => (
            <div
              key={index}
              className="flex items-center gap-4 rounded-xl bg-muted/20 p-4 animate-pulse"
            >
              <div className="h-10 w-10 rounded-full bg-muted/60" />
              <div className="flex-1 space-y-2">
                <div className="h-4 w-48 rounded bg-muted/60" />
                <div className="h-3 w-72 rounded bg-muted/60" />
              </div>
            </div>
          ))}
        </TileList>
      ) : isError ? (
        <TileList>
          <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-10 text-center">
            <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
            <p className="text-base font-semibold text-foreground">
              Reports could not be loaded
            </p>
            <p className="mt-1 text-sm text-muted-foreground">
              {error?.message ?? 'Please try again.'}
            </p>
          </div>
        </TileList>
      ) : reports.length === 0 ? (
        <TileList>
          <div className="flex flex-col items-center justify-center rounded-2xl bg-muted/20 px-6 py-12 text-center">
            <BarChart3 className="mb-4 h-10 w-10 text-muted-foreground" />
            <p className="text-base font-semibold text-foreground">
              No reports yet
            </p>
            <p className="mt-1 max-w-md text-sm text-muted-foreground">
              Create a report to render Object Model data as markdown, tables,
              metrics, and charts.
            </p>
          </div>
        </TileList>
      ) : (
        <TileList>
          {reports.map((report) => (
            <Link
              key={report.id}
              to={`/reports/${report.id}`}
              className="flex items-center gap-4 rounded-xl border bg-background p-4 transition-colors hover:bg-muted/40"
            >
              <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-blue-50 text-blue-600 dark:bg-blue-950 dark:text-blue-300">
                <BarChart3 className="h-5 w-5" />
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="truncate text-sm font-semibold text-foreground">
                    {report.name}
                  </p>
                  <Badge variant="secondary">{report.status}</Badge>
                </div>
                <p className="mt-1 truncate text-sm text-muted-foreground">
                  {report.description || report.slug}
                </p>
              </div>
              <p className="hidden text-xs text-muted-foreground sm:block">
                {new Date(report.updatedAt).toLocaleString()}
              </p>
            </Link>
          ))}
        </TileList>
      )}
    </TilesPage>
  );
}
