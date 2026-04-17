import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { RefreshCw, Cpu, HardDrive, MemoryStick } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Card } from '@/shared/components/ui/card';
import { Progress } from '@/shared/components/ui/progress';
import { useSystemAnalytics } from '../../hooks/useAnalytics';
import { formatBytes } from '../../utils';

export function System() {
  usePageTitle('System Analytics');

  const {
    data: systemAnalytics,
    isLoading: systemLoading,
    refetch: refetchSystem,
  } = useSystemAnalytics();

  const handleRefresh = () => {
    refetchSystem();
  };

  return (
    <div className="w-full px-4 py-3">
      <div className="flex w-full flex-col gap-3">
        <section className="bg-transparent">
          <div className="flex flex-col gap-2 lg:flex-row lg:items-end lg:justify-between">
            <div className="space-y-0.5">
              <p className="text-xs font-semibold uppercase tracking-[0.2em] text-muted-foreground">
                Analytics
              </p>
              <h1 className="text-xl font-semibold leading-tight text-slate-900/90 dark:text-slate-100">
                System
              </h1>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                onClick={handleRefresh}
                variant="ghost"
                size="sm"
                className="h-9 px-3 text-xs font-medium text-muted-foreground hover:text-foreground"
              >
                <RefreshCw className="h-4 w-4 mr-2" />
                Refresh
              </Button>
            </div>
          </div>
        </section>

        <section>
          <div className="grid gap-4 md:grid-cols-3">
            {/* CPU Info */}
            <Card className="rounded-xl border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Cpu className="h-5 w-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      CPU
                    </span>
                  </div>
                  <div className="h-6 w-32 rounded bg-muted animate-pulse" />
                  <div className="h-4 w-24 rounded bg-muted animate-pulse" />
                </div>
              ) : systemAnalytics?.data?.cpu ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <Cpu className="h-5 w-5 text-blue-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      CPU
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-slate-900/90 dark:text-slate-100">
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
                    <Cpu className="h-5 w-5 text-muted-foreground" />
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
            <Card className="rounded-xl border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <MemoryStick className="h-5 w-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Memory
                    </span>
                  </div>
                  <div className="h-6 w-32 rounded bg-muted animate-pulse" />
                  <div className="h-2 w-full rounded bg-muted animate-pulse" />
                  <div className="h-4 w-40 rounded bg-muted animate-pulse" />
                </div>
              ) : systemAnalytics?.data?.memory ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <MemoryStick className="h-5 w-5 text-green-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Memory
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-slate-900/90 dark:text-slate-100">
                    {formatBytes(
                      systemAnalytics.data.memory.availableForScenariosBytes
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
                    <MemoryStick className="h-5 w-5 text-muted-foreground" />
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
            <Card className="rounded-xl border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
              {systemLoading ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <HardDrive className="h-5 w-5 text-muted-foreground" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Disk
                    </span>
                  </div>
                  <div className="h-6 w-32 rounded bg-muted animate-pulse" />
                  <div className="h-2 w-full rounded bg-muted animate-pulse" />
                  <div className="h-4 w-40 rounded bg-muted animate-pulse" />
                </div>
              ) : systemAnalytics?.data?.disk ? (
                <div className="space-y-3">
                  <div className="flex items-center gap-2">
                    <HardDrive className="h-5 w-5 text-purple-500" />
                    <span className="text-sm font-semibold text-muted-foreground">
                      Disk
                    </span>
                  </div>
                  <div className="text-2xl font-semibold text-slate-900/90 dark:text-slate-100">
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
                    <HardDrive className="h-5 w-5 text-muted-foreground" />
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
    </div>
  );
}
