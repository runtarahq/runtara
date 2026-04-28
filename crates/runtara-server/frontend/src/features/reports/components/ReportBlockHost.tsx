import { useEffect, useMemo, useRef, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { ReportBlockDefinition, ReportBlockResult } from '../types';
import { useReportBlockData } from '../hooks/useReports';
import { ChartBlock } from './blocks/ChartBlock';
import { MetricBlock } from './blocks/MetricBlock';
import { TableBlock } from './blocks/TableBlock';

type ReportBlockHostProps = {
  reportId: string;
  block: ReportBlockDefinition;
  initialResult?: ReportBlockResult;
  filters: Record<string, unknown>;
};

export function ReportBlockHost({
  reportId,
  block,
  initialResult,
  filters,
}: ReportBlockHostProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [isVisible, setIsVisible] = useState(!block.lazy);
  const defaultPageSize = getDefaultPageSize(block);
  const [page, setPage] = useState({ offset: 0, size: defaultPageSize });

  useEffect(() => {
    if (!block.lazy || isVisible) return;
    const element = rootRef.current;
    if (!element) return;

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setIsVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: '200px' }
    );

    observer.observe(element);
    return () => observer.disconnect();
  }, [block.lazy, isVisible]);

  useEffect(() => {
    setPage({ offset: 0, size: defaultPageSize });
  }, [block.id, defaultPageSize, filters]);

  const needsBlockFetch =
    isVisible && (block.lazy || !initialResult || page.offset > 0);

  const request = useMemo(
    () => ({
      filters,
      page: block.type === 'table' ? page : undefined,
      sort: block.table?.defaultSort ?? [],
      blockFilters: {},
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }),
    [block.table?.defaultSort, block.type, filters, page]
  );

  const {
    data: fetchedResult,
    isFetching,
    refetch,
  } = useReportBlockData(reportId, block.id, request, needsBlockFetch);

  const result = fetchedResult ?? initialResult;

  return (
    <div ref={rootRef} className="my-5">
      {block.title && (
        <div className="mb-2 flex items-center justify-between gap-3">
          <h2 className="text-base font-semibold text-foreground">
            {block.title}
          </h2>
          {result?.status === 'error' && (
            <Button variant="outline" size="sm" onClick={() => refetch()}>
              <RefreshCw className="mr-2 h-4 w-4" />
              Retry
            </Button>
          )}
        </div>
      )}
      {!isVisible || isFetching ? (
        <BlockSkeleton block={block} />
      ) : !result ? (
        <BlockSkeleton block={block} />
      ) : result.status === 'error' ? (
        <BlockError result={result} onRetry={() => refetch()} />
      ) : (
        <RenderedBlock
          block={block}
          result={result}
          onPageChange={(offset, size) => setPage({ offset, size })}
        />
      )}
    </div>
  );
}

function RenderedBlock({
  block,
  result,
  onPageChange,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  onPageChange: (offset: number, size: number) => void;
}) {
  if (block.type === 'table') {
    return (
      <TableBlock block={block} result={result} onPageChange={onPageChange} />
    );
  }

  if (block.type === 'chart') {
    return <ChartBlock block={block} result={result} />;
  }

  if (block.type === 'metric') {
    return <MetricBlock block={block} result={result} />;
  }

  return null;
}

function BlockSkeleton({ block }: { block: ReportBlockDefinition }) {
  const height = block.type === 'metric' ? 'h-28' : 'h-72';
  return (
    <div
      className={`${height} animate-pulse rounded-lg border bg-muted/30`}
      aria-label="Loading report block"
    />
  );
}

function BlockError({
  result,
  onRetry,
}: {
  result: ReportBlockResult;
  onRetry: () => void;
}) {
  return (
    <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4">
      <p className="text-sm font-semibold text-destructive">
        {result.error?.message ?? 'This report block could not be rendered.'}
      </p>
      <Button className="mt-3" variant="outline" size="sm" onClick={onRetry}>
        <RefreshCw className="mr-2 h-4 w-4" />
        Retry
      </Button>
    </div>
  );
}

function getDefaultPageSize(block: ReportBlockDefinition) {
  return block.table?.pagination?.defaultPageSize ?? 50;
}
