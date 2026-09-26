import {
  useContext,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { useAuth } from 'react-oidc-context';
import { useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { toast } from 'sonner';
import { useToken } from '@/shared/hooks';
import { useAuthStore } from '@/shared/stores/authStore';
import { queryKeys } from '@/shared/queries/query-keys';
import { Button } from '@/shared/components/ui/button';
import { deliverSignal } from '../queries';
import { submitReportWorkflowAction } from '@/features/reports/queries';
import { InputSubmissionError } from '../utils/input-submission';
import {
  RetainedInputs,
  type InputIntentRequest,
} from '../utils/retained-inputs';

import {
  ManagedInputContext as Context,
  useManagedInputSubmissions,
} from '../hooks/useManagedInputSubmissions';

/** Nested timeline/report blocks share their page's owner instead of tying an
 * uncertain operation to a rendered action or a particular discovery result. */
export function ManagedInputScope({ children }: { children: ReactNode }) {
  const parent = useContext(Context);
  const tenant = useAuthStore((state) => state.orgId);
  const auth = useAuth();
  const principal = auth.user?.profile?.sub ?? '';
  return parent ? (
    children
  ) : (
    <InputOwner key={JSON.stringify([tenant, principal])} tenant={tenant}>
      {children}
    </InputOwner>
  );
}

function InputOwner({
  children,
  tenant,
}: {
  children: ReactNode;
  tenant: string;
}) {
  const [store] = useState(() => new RetainedInputs());
  const active = useRef(true);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const inputs = useSyncExternalStore(store.subscribe, store.snapshot);
  const token = useToken();
  const queryClient = useQueryClient();
  const retry = async (operationId: string) => {
    // Also guard saved callbacks from the previous tenant before React rerenders.
    if (!active.current || useAuthStore.getState().orgId !== tenant)
      return false;
    const input = store
      .snapshot()
      .find((item) => item.operationId === operationId);
    if (!input) return false;
    const confirmed = await store.send(
      operationId,
      async ({ request, operationId }) => {
        if (request.kind === 'execution') {
          return deliverSignal(token, request.instanceId, {
            requestId: request.requestId,
            operationId,
            payload: request.payload,
          });
        }
        try {
          return await submitReportWorkflowAction(token, {
            ...request,
            actionId: request.requestId,
            operationId,
          });
        } catch (error) {
          if (isAxiosError(error))
            throw new InputSubmissionError(
              error.response?.data?.message ?? error.message,
              error.response?.data?.code,
              error.response?.status
            );
          throw error;
        }
      }
    );
    if (active.current && useAuthStore.getState().orgId === tenant) {
      if (confirmed) toast.success('Response accepted');
      const request = input.request;
      // Refresh is advisory; a refresh failure cannot undo a confirmed receipt.
      void Promise.resolve(
        queryClient.invalidateQueries({
          queryKey:
            request.kind === 'execution'
              ? queryKeys.workflows.pendingInput(
                  request.workflowId,
                  request.instanceId
                )
              : queryKeys.reports.all,
        })
      ).catch(() => {});
    }
    return (
      confirmed && active.current && useAuthStore.getState().orgId === tenant
    );
  };
  const submit = (request: InputIntentRequest, label: string) => {
    if (!active.current || useAuthStore.getState().orgId !== tenant)
      return Promise.resolve(false);
    return retry(store.prepare(request, label).operationId);
  };
  return (
    <Context.Provider value={{ inputs, submit, retry }}>
      {children}
    </Context.Provider>
  );
}

export function InputRetryPanel({
  matches,
}: {
  matches: (request: InputIntentRequest) => boolean;
}) {
  const { inputs, retry } = useManagedInputSubmissions();
  const retained = inputs.filter(
    (input) => matches(input.request) && input.state !== 'accepted'
  );
  if (!retained.length) return null;
  return (
    <section
      aria-label="Response submissions"
      className="report-print-hidden my-3 space-y-3"
    >
      {retained.map((input) => (
        <div key={input.operationId} className="rounded-lg border p-3 text-sm">
          <p className="font-medium">{input.label}</p>
          <p role={input.error ? 'alert' : 'status'}>
            {input.state === 'submitting'
              ? 'Confirming response…'
              : input.state === 'rejected'
                ? `Response could not be confirmed: ${input.error}`
                : `Acceptance is unconfirmed. ${input.error ?? ''}`}
          </p>
          <details className="my-2">
            <summary>Submitted response</summary>
            <p className="break-all text-muted-foreground">
              Execution: {input.request.instanceId}
            </p>
            <pre className="overflow-auto whitespace-pre-wrap">
              {JSON.stringify(input.request.payload, null, 2)}
            </pre>
            {input.request.kind === 'report' && (
              <>
                <p className="text-muted-foreground">Filters at submission</p>
                <pre className="overflow-auto whitespace-pre-wrap">
                  {JSON.stringify(
                    {
                      report: input.request.filters,
                      block: input.request.blockFilters,
                    },
                    null,
                    2
                  )}
                </pre>
              </>
            )}
          </details>
          <Button
            size="sm"
            variant="secondary"
            disabled={input.state === 'submitting'}
            onClick={() => void retry(input.operationId)}
          >
            Retry original response
          </Button>
        </div>
      ))}
    </section>
  );
}
