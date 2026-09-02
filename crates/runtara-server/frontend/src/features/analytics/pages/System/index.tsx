import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw, Cpu, HardDrive, MemoryStick } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Breadcrumb,
  ConsoleTableShell,
  ConsoleToolbar,
} from '@/shared/components/console';
import { Card } from '@/shared/components/ui/card';
import { Progress } from '@/shared/components/ui/progress';
import { useSystemAnalytics } from '../../hooks/useAnalytics';
import { usePipelineStream } from '../../hooks/usePipelineStream';
import { PipelineRates } from '../../components/PipelineRates';
import { PipelineStageRow } from '../../components/PipelineStageRow';
import { formatBytes } from '../../utils';

export function System() {
  usePageTitle('System Analytics');

  const {
    data: systemAnalytics,
    isLoading: systemLoading,
    refetch: refetchSystem,
  } = useSystemAnalytics();

  const pipeline = usePipelineStream();

  const handleRefresh = () => {
    refetchSystem();
  };

  return (
    <ConsoleTableShell
      bodyClassName="p-4 md:p-6"
      toolbar={
        <ConsoleToolbar
          left={
            <Breadcrumb
              items={[
                { label: 'Analytics', to: '/analytics/usage' },
                { label: 'System' },
              ]}
            />
          }
          actions={
            <Button
              onClick={handleRefresh}
              variant="secondary"
              bordered
              size="sm"
            >
              <RefreshCw className="mr-2 size-4" />
              Refresh
            </Button>
          }
        />
      }
    >
      <div className="space-y-6">
        {/* Occupancy leads: it is the live thing, and the host description
            below it is the context for why the bounds are what they are. */}
        <section aria-label="Execution pipeline">
          <div className="mb-3 flex items-baseline justify-between gap-3">
            <div>
              <h2 className="text-sm font-semibold text-foreground">
                Execution pipeline
              </h2>
              <p className="text-xs text-muted-foreground">
                Occupancy against every concurrency limit, sampled each second
              </p>
            </div>
            <span className="text-[11px] tabular-nums text-muted-foreground">
              {pipeline.connected
                ? 'live'
                : pipeline.snapshot
                  ? 'polling'
                  : 'connecting…'}
            </span>
          </div>

          {pipeline.snapshot ? (
            <div className="space-y-3">
              <PipelineRates rates={pipeline.snapshot.rates} />
              <div className="space-y-1.5">
                {pipeline.snapshot.stages.map((stage) => (
                  <PipelineStageRow
                    key={stage.key}
                    stage={stage}
                    history={pipeline.history[stage.key] ?? []}
                    inflow={
                      pipeline.snapshot?.rates
                        ? ((
                            pipeline.snapshot.rates as unknown as Record<
                              string,
                              number | null
                            >
                          )[stage.inflowKey] ?? null)
                        : null
                    }
                    isChokepoint={pipeline.chokepointKey === stage.key}
                  />
                ))}
              </div>
            </div>
          ) : (
            <div className="rounded-lg border border-border/40 bg-card px-4 py-6 text-sm text-muted-foreground">
              {pipeline.error
                ? `Pipeline unavailable: ${pipeline.error}`
                : 'Waiting for the first sample…'}
            </div>
          )}
        </section>

        <section aria-label="Host">
          <h2 className="mb-3 text-sm font-semibold text-foreground">Host</h2>
          <div className="grid gap-4 md:grid-cols-3">
            {/* CPU Info */}
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 shadow-none sm:px-5">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Cpu className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      CPU
                    </span>
                  </div>
                  <div className="h-6 w-32 animate-pulse rounded bg-muted" />
                  <div className="h-4 w-24 animate-pulse rounded bg-muted" />
                </div>
              ) : systemAnalytics?.data?.cpu ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Cpu className="size-5 text-blue-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      CPU
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-foreground">
                    {systemAnalytics.data.cpu.logicalCores} Cores
                  </div>
                  <div className="text-sm text-muted-foreground">
                    {systemAnalytics.data.cpu.physicalCores} physical,{' '}
                    {systemAnalytics.data.cpu.architecture}
                  </div>
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Cpu className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      CPU
                    </span>
                  </div>
                  <div className="text-sm text-muted-foreground">
                    No data available
                  </div>
                </div>
              )}
            </Card>

            {/* Memory Info */}
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 shadow-none sm:px-5">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <MemoryStick className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Memory
                    </span>
                  </div>
                  <div className="h-6 w-32 animate-pulse rounded bg-muted" />
                  <div className="h-2 w-full animate-pulse rounded bg-muted" />
                  <div className="h-4 w-40 animate-pulse rounded bg-muted" />
                </div>
              ) : systemAnalytics?.data?.memory ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <MemoryStick className="size-5 text-green-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Memory
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-foreground">
                    {formatBytes(
                      systemAnalytics.data.memory.availableForWorkflowsBytes
                    )}{' '}
                    available
                  </div>
                  <Progress
                    value={
                      ((systemAnalytics.data.memory.totalBytes -
                        systemAnalytics.data.memory.availableBytes) /
                        systemAnalytics.data.memory.totalBytes) *
                      100
                    }
                    className="h-2"
                  />
                  <div className="text-sm text-muted-foreground">
                    {formatBytes(systemAnalytics.data.memory.availableBytes)}{' '}
                    free of{' '}
                    {formatBytes(systemAnalytics.data.memory.totalBytes)} total
                  </div>
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <MemoryStick className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Memory
                    </span>
                  </div>
                  <div className="text-sm text-muted-foreground">
                    No data available
                  </div>
                </div>
              )}
            </Card>

            {/* Disk Info */}
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 shadow-none sm:px-5">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <HardDrive className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Disk
                    </span>
                  </div>
                  <div className="h-6 w-32 animate-pulse rounded bg-muted" />
                  <div className="h-2 w-full animate-pulse rounded bg-muted" />
                  <div className="h-4 w-40 animate-pulse rounded bg-muted" />
                </div>
              ) : systemAnalytics?.data?.disk ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <HardDrive className="size-5 text-purple-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Disk
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-foreground">
                    {formatBytes(systemAnalytics.data.disk.availableBytes)}{' '}
                    available
                  </div>
                  <Progress
                    value={
                      ((systemAnalytics.data.disk.totalBytes -
                        systemAnalytics.data.disk.availableBytes) /
                        systemAnalytics.data.disk.totalBytes) *
                      100
                    }
                    className="h-2"
                  />
                  <div className="text-sm text-muted-foreground">
                    {formatBytes(
                      systemAnalytics.data.disk.totalBytes -
                        systemAnalytics.data.disk.availableBytes
                    )}{' '}
                    used of {formatBytes(systemAnalytics.data.disk.totalBytes)}
                  </div>
                </div>
              ) : (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <HardDrive className="size-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Disk
                    </span>
                  </div>
                  <div className="text-sm text-muted-foreground">
                    No data available
                  </div>
                </div>
              )}
            </Card>
          </div>
        </section>
      </div>
    </ConsoleTableShell>
  );
}
