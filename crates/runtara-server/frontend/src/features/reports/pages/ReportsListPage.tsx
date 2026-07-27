import { type ReactNode } from 'react';
import { Link } from 'react-router';
import { BarChart3, Edit, PlusIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { Can } from '@/shared/components/Can';
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
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  ConsoleToolbar,
  StatusPill,
  TableSkeletonRows,
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
        <Can permission="report:create">
          <Link to="/reports/new">
            <Button>
              <PlusIcon className="mr-2 size-4" />
              New report
            </Button>
          </Link>
        </Can>
      }
    />
  );

  let body: ReactNode;
  if (isPending) {
    body = (
      <TableSkeletonRows rows={8} widths={['w-48', 'w-72', 'ml-auto w-32']} />
    );
  } else if (isError) {
    body = <ConsoleErrorState error={error} entityLabel="reports" />;
  } else if (reports.length === 0) {
    body = (
      <ConsoleEmptyState
        icon={<BarChart3 className="mb-4 size-10 text-muted-foreground" />}
        title="No reports yet"
        description="Create a report to render Object Model data as markdown, tables, metrics, and charts."
      />
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
                  <Can permission="report:update">
                    <Link to={`/reports/${report.id}?edit=1`}>
                      <WithTooltip label={`Edit ${report.name}`}>
                        <Button
                          type="button"
                          variant="quiet"
                          size="icon-sm"
                          aria-label={`Edit ${report.name}`}
                        >
                          <Edit className="size-4" />
                        </Button>
                      </WithTooltip>
                    </Link>
                  </Can>
                  <Can permission="report:delete">
                    <ReportDeleteButton
                      reportId={report.id}
                      reportName={report.name}
                      iconOnly
                      navigateAfterDelete={false}
                      triggerVariant="ghost"
                      triggerSize="icon-sm"
                      className="text-muted-foreground hover:text-destructive"
                    />
                  </Can>
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
