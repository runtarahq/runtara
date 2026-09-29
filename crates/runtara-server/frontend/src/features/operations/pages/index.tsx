import { lazy } from 'react';
import { Route, Routes } from 'react-router';
import { ManagedInputScope } from '@/features/workflows/components/ManagedInputSubmissions';
import { OverviewPage } from './Overview';
import { QueuesPage } from './Queues';
import { QueuePage } from './Queue';
import { MonitorPage } from './Monitor';
import { RunPage } from './Run';
const RunsPage = lazy(() =>
  import('@/features/invocation-history/pages/InvocationHistory').then((m) => ({
    default: m.InvocationHistory,
  }))
);

export function Operations() {
  return (
    <ManagedInputScope>
      <Routes>
        <Route index element={<OverviewPage />} />
        <Route path="queues" element={<QueuesPage />} />
        <Route path="queues/:workflowId/:actionKey" element={<QueuePage />} />
        <Route path="processes/:workflowId" element={<QueuePage />} />
        <Route path="views/:viewId" element={<QueuePage />} />
        <Route path="runs" element={<RunsPage />} />
        <Route path="monitor" element={<MonitorPage />} />
        <Route path="runs/:workflowId/:instanceId" element={<RunPage />} />
      </Routes>
    </ManagedInputScope>
  );
}
