import { type ReactNode } from 'react';
import { Link } from 'react-router';
import { BarChart3, Edit, PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Icons } from '@/shared/components/icons';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  Breadcrumb,
  ConsoleTableShell,
  ConsoleToolbar,
  StatusPill,
  TableStatusFooter,
} from '@/shared/components/console';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useReports } from '../hooks/useReports';
import { ReportDeleteButton } from '../components/ReportDeleteButton';

export function ReportsListPage() {
  usePageTitle('Reports');

  const { data: reports = [], isPending, isError, error } = useReports();

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={[{ label: 'Reports' }]} />}
      actions={
        <Link to="/reports/new">
          <Button>
            <PlusIcon className="mr-2 h-4 w-4" />
            New report
          </Button>
        </Link>
      }
    />
  );

  let body: ReactNode;
  if (isPending) {
    body = (
      <div className="divide-y divide-border/50">
        {[...Array(8)].map((_, index) => (
          <div key={index} className="flex items-center gap-4 px-5 py-3.5">
            <div className="h-4 w-48 animate-pulse rounded bg-muted/60" />
            <div className="h-4 w-72 animate-pulse rounded bg-muted/60" />
            <div className="ml-auto h-4 w-32 animate-pulse rounded bg-muted/60" />
          </div>
        ))}
      </div>
    );
  } else if (isError) {
    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-10 text-center">
        <Icons.warning className="mb-4 h-10 w-10 text-destructive" />
        <p className="text-base font-semibold text-foreground">
          Reports could not be loaded
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          {error?.message ?? 'Please try again.'}
        </p>
      </div>
    );
  } else if (reports.length === 0) {
    body = (
      <div className="flex h-full flex-col items-center justify-center px-6 py-12 text-center">
        <BarChart3 className="mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No reports yet
        </p>
        <p className="mt-1 max-w-md text-sm text-muted-foreground">
          Create a report to render Object Model data as markdown, tables,
          metrics, and charts.
        </p>
      </div>
    );
  } else {
    body = (
      <Table variant="console">
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Description</TableHead>
            <TableHead className="w-44">Updated</TableHead>
            <TableHead className="w-0" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {reports.map((report) => (
            <TableRow key={report.id}>
              <TableCell className="font-medium text-foreground">
                <span className="flex items-center gap-2">
                  <Link
                    to={`/reports/${report.id}`}
                    className="truncate transition-colors hover:text-primary"
                  >
                    {report.name}
                  </Link>
                  {report.needsReAuthoring ? (
                    <StatusPill
                      tone="warning"
                      label="Needs re-authoring"
                      className="shrink-0"
                    />
                  ) : null}
                </span>
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
    );
  }

  return (
    <ConsoleTableShell
      toolbar={toolbar}
      footer={
        !isPending && !isError && reports.length > 0 ? (
          <TableStatusFooter
            left={`${reports.length.toLocaleString()} report${
              reports.length === 1 ? '' : 's'
            }`}
          />
        ) : undefined
      }
    >
      {body}
    </ConsoleTableShell>
  );
}
