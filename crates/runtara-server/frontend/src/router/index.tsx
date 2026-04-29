/* eslint-disable react-refresh/only-export-components */
// Router config files naturally export route configurations that reference components.
// Separating would require splitting the routing logic from its component references.
import { createBrowserRouter, Navigate } from 'react-router';
import { lazy, Suspense } from 'react';
import { PrivateRoute } from '@/router/PrivateRoute';
import { Layout } from '@/shared/layouts/layout';
import { Login } from '@/shared/pages/login';
import { ErrorBoundary } from '@/shared/components/error-boundary.tsx';

// Lazy load all page components
const Connections = lazy(() =>
  import('@/features/connections/pages/Connections').then((m) => ({
    default: m.Connections,
  }))
);
const Connection = lazy(() =>
  import('@/features/connections/pages/Connection').then((m) => ({
    default: m.Connection,
  }))
);
const CreateConnection = lazy(() =>
  import('@/features/connections/pages/CreateConnection').then((m) => ({
    default: m.CreateConnection,
  }))
);
const CreateTrigger = lazy(() =>
  import('@/features/triggers/pages/CreateTrigger').then((m) => ({
    default: m.CreateTrigger,
  }))
);
const EditTrigger = lazy(() =>
  import('@/features/triggers/pages/EditTrigger').then((m) => ({
    default: m.EditTrigger,
  }))
);
const Triggers = lazy(() =>
  import('@/features/triggers/pages/Triggers').then((m) => ({
    default: m.Triggers,
  }))
);
const CreateWorkflow = lazy(() =>
  import('@/features/workflows/pages/CreateWorkflow').then((m) => ({
    default: m.CreateWorkflow,
  }))
);
const Workflows = lazy(() =>
  import('@/features/workflows/pages/Workflows').then((m) => ({
    default: m.Workflows,
  }))
);
const Workflow = lazy(() =>
  import('@/features/workflows/pages/Workflow').then((m) => ({
    default: m.Workflow,
  }))
);
const WorkflowHistory = lazy(() =>
  import('@/features/workflows/pages/WorkflowHistory').then((m) => ({
    default: m.WorkflowHistory,
  }))
);
const WorkflowLogs = lazy(() =>
  import('@/features/workflows/pages/WorkflowLogs').then((m) => ({
    default: m.WorkflowLogs,
  }))
);
const ChatPage = lazy(() =>
  import('@/features/workflows/pages/Chat').then((m) => ({
    default: m.ChatPage,
  }))
);
const ObjectSchemas = lazy(() =>
  import('@/features/objects/pages/ObjectSchemas').then((m) => ({
    default: m.ObjectSchemas,
  }))
);
const CreateObjectSchema = lazy(() =>
  import('@/features/objects/pages/CreateObjectSchema').then((m) => ({
    default: m.CreateObjectSchema,
  }))
);
const EditObjectSchema = lazy(() =>
  import('@/features/objects/pages/EditObjectSchema').then((m) => ({
    default: m.EditObjectSchema,
  }))
);
const ManageInstances = lazy(() =>
  import('@/features/objects/pages/ManageInstances').then((m) => ({
    default: m.ManageInstances,
  }))
);
const CreateObjectInstance = lazy(() =>
  import('@/features/objects/pages/CreateObjectInstance').then((m) => ({
    default: m.CreateObjectInstance,
  }))
);
const EditObjectInstance = lazy(() =>
  import('@/features/objects/pages/EditObjectInstance').then((m) => ({
    default: m.EditObjectInstance,
  }))
);
const FilesPage = lazy(() =>
  import('@/features/files/pages/Files').then((m) => ({
    default: m.Files,
  }))
);
const AnalyticsUsage = lazy(() =>
  import('@/features/analytics/pages/Usage').then((m) => ({
    default: m.Usage,
  }))
);
const AnalyticsSystem = lazy(() =>
  import('@/features/analytics/pages/System').then((m) => ({
    default: m.System,
  }))
);
const AnalyticsRateLimits = lazy(() =>
  import('@/features/analytics/pages/RateLimits').then((m) => ({
    default: m.RateLimits,
  }))
);
const InvocationHistory = lazy(() =>
  import('@/features/invocation-history/pages/InvocationHistory').then((m) => ({
    default: m.InvocationHistory,
  }))
);
const ReportsListPage = lazy(() =>
  import('@/features/reports/pages/ReportsListPage').then((m) => ({
    default: m.ReportsListPage,
  }))
);
const ReportViewerPage = lazy(() =>
  import('@/features/reports/pages/ReportViewerPage').then((m) => ({
    default: m.ReportViewerPage,
  }))
);
const ReportExplorePage = lazy(() =>
  import('@/features/reports/pages/ReportExplorePage').then((m) => ({
    default: m.ReportExplorePage,
  }))
);
const ReportEditorPage = lazy(() =>
  import('@/features/reports/pages/ReportEditorPage').then((m) => ({
    default: m.ReportEditorPage,
  }))
);
const Settings = lazy(() =>
  import('@/features/settings/pages/Settings').then((m) => ({
    default: m.Settings,
  }))
);

// Loading component for Suspense
const PageLoader = () => (
  <div className="flex items-center justify-center h-full">
    <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
  </div>
);

// Basename comes from the <base href> the server injects at startup so the
// SPA works under any mount path (/ui, /ui/tenant-abc, etc.) without rebuilding.
const basename = new URL(document.baseURI).pathname.replace(/\/$/, '');

export const router = createBrowserRouter(
  [
    {
      path: '/',
      element: <Layout />,
      errorElement: (
        <Layout>
          <ErrorBoundary />
        </Layout>
      ),
      children: [
        {
          path: '/',
          index: true,
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Workflows />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Workflows />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/create',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <CreateWorkflow />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/:workflowId',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Workflow />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/:workflowId/history/:instanceId',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <WorkflowHistory />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/:workflowId/history/:instanceId/logs',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <WorkflowLogs />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/:workflowId/chat',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ChatPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/workflows/:workflowId/chat/:instanceId',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ChatPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/invocation-triggers',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Triggers />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/invocation-triggers/create',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <CreateTrigger />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/invocation-triggers/:triggerId',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <EditTrigger />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/connections',
          index: true,
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Connections />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/connections/:id',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Connection />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/connections/:id/create',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <CreateConnection />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/types',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ObjectSchemas />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/types/create',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <CreateObjectSchema />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/types/:id',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <EditObjectSchema />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/:typeName',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ManageInstances />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/:typeName/create',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <CreateObjectInstance />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/objects/:typeName/:id',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <EditObjectInstance />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/files',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <FilesPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/analytics',
          element: (
            <PrivateRoute>
              <Navigate to="/analytics/usage" replace />
            </PrivateRoute>
          ),
        },
        {
          path: '/analytics/usage',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <AnalyticsUsage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/analytics/system',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <AnalyticsSystem />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/analytics/rate-limits',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <AnalyticsRateLimits />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/invocation-history',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <InvocationHistory />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/reports',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ReportsListPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/reports/new',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ReportEditorPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/reports/:reportId',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ReportViewerPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/reports/:reportId/explore',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ReportExplorePage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/reports/:reportId/edit',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <ReportEditorPage />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/settings',
          element: (
            <PrivateRoute>
              <Navigate to="/settings/api-keys" replace />
            </PrivateRoute>
          ),
        },
        {
          path: '/settings/api-keys',
          element: (
            <PrivateRoute>
              <Suspense fallback={<PageLoader />}>
                <Settings />
              </Suspense>
            </PrivateRoute>
          ),
        },
        {
          path: '/login',
          element: <Login />,
        },
        {
          path: '*',
          element: <>404</>,
        },
      ],
    },
  ],
  { basename }
);
