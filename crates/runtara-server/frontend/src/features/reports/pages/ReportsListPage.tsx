import { Link } from 'react-router';
import { BarChart3, Edit, PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Icons } from '@/shared/components/icons';
import { TilesPage } from '@/shared/components/tiles-page';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useReports } from '../hooks/useReports';
import { ReportDeleteButton } from '../components/ReportDeleteButton';

export function ReportsListPage() {
  usePageTitle('Reports');

  const { data: reports = [], isFetching, isError, error } = useReports();

  return (
    <TilesPage
      kicker="Reports"
      title="Reporting"
      action={
        <Link to="/reports/new" className="w-full sm:w-auto">
          <Button className="w-full sm:w-auto sm:px-4">
            <PlusIcon className="mr-2 h-4 w-4" />
            New report
          </Button>
        </Link>
      }
    >
      {isFetching ? (
        <div className="rounded-lg border divide-y">
          {[...Array(5)].map((_, index) => (
            <div key={index} className="flex items-center gap-4 px-3 py-2.5">
              <div className="h-4 w-48 rounded bg-muted/60 animate-pulse" />
              <div className="h-4 w-72 rounded bg-muted/60 animate-pulse" />
              <div className="ml-auto h-4 w-32 rounded bg-muted/60 animate-pulse" />
            </div>
          ))}
        </div>
      ) : isError ? (
        <div className="flex flex-col items-center justify-center rounded-lg border bg-muted/20 px-6 py-10 text-center">
          <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
          <p className="text-base font-semibold text-foreground">
            Reports could not be loaded
          </p>
          <p className="mt-1 text-sm text-muted-foreground">
            {error?.message ?? 'Please try again.'}
          </p>
        </div>
      ) : reports.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-lg border bg-muted/20 px-6 py-12 text-center">
          <BarChart3 className="mb-4 h-10 w-10 text-muted-foreground" />
          <p className="text-base font-semibold text-foreground">
            No reports yet
          </p>
          <p className="mt-1 max-w-md text-sm text-muted-foreground">
            Create a report to render Object Model data as markdown, tables,
            metrics, and charts.
          </p>
        </div>
      ) : (
        <div className="rounded-lg border overflow-hidden">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Description</TableHead>
                <TableHead>Updated</TableHead>
                <TableHead className="w-0" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {reports.map((report) => (
                <TableRow key={report.id}>
                  <TableCell className="font-medium text-foreground">
                    <Link
                      to={`/reports/${report.id}`}
                      className="transition-colors hover:text-primary"
                    >
                      {report.name}
                    </Link>
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    <div className="max-w-[24rem] truncate">
                      {report.description || report.slug}
                    </div>
                  </TableCell>
                  <TableCell className="whitespace-nowrap text-muted-foreground">
                    {new Date(report.updatedAt).toLocaleString()}
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Link to={`/reports/${report.id}?edit=1`}>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 text-muted-foreground"
                          aria-label={`Edit ${report.name}`}
                          title={`Edit ${report.name}`}
                        >
                          <Edit className="h-4 w-4" />
                        </Button>
                      </Link>
                      <ReportDeleteButton
                        reportId={report.id}
                        reportName={report.name}
                        iconOnly
                        navigateAfterDelete={false}
                        triggerVariant="ghost"
                        triggerSize="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      />
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </TilesPage>
  );
}
