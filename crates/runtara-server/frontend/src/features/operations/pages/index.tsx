import { lazy } from 'react';
import { Route, Routes } from 'react-router';
import { ManagedInputScope } from '@/features/workflows/components/ManagedInputSubmissions';
import { OverviewPage } from './Overview';
import { RequestsPage } from './Requests';
import { QueuesPage } from './Queues';
import { QueuePage } from './Queue';
import { InvocationHistoryRedirect } from '@/router/InvocationHistoryRedirect';
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
        <Route path="requests" element={<RequestsPage />} />
        <Route path="queues" element={<QueuesPage />} />
        <Route path="queues/:workflowId/:actionKey" element={<QueuePage />} />
        <Route path="processes/:workflowId" element={<QueuePage />} />
        <Route path="views/:viewId" element={<QueuePage />} />
        <Route path="runs" element={<RunsPage />} />
        <Route path="monitor" element={<InvocationHistoryRedirect />} />
        <Route path="runs/:workflowId/:instanceId" element={<RunPage />} />
      </Routes>
    </ManagedInputScope>
  );
}
