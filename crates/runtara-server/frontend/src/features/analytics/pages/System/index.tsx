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
              variant="outline"
              size="sm"
              className="text-muted-foreground"
            >
              <RefreshCw className="mr-2 h-4 w-4" />
              Refresh
            </Button>
          }
        />
      }
    >
      <div className="space-y-4">
        <section>
          <div className="grid gap-4 md:grid-cols-3">
            {/* CPU Info */}
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
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
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
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
            <Card className="rounded-lg border border-border/40 bg-card px-4 py-4 sm:px-5 shadow-none">
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
    </ConsoleTableShell>
  );
}
