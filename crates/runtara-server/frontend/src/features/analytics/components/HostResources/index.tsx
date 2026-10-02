import type { SystemAnalyticsData } from '@/generated/RuntaraRuntimeApi';
import { Progress } from '@/shared/components/ui/progress';

import { formatBytes } from '../../utils';
import { AnalyticsHeroCard } from '../AnalyticsHeroCard';

function usedPercent(total: number, available: number): number {
  return total > 0 ? ((total - available) / total) * 100 : 0;
}

function Detail({ children }: { children: React.ReactNode }) {
  return (
    <div className="truncate text-xs text-muted-foreground">{children}</div>
  );
}

/**
 * The host the runtime is on, as three tiles for the usage dashboard's top row.
 *
 * This used to be its own "System" page - a whole submenu entry for three
 * tiles. Unlike the rest of the row these ignore the date range: they are a
 * live reading, which is what the "Host" prefix on each label stands for.
 * Rendered as a fragment so the parent grid lays them out alongside the usage
 * tiles.
 */
export function HostResourceTiles({
  data,
  loading,
}: {
  data: SystemAnalyticsData | undefined;
  loading: boolean;
}) {
  const { cpu, memory, disk } = data ?? {};
  const noData = <Detail>No data available</Detail>;

  return (
    <>
      <AnalyticsHeroCard
        label="Host CPU"
        value={cpu ? `${cpu.logicalCores} cores` : '—'}
        loading={loading}
      >
        {cpu ? (
          <Detail>
            {cpu.physicalCores} physical, {cpu.architecture}
          </Detail>
        ) : (
          noData
        )}
      </AnalyticsHeroCard>

      <AnalyticsHeroCard
        label="Host memory for workflows"
        value={memory ? formatBytes(memory.availableForWorkflowsBytes) : '—'}
        loading={loading}
      >
        {memory ? (
          <>
            <Progress
              value={usedPercent(memory.totalBytes, memory.availableBytes)}
              className="h-1"
              aria-label="Host memory used"
            />
            <Detail>
              {formatBytes(memory.availableBytes)} free of{' '}
              {formatBytes(memory.totalBytes)}
            </Detail>
          </>
        ) : (
          noData
        )}
      </AnalyticsHeroCard>

      <AnalyticsHeroCard
        label="Host disk free"
        value={disk ? formatBytes(disk.availableBytes) : '—'}
        loading={loading}
      >
        {disk ? (
          <>
            <Progress
              value={usedPercent(disk.totalBytes, disk.availableBytes)}
              className="h-1"
              aria-label="Host disk used"
            />
            <Detail>
              {formatBytes(disk.totalBytes - disk.availableBytes)} used of{' '}
              {formatBytes(disk.totalBytes)}
            </Detail>
          </>
        ) : (
          noData
        )}
      </AnalyticsHeroCard>
    </>
  );
}
