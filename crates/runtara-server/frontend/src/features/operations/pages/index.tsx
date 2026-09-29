import { Route, Routes } from 'react-router';
import { ManagedInputScope } from '@/features/workflows/components/ManagedInputSubmissions';
import { OverviewPage } from './Overview';
import { QueuePage } from './Queue';
import { MonitorPage } from './Monitor';
import { RunPage } from './Run';
export function Operations() {
  return (
    <ManagedInputScope>
      <Routes>
        <Route index element={<OverviewPage />} />
        <Route path="queues/:workflowId/:actionKey" element={<QueuePage />} />
        <Route path="processes/:workflowId" element={<QueuePage />} />
        <Route path="views/:viewId" element={<QueuePage />} />
        <Route path="monitor" element={<MonitorPage />} />
        <Route path="runs/:workflowId/:instanceId" element={<RunPage />} />
      </Routes>
    </ManagedInputScope>
  );
}
