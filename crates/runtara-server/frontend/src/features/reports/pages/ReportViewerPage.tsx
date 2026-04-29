import { useMemo } from 'react';
import { Link, useParams, useSearchParams } from 'react-router';
import { Edit, Printer, RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { TileList, TilesPage } from '@/shared/components/tiles-page';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import RuntaraLogo from '@/assets/logo/runtara-logo-icon.svg';
import { ReportFilterBar } from '../components/ReportFilterBar';
import { ReportRenderer } from '../components/ReportRenderer';
import { useReport, useReportRender } from '../hooks/useReports';
import {
  decodeFilterValue,
  encodeFilterValue,
  getEagerBlocks,
  getFilterDefaultValue,
} from '../utils';

export function ReportViewerPage() {
  const { reportId } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: report, isFetching, isError, error } = useReport(reportId);

  usePageTitle(report?.name ?? 'Report');

  const filters = useMemo(() => {
    if (!report) return {};
    return Object.fromEntries(
      report.definition.filters.map((filter) => [
        filter.id,
        decodeFilterValue(filter, searchParams.get(filter.id)),
      ])
    );
  }, [report, searchParams]);

  const eagerBlocks = useMemo(
    () => (report ? getEagerBlocks(report.definition) : []),
    [report]
  );
  const renderRequest = useMemo(
    () =>
      report
        ? {
            filters,
            blocks: eagerBlocks.map((block) => ({
              id: block.id,
              page:
                block.type === 'table'
                  ? {
                      offset: 0,
                      size: block.table?.pagination?.defaultPageSize ?? 50,
                    }
                  : undefined,
              sort: block.table?.defaultSort ?? [],
            })),
            timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          }
        : undefined,
    [eagerBlocks, filters, report]
  );

  const {
    data: renderResponse,
    isFetching: isRendering,
    refetch,
  } = useReportRender(reportId, renderRequest, Boolean(report));

  const handleFilterChanges = (updates: Record<string, unknown>) => {
    setSearchParams(
      (currentParams) => {
        const nextParams = new URLSearchParams(currentParams);
        for (const [filterId, value] of Object.entries(updates)) {
          const filter = report?.definition.filters.find(
            (filter) => filter.id === filterId
          );
          const defaultValue = filter
            ? getFilterDefaultValue(filter)
            : undefined;
          if (
            isEmptyFilterValue(value) ||
            isSameFilterValue(value, defaultValue)
          ) {
            nextParams.delete(filterId);
          } else {
            nextParams.set(filterId, encodeFilterValue(value));
          }
        }
        return nextParams;
      },
      { replace: true }
    );
  };

  const handleFilterChange = (filterId: string, value: unknown) => {
    handleFilterChanges({ [filterId]: value });
  };

  const handlePrint = () => {
    window.requestAnimationFrame(() => window.print());
  };

  if (isFetching) {
    return (
      <TilesPage kicker="Reports" title="Loading report">
        <TileList>
          <div className="h-96 animate-pulse rounded-xl bg-muted/30" />
        </TileList>
      </TilesPage>
    );
  }

  if (isError || !report) {
    return (
      <TilesPage kicker="Reports" title="Report unavailable">
        <TileList>
          <div className="rounded-xl border bg-background p-6 text-sm text-muted-foreground">
            {error?.message ?? 'The report could not be loaded.'}
          </div>
        </TileList>
      </TilesPage>
    );
  }

  return (
    <TilesPage
      kicker="Reports"
      title={report.name}
      toolbar={
        <ReportFilterBar
          reportId={report.id}
          definition={report.definition}
          values={filters}
          onChange={handleFilterChange}
        />
      }
      action={
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row">
          <Button
            variant="outline"
            className="h-11 rounded-full sm:px-5"
            onClick={handlePrint}
            disabled={isRendering}
          >
            <Printer className="mr-2 h-4 w-4" />
            Print / PDF
          </Button>
          <Button
            variant="outline"
            className="h-11 rounded-full sm:px-5"
            onClick={() => refetch()}
            disabled={isRendering}
          >
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
          <Link to={`/reports/${report.id}/edit`} className="w-full sm:w-auto">
            <Button className="h-11 w-full rounded-full sm:w-auto sm:px-5">
              <Edit className="mr-2 h-4 w-4" />
              Edit
            </Button>
          </Link>
        </div>
      }
      className="report-print-root"
      contentClassName="report-print-content pb-16"
    >
      <ReportRenderer
        reportId={report.id}
        definition={report.definition}
        renderResponse={renderResponse}
        filters={filters}
        onFilterChange={handleFilterChange}
        onFiltersChange={handleFilterChanges}
      />
      <div className="report-print-brand">
        <img src={RuntaraLogo} alt="" />
        <span>Generated in Runtara</span>
      </div>
    </TilesPage>
  );
}

function isEmptyFilterValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  return false;
}

function isSameFilterValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
