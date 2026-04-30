import { useParams, useSearchParams } from 'react-router';
import { useReactFlow } from '@xyflow/react';
import { toast } from 'sonner';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { WorkflowEditor } from '@/features/workflows/components/WorkflowEditor';
import { WorkflowActionsForm } from '@/features/workflows/pages/Workflow/WorkflowActionsForm';
import { Loader } from '@/shared/components/loader.tsx';
import { composeExecutionGraph } from '@/features/workflows/components/WorkflowEditor/CustomNodes/utils.tsx';
import { validateWorkflowStructure } from '@/features/workflows/utils/graph-validation';
import '@xyflow/react/dist/base.css';
import { queryClient } from '@/main.tsx';
import { slugify } from '@/shared/utils/string-utils';
import { useEffect, useState, useRef, useMemo, useCallback } from 'react';
import * as form from '@/features/workflows/components/WorkflowEditor/NodeForm/NodeFormItem';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { DebugStepInspector } from '@/features/workflows/components/DebugStepInspector';

import { WorkflowExecuteDialog } from '@/features/workflows/components/WorkflowExecuteDialog';
import {
  getWorkflowVersions,
  getWorkflowWorkflow,
  scheduleWorkflow,
  setCurrentVersion,
  updateWorkflow,
  getWorkflowInstance,
  getStepEvents,
  toggleTrackEvents,
  resumeInstance,
  stopInstance,
  WorkflowVersionInfoDto,
} from '@/features/workflows/queries';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';

import { useNavigationBlockerStore } from '@/shared/stores/navigationBlockerStore';
import { UnsavedChangesDialog } from '@/shared/components/unsaved-changes-dialog';
import { ValidationPanel } from '@/features/workflows/components/ValidationPanel';
import { useValidationStore } from '@/features/workflows/stores/validationStore';
import {
  convertClientErrors,
  convertClientWarnings,
  convertServerErrors,
} from '@/features/workflows/utils/validation-helpers';

import {
  parseSchema,
  buildSchemaFromFields,
} from '@/features/workflows/utils/schema';

function normalizeNodeDataForStagingComparison(
  data?: Partial<form.SchemaType>
): Partial<form.SchemaType> {
  return {
    ...form.initialValues,
    ...(data ? JSON.parse(JSON.stringify(data)) : {}),
  };
}

export function Workflow() {
  const { workflowId } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const [selectedVersion, setSelectedVersion] = useState<number | undefined>(
    undefined
  );
  const [queryVersion, setQueryVersion] = useState<number | undefined>(
    undefined
  );
  const [versionsWithDebug, setVersionsWithDebug] = useState<
    WorkflowVersionInfoDto[]
  >([]);

  const [executeDialogOpen, setExecuteDialogOpen] = useState(false);
  const [executeError, setExecuteError] = useState<string | null>(null);

  const [isUnsavedChangesDialogOpen, setIsUnsavedChangesDialogOpen] =
    useState(false);
  const pendingUnsavedChangesActionRef = useRef<(() => void) | null>(null);
  // Track if we're switching versions (for showing inline loading instead of full page loader)
  const [isSwitchingVersion, setIsSwitchingVersion] = useState(false);

  // Staged node changes - preserved until global save
  const [stagedNodeChanges, setStagedNodeChanges] = useState<
    Record<string, form.SchemaType>
  >({});

  // Staged workflow changes (name, description, variables, schemas, timeout) - preserved until global save
  const [stagedWorkflowChanges, setStagedWorkflowChanges] = useState<{
    name?: string;
    description?: string;
    variables?: Array<{ name: string; value: string; type: string }>;
    inputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
      defaultValue?: any;
    }>;
    outputSchemaFields?: Array<{
      name: string;
      type: string;
      required: boolean;
      description: string;
      defaultValue?: any;
    }>;
    executionTimeoutSeconds?: number;
    rateLimitBudgetMs?: number;
  }>({});

  // Sync staged node IDs to workflow store for visual highlighting
  useEffect(() => {
    const stagedIds = new Set(Object.keys(stagedNodeChanges));
    useWorkflowStore.getState().setStagedNodeIds(stagedIds);
  }, [stagedNodeChanges]);

  // Reactively subscribe to isDirty state
  const isDirty = useWorkflowStore((state) => state.isDirty);
  const isStructurallyDirty = useWorkflowStore(
    (state) => state.isStructurallyDirty
  );

  // Compute if there are any unsaved changes (workflow dirty or staged node/workflow changes)
  // Ignore dirty state during version switching to prevent button flickering
  const hasUnsavedChanges = useMemo(() => {
    // During version switch, we don't have unsaved changes (they were intentionally discarded)
    if (isSwitchingVersion) {
      return false;
    }
    const hasWorkflowChanges = Object.keys(stagedWorkflowChanges).length > 0;
    return (
      isDirty || Object.keys(stagedNodeChanges).length > 0 || hasWorkflowChanges
    );
  }, [isDirty, stagedNodeChanges, stagedWorkflowChanges, isSwitchingVersion]);

  // Structural changes (add/remove/edit nodes or edges) block execution.
  // Position-only changes (dragging, auto-layout) do not.
  const hasStructuralUnsavedChanges = useMemo(() => {
    if (isSwitchingVersion) {
      return false;
    }
    return (
      isStructurallyDirty ||
      Object.keys(stagedNodeChanges).length > 0 ||
      Object.keys(stagedWorkflowChanges).length > 0
    );
  }, [
    isStructurallyDirty,
    stagedNodeChanges,
    stagedWorkflowChanges,
    isSwitchingVersion,
  ]);

  // Helper to check for unsaved changes before performing a destructive action (version change, import)
  const confirmIfUnsavedChanges = useCallback(
    (action: () => void) => {
      if (hasUnsavedChanges) {
        pendingUnsavedChangesActionRef.current = action;
        setIsUnsavedChangesDialogOpen(true);
      } else {
        action();
      }
    },
    [hasUnsavedChanges]
  );

  const handleConfirmUnsavedChanges = useCallback(() => {
    const pendingAction = pendingUnsavedChangesActionRef.current;
    pendingUnsavedChangesActionRef.current = null;
    setIsUnsavedChangesDialogOpen(false);

    if (pendingAction) {
      pendingAction();
    }
  }, []);

  const handleCancelUnsavedChanges = useCallback(() => {
    pendingUnsavedChangesActionRef.current = null;
    setIsUnsavedChangesDialogOpen(false);
  }, []);

  // Use the global navigation blocker store
  const setBlocker = useNavigationBlockerStore((state) => state.setBlocker);

  // Register/unregister the blocker based on hasUnsavedChanges
  useEffect(() => {
    setBlocker(hasUnsavedChanges, () => {
      setStagedNodeChanges({});
      setStagedWorkflowChanges({});
      useWorkflowStore.getState().clearDirtyFlag();
    });

    return () => {
      setBlocker(false);
    };
  }, [hasUnsavedChanges, setBlocker]);

  const { getViewport } = useReactFlow();

  // Fetch available versions
  const { data: versionsResponse } = useCustomQuery({
    queryKey: queryKeys.workflows.versions(workflowId ?? ''),
    queryFn: (token: string) => getWorkflowVersions(token, workflowId!),
    enabled: !!workflowId,
    placeholderData: { data: [], message: '', success: true } as any,
  });

  // Extract versions from wrapped response
  const versions = useMemo(
    () => (versionsResponse as any)?.data || [],
    [versionsResponse]
  );

  // Use queryVersion for the actual query to control when it changes
  const { data: response, isFetching } = useCustomQuery({
    queryKey: queryKeys.workflows.workflow(workflowId ?? '', queryVersion),
    queryFn: (token: string) =>
      getWorkflowWorkflow(token, workflowId!, queryVersion),
    enabled: !!workflowId,
    placeholderData: {
      data: {
        nodes: [],
        edges: [],
      },
      message: '',
      success: true,
    } as any,
  });

  // Extract workflow data from wrapped response
  const data = useMemo(
    () => (response as any)?.data || { nodes: [], edges: [] },
    [response]
  );
  const hasInputSchema = useMemo(
    () => ((data as any)?.inputSchemaFields ?? []).length > 0,
    [data]
  );
  const workflowInputSchema = useMemo(() => {
    const fields = (data as any)?.inputSchemaFields ?? [];
    return fields.length > 0 ? buildSchemaFromFields(fields) : {};
  }, [data]);

  // Reset selectedVersion and invalidate queries when workflowId changes
  useEffect(() => {
    // Reset execution state when switching workflows
    useExecutionStore.getState().resetExecution();

    // Reset workflow state when switching workflows
    useWorkflowStore.getState().resetState();

    // Clear validation problems from previous workflow
    useValidationStore.getState().clearMessages();

    // Clear staged node and workflow changes
    setStagedNodeChanges({});
    setStagedWorkflowChanges({});

    setSelectedVersion(undefined);
    setQueryVersion(undefined);

    // Batch invalidate queries to ensure fresh data is fetched for the new workflow
    Promise.all([
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.workflow(workflowId ?? ''),
      }),
      queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.versions(workflowId ?? ''),
      }),
    ]);
  }, [workflowId]);

  // Reset execution state when leaving the workflow page
  useEffect(() => {
    return () => {
      useExecutionStore.getState().resetExecution();
    };
  }, []);

  // Clear dirty flag after workflow data is loaded and rendered
  useEffect(() => {
    if (!isFetching) {
      // Use a small timeout to ensure React Flow has finished initializing
      const timer = setTimeout(() => {
        useWorkflowStore.getState().clearDirtyFlag();
      }, 100);
      return () => clearTimeout(timer);
    }
  }, [isFetching, workflowId, queryVersion]);

  // Set the current version as the default selected version when data is loaded
  useEffect(() => {
    if (data.currentVersionNumber && selectedVersion === undefined) {
      setSelectedVersion(data.currentVersionNumber);
      setQueryVersion(data.currentVersionNumber);
    }
  }, [data.currentVersionNumber, selectedVersion]);

  // Set versions with debug mode from API response
  useEffect(() => {
    if (!versions || versions.length === 0) {
      setVersionsWithDebug([]);
      return;
    }

    // API now provides trackEvents directly in WorkflowVersionInfoDto
    setVersionsWithDebug(versions);
  }, [versions]);

  // Reset isSwitchingVersion when fetching completes and dirty flag is cleared
  // Use the same delay as the dirty flag clearing to prevent button flickering
  useEffect(() => {
    if (!isFetching && isSwitchingVersion) {
      const timer = setTimeout(() => {
        setIsSwitchingVersion(false);
      }, 150); // Slightly longer than the 100ms dirty flag clear timeout
      return () => clearTimeout(timer);
    }
  }, [isFetching, isSwitchingVersion]);

  // Internal version change handler (bypasses unsaved changes check)
  const performVersionChange = useCallback(
    (version: number | undefined) => {
      // Clear dirty flag when switching versions
      useWorkflowStore.getState().clearDirtyFlag();
      // Clear staged node and workflow changes when switching versions
      setStagedNodeChanges({});
      setStagedWorkflowChanges({});

      // Mark that we're switching versions to show inline loading
      setIsSwitchingVersion(true);

      // Remove cached data for the target version to force a fresh fetch
      // This ensures we always get the correct workflow data for each version
      queryClient.removeQueries({
        queryKey: queryKeys.workflows.workflow(workflowId ?? '', version),
        exact: true,
      });

      setSelectedVersion(version);
      setQueryVersion(version); // This will trigger a new query
    },
    [workflowId]
  );

  // Sync queryVersion with selectedVersion only when user explicitly changes version
  // Not when we programmatically update it after save
  const handleVersionChange = useCallback(
    (version: number | undefined) => {
      confirmIfUnsavedChanges(() => performVersionChange(version));
    },
    [confirmIfUnsavedChanges, performVersionChange]
  );

  // Set page title with workflow name
  usePageTitle(data.name ? `Workflows - ${data.name}` : 'Edit Workflow');

  // Track the staged workflow changes at mutation time to update cache correctly
  const stagedWorkflowChangesRef = useRef(stagedWorkflowChanges);
  useEffect(() => {
    stagedWorkflowChangesRef.current = stagedWorkflowChanges;
  }, [stagedWorkflowChanges]);

  const updateMutation = useCustomMutation({
    mutationFn: updateWorkflow,
    suppressValidationToasts: true, // Use validation panel instead of toasts
    onSuccess: async (response: any) => {
      // Response structure: { message, workflowId, success, timestamp, version: "28" }
      const newVersionNumber = response?.version
        ? parseInt(response.version, 10)
        : undefined;

      // Clear the dirty flag in the store since we've successfully saved
      useWorkflowStore.getState().clearDirtyFlag();

      // Capture staged changes before they get cleared
      const savedStagedChanges = stagedWorkflowChangesRef.current;

      if (newVersionNumber) {
        // Update selectedVersion to show the new version in the selectbox
        setSelectedVersion(newVersionNumber);

        // Get the current cached data
        const currentCachedData = queryClient.getQueryData(
          queryKeys.workflows.workflow(workflowId ?? '', queryVersion)
        ) as any;

        // Update the cached data for the new version
        // Note: Do NOT update currentVersionNumber here - saving a new version does not
        // make it the active version. currentVersionNumber should only change when
        // explicitly activating a version via setCurrentVersion API.
        if (currentCachedData) {
          const updatedData = {
            ...currentCachedData,
            data: {
              ...currentCachedData.data,
              // Update name and description if they were changed
              ...(savedStagedChanges.name !== undefined && {
                name: savedStagedChanges.name,
              }),
              ...(savedStagedChanges.description !== undefined && {
                description: savedStagedChanges.description,
              }),
              ...(savedStagedChanges.variables !== undefined && {
                variables: savedStagedChanges.variables,
              }),
              ...(savedStagedChanges.inputSchemaFields !== undefined && {
                inputSchemaFields: savedStagedChanges.inputSchemaFields,
              }),
              ...(savedStagedChanges.outputSchemaFields !== undefined && {
                outputSchemaFields: savedStagedChanges.outputSchemaFields,
              }),
              ...(savedStagedChanges.executionTimeoutSeconds !== undefined && {
                executionTimeoutSeconds:
                  savedStagedChanges.executionTimeoutSeconds,
              }),
              ...(savedStagedChanges.rateLimitBudgetMs !== undefined && {
                rateLimitBudgetMs: savedStagedChanges.rateLimitBudgetMs,
              }),
              // Keep the original currentVersionNumber (the active version)
            },
          };

          // Update the current query's cached data
          queryClient.setQueryData(
            queryKeys.workflows.workflow(workflowId ?? '', queryVersion),
            updatedData
          );

          // Also set the cache for the new version number
          queryClient.setQueryData(
            queryKeys.workflows.workflow(workflowId ?? '', newVersionNumber),
            updatedData
          );
        }
      }

      // Fetch new versions in background to update the version list
      await queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.versions(workflowId ?? ''),
      });

      // Open the versions tab to show the newly created version
      useValidationStore.getState().setActiveTab('versions');
    },
  });

  const {
    startExecution,
    executingInstanceId,
    debugMode: executionDebugMode,
    isSuspended,
    updateNodeStatus,
    updateInstanceStatus,
    setSuspended,
    setStepDebugData,
    resetExecution,
  } = useExecutionStore();

  const scheduleMutation = useCustomMutation({
    mutationFn: (
      token: string,
      {
        workflowId,
        inputs,
        version,
        debug,
      }: {
        workflowId: string;
        trackEvents: boolean;
        inputs?: Record<string, any>;
        version?: number;
        debug?: boolean;
      }
    ) => scheduleWorkflow(token, workflowId, inputs, version, debug),
    onSuccess: (
      response: any,
      variables: {
        workflowId: string;
        trackEvents: boolean;
        inputs?: Record<string, any>;
        version?: number;
        debug?: boolean;
      }
    ) => {
      // Response is wrapped: { data: ExecuteWorkflowResponse, message, success }
      // ExecuteWorkflowResponse: { instanceId, status }
      const instanceId = response?.data?.instanceId;

      if (instanceId && variables.workflowId) {
        // Start execution visualization (debugMode enables step-event polling)
        startExecution(
          instanceId,
          variables.workflowId,
          variables.trackEvents,
          variables.debug
        );
      }
    },
  });

  // Poll instance data for execution visualization
  const { data: executionInstanceData, refetch: refetchInstanceData } =
    useCustomQuery({
      queryKey: queryKeys.workflows.instance(
        workflowId ?? '',
        executingInstanceId ?? ''
      ),
      queryFn: (token: string) =>
        getWorkflowInstance(token, workflowId!, executingInstanceId!),
      enabled: !!executingInstanceId,
      refetchInterval: (query: any) => {
        const status = query.state.data?.status;
        const isActive =
          status &&
          [
            ExecutionStatus.Queued,
            ExecutionStatus.Compiling,
            ExecutionStatus.Running,
          ].includes(status);
        // Also keep polling (slower) when suspended to detect resume completion
        return isActive
          ? 2000
          : status === ExecutionStatus.Suspended
            ? 3000
            : false;
      },
    });

  // Poll step events (only in debug/trackEvents mode)
  const { data: executionStepEventsData, refetch: refetchStepEvents } =
    useCustomQuery({
      queryKey: queryKeys.workflows.stepEvents(
        workflowId,
        executingInstanceId ?? undefined
      ),
      queryFn: (token: string) =>
        getStepEvents(token, workflowId!, executingInstanceId!),
      enabled: !!executingInstanceId && executionDebugMode,
      refetchInterval: () => {
        // Keep polling while instance is active or suspended
        const status = executionInstanceData?.status;
        const isActive =
          status &&
          [
            ExecutionStatus.Queued,
            ExecutionStatus.Compiling,
            ExecutionStatus.Running,
          ].includes(status);
        // Poll more frequently (500ms) in debug mode to catch intermediate states
        return isActive || status === ExecutionStatus.Suspended ? 500 : false;
      },
    });

  // Initialize all nodes with queued status when execution starts
  useEffect(() => {
    if (!executingInstanceId) return;

    const { nodes } = useWorkflowStore.getState();

    // Set all nodes (except start and create nodes) to queued status initially
    nodes.forEach((node) => {
      if (node.id !== 'start' && node.type !== 'CreateNode') {
        updateNodeStatus(node.id, {
          status: ExecutionStatus.Queued,
        });
      }
    });
  }, [executingInstanceId, updateNodeStatus]);

  // Reattach to a suspended execution when navigating with ?attachInstance=<id>
  useEffect(() => {
    const attachInstanceId = searchParams.get('attachInstance');
    if (!attachInstanceId || !workflowId) return;
    // Only attach if we're not already tracking an execution
    if (executingInstanceId) return;

    // Remove the query param so it doesn't re-trigger
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete('attachInstance');
        return next;
      },
      { replace: true }
    );

    // Attach to the instance — enable step event polling (debugMode=true) and mark as debug execution
    startExecution(attachInstanceId, workflowId, true, true);
  }, [
    searchParams,
    workflowId,
    executingInstanceId,
    startExecution,
    setSearchParams,
  ]);

  // Track the last instance ID we did a final refetch for
  const lastFinalRefetchInstanceId = useRef<string | null>(null);
  const finalRefetchRetryCountRef = useRef<number>(0);

  // Helper to check if all step events have completed (have both start and end events)
  const checkAllStepsCompleted = useCallback((eventsData: any): boolean => {
    const events = eventsData?.data?.events || [];
    if (events.length === 0) return true; // No events means nothing to check

    const startEvents = new Set<string>();
    const endEvents = new Set<string>();

    for (const event of events) {
      if (event.subtype === 'step_debug_start' && event.payload?.step_id) {
        startEvents.add(event.payload.step_id);
      } else if (event.subtype === 'step_debug_end' && event.payload?.step_id) {
        endEvents.add(event.payload.step_id);
      }
    }

    // All steps that started should have ended
    for (const stepId of startEvents) {
      if (!endEvents.has(stepId)) {
        return false;
      }
    }
    return true;
  }, []);

  // Do a final refetch when execution reaches terminal state
  useEffect(() => {
    if (!executionInstanceData?.status || !executingInstanceId) return;

    const terminalStates = [
      ExecutionStatus.Completed,
      ExecutionStatus.Failed,
      ExecutionStatus.Timeout,
      ExecutionStatus.Cancelled,
    ];

    // If execution just finished and we haven't done final refetch for this instance yet
    if (
      terminalStates.includes(executionInstanceData.status) &&
      lastFinalRefetchInstanceId.current !== executingInstanceId
    ) {
      lastFinalRefetchInstanceId.current = executingInstanceId;
      finalRefetchRetryCountRef.current = 0;

      // Recursive function to retry fetching step events until all are completed
      let cancelled = false;
      const fetchWithRetry = async (retryCount: number, maxRetries: number) => {
        if (cancelled) return;
        await refetchInstanceData();

        if (cancelled || !executionDebugMode) return;
        const result = await refetchStepEvents();
        const allCompleted = checkAllStepsCompleted(result.data);

        if (!cancelled && !allCompleted && retryCount < maxRetries) {
          finalRefetchRetryCountRef.current = retryCount + 1;
          setTimeout(() => fetchWithRetry(retryCount + 1, maxRetries), 500);
        }
      };

      // Delay initial fetch to allow backend to finalize data, then retry if needed
      const timer = setTimeout(() => fetchWithRetry(0, 5), 300);

      return () => {
        cancelled = true;
        clearTimeout(timer);
      };
    }
  }, [
    executingInstanceId,
    executionInstanceData?.status,
    executionDebugMode,
    refetchInstanceData,
    refetchStepEvents,
    checkAllStepsCompleted,
  ]);

  // Update node statuses based on execution data
  useEffect(() => {
    // Don't update if execution was cleared (executingInstanceId is null)
    if (!executingInstanceId || !executionInstanceData) return;

    // Verify this data is for the current execution instance
    // This prevents processing stale data from previous executions
    if (executionInstanceData.id !== executingInstanceId) {
      return;
    }

    // Update instance status in store
    if (executionInstanceData.status) {
      updateInstanceStatus(executionInstanceData.status);
    }

    // Detect suspended state (breakpoint hit in debug execution)
    // NOTE: this block must NOT early-return — the node status mapping below must always run.
    if (
      executionInstanceData.status === ExecutionStatus.Suspended &&
      !justResumedRef.current
    ) {
      // Look for the LATEST breakpoint_hit event
      const events = executionStepEventsData?.data?.events;
      if (events) {
        // Events are returned newest-first from the API — take the first breakpoint_hit (current one)
        const latestBreakpoint =
          events.find(
            (e: any) =>
              e.eventType === 'custom' && e.subtype === 'breakpoint_hit'
          ) ?? null;

        if (
          latestBreakpoint?.payload &&
          latestBreakpoint.payload.step_id !==
            lastProcessedBreakpointRef.current
        ) {
          lastProcessedBreakpointRef.current = latestBreakpoint.payload.step_id;
          setSuspended(true, {
            stepId: latestBreakpoint.payload.step_id,
            stepName: latestBreakpoint.payload.step_name,
            stepType: latestBreakpoint.payload.step_type,
            inputs: latestBreakpoint.payload.inputs ?? null,
            stepsContext: latestBreakpoint.payload.steps_context || {},
          });
        } else if (!latestBreakpoint?.payload && !isSuspended) {
          // Step events don't have breakpoint_hit yet — force immediate refetch
          setSuspended(true, null);
          refetchStepEvents();
        }
      } else if (!isSuspended) {
        // No step events data yet — force immediate refetch
        setSuspended(true, null);
        refetchStepEvents();
      }
    } else if (executionInstanceData.status !== ExecutionStatus.Suspended) {
      // Status is not suspended — clear the justResumed guard and suspended state
      if (justResumedRef.current) {
        justResumedRef.current = false;
      }
      if (isSuspended) {
        setSuspended(false, null);
      }
    }

    // Map steps to nodes
    if (executionDebugMode && executionStepEventsData?.data?.events) {
      // Debug mode: process raw events by pairing start and end events
      const rawEvents = executionStepEventsData.data.events;

      // Sort events by id to process in order
      const sortedEvents = [...rawEvents].sort((a: any, b: any) => a.id - b.id);

      // Track start events and build paired step data
      const startEvents = new Map<string, any>();
      const processedSteps = new Map<
        string,
        {
          status: ExecutionStatus;
          startedAt?: string;
          executionTime?: number;
          error?: string;
        }
      >();

      for (const event of sortedEvents) {
        if (event.subtype === 'step_debug_start' && event.payload?.step_id) {
          // Store start event
          startEvents.set(event.payload.step_id, event);

          // Mark step as Running
          const timestamp = event.payload.timestamp_ms
            ? new Date(event.payload.timestamp_ms).toISOString()
            : undefined;
          processedSteps.set(event.payload.step_id, {
            status: ExecutionStatus.Running,
            startedAt: timestamp,
          });

          // Store step inputs for debug inspector
          if (event.payload.inputs !== undefined) {
            setStepDebugData(event.payload.step_id, {
              inputs: event.payload.inputs,
            });
          }
        } else if (
          event.subtype === 'step_debug_end' &&
          event.payload?.step_id
        ) {
          // Find matching start event
          const startEvent = startEvents.get(event.payload.step_id);
          const startTimestamp = startEvent?.payload?.timestamp_ms
            ? new Date(startEvent.payload.timestamp_ms).toISOString()
            : undefined;

          // Determine status from end event payload
          const hasError = !!event.payload.error;
          const status = hasError
            ? ExecutionStatus.Failed
            : ExecutionStatus.Completed;

          processedSteps.set(event.payload.step_id, {
            status,
            startedAt: startTimestamp,
            executionTime: event.payload.duration_ms || undefined,
            error: event.payload.error || undefined,
          });

          // Store step outputs for debug inspector
          setStepDebugData(event.payload.step_id, {
            outputs: event.payload.outputs,
            durationMs: event.payload.duration_ms,
            error: event.payload.error || undefined,
          });
        }
      }

      // Update node statuses for all processed steps
      const executedStepIds = new Set(processedSteps.keys());
      for (const [stepId, stepData] of processedSteps) {
        updateNodeStatus(stepId, stepData);
      }

      // After mapping all step events, apply Suspended highlight to the current breakpoint step.
      // This must run AFTER the loop above so it overrides the step_debug_start "Running" status.
      const currentBp = useExecutionStore.getState().breakpointHit;
      if (isSuspended && currentBp?.stepId) {
        updateNodeStatus(currentBp.stepId, {
          status: ExecutionStatus.Suspended,
        });
      }

      // If execution is finished, clear status for nodes that weren't executed (skipped)
      const isExecutionTerminal = [
        ExecutionStatus.Completed,
        ExecutionStatus.Failed,
        ExecutionStatus.Timeout,
        ExecutionStatus.Cancelled,
      ].includes(executionInstanceData.status);

      if (isExecutionTerminal) {
        const { nodes } = useWorkflowStore.getState();
        const currentStore = useExecutionStore.getState();
        const skippedNodeIds: string[] = [];

        nodes.forEach((node) => {
          if (node.id === 'start' || node.type === 'CreateNode') return;

          const currentStatus = currentStore.nodeExecutionStatus.get(node.id);
          if (
            currentStatus?.status === ExecutionStatus.Queued &&
            !executedStepIds.has(node.id)
          ) {
            skippedNodeIds.push(node.id);
          }
        });

        if (skippedNodeIds.length > 0) {
          const newMap = new Map(currentStore.nodeExecutionStatus);
          skippedNodeIds.forEach((nodeId) => newMap.delete(nodeId));

          useExecutionStore.setState({
            nodeExecutionStatus: newMap,
            statusVersion: currentStore.statusVersion + 1,
          });
        }
      }
    } else if (
      executionInstanceData.steps &&
      executionInstanceData.steps.length > 0
    ) {
      // Non-debug mode with step details available
      const isExecutionTerminal = [
        ExecutionStatus.Completed,
        ExecutionStatus.Failed,
        ExecutionStatus.Timeout,
        ExecutionStatus.Cancelled,
      ].includes(executionInstanceData.status);

      // Get all step IDs that were actually executed
      const executedStepIds = new Set(
        executionInstanceData.steps.map((step: any) => step.id)
      );

      executionInstanceData.steps.forEach((step: any) => {
        const status = step.finished
          ? ExecutionStatus.Completed
          : step.started
            ? ExecutionStatus.Running
            : isExecutionTerminal
              ? executionInstanceData.status
              : ExecutionStatus.Queued;

        updateNodeStatus(step.id, {
          status,
          startedAt: step.started || undefined,
          completedAt: step.finished || undefined,
          executionTime: step.executionTime || undefined,
        });
      });

      // If execution is finished, clear status for nodes that weren't executed (skipped)
      if (isExecutionTerminal) {
        const { nodes } = useWorkflowStore.getState();
        const currentStore = useExecutionStore.getState();
        const skippedNodeIds: string[] = [];

        // Find all nodes that are queued but weren't executed
        nodes.forEach((node) => {
          // Skip start and create nodes
          if (node.id === 'start' || node.type === 'CreateNode') return;

          // If node has queued status but wasn't in the executed steps, it was skipped
          const currentStatus = currentStore.nodeExecutionStatus.get(node.id);
          if (
            currentStatus?.status === ExecutionStatus.Queued &&
            !executedStepIds.has(node.id)
          ) {
            skippedNodeIds.push(node.id);
          }
        });

        // Clear status for all skipped nodes in one update
        if (skippedNodeIds.length > 0) {
          const newMap = new Map(currentStore.nodeExecutionStatus);
          skippedNodeIds.forEach((nodeId) => newMap.delete(nodeId));

          useExecutionStore.setState({
            nodeExecutionStatus: newMap,
            statusVersion: currentStore.statusVersion + 1,
          });
        }
      }
    } else {
      // Non-debug mode without step details: apply overall status to all nodes
      const { nodes } = useWorkflowStore.getState();

      // Update all nodes except the start node
      nodes.forEach((node) => {
        if (node.id !== 'start' && node.type !== 'CreateNode') {
          updateNodeStatus(node.id, {
            status: executionInstanceData.status,
          });
        }
      });
    }
  }, [
    executingInstanceId,
    executionInstanceData,
    executionStepEventsData,
    executionDebugMode,
    isSuspended,
    updateNodeStatus,
    updateInstanceStatus,
    setSuspended,
    setStepDebugData,
    refetchStepEvents,
  ]);

  const setCurrentVersionMutation = useCustomMutation({
    mutationFn: setCurrentVersion,
    onSuccess: async () => {
      // Remove all cached workflow versions for this workflow to ensure fresh data on return
      // Using removeQueries with exact: false matches all cache entries starting with this key prefix
      queryClient.removeQueries({
        queryKey: queryKeys.workflows.workflow(workflowId ?? ''),
        exact: false,
      });

      // Update versions metadata
      await queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.versions(workflowId ?? ''),
      });

      // Invalidate the main workflows list to update the displayed version number
      await queryClient.invalidateQueries({
        queryKey: queryKeys.workflows.all,
      });
    },
  });

  const toggleTrackEventsMutation = useCustomMutation({
    mutationFn: toggleTrackEvents,
    onSuccess: async (
      response: any,
      variables: { workflowId: string; version: number; trackEvents: boolean }
    ) => {
      const newTrackEvents = response?.data?.trackEvents;
      const toggledVersion = variables.version;

      toast.info(
        `Event tracking ${newTrackEvents ? 'enabled' : 'disabled'} for version ${toggledVersion}`
      );

      // Update the current query's cached data if we toggled the selected version
      if (queryVersion === toggledVersion) {
        queryClient.setQueryData(
          queryKeys.workflows.workflow(workflowId ?? '', queryVersion),
          (oldData: any) => {
            if (!oldData) return oldData;
            return {
              ...oldData,
              data: {
                ...oldData.data,
                trackEvents: newTrackEvents,
              },
            };
          }
        );
      }

      // Update the versions list to reflect the track events change
      setVersionsWithDebug((prevVersions) =>
        prevVersions.map((v) =>
          v.versionNumber === toggledVersion
            ? { ...v, trackEvents: newTrackEvents }
            : v
        )
      );
    },
  });

  // Handlers for staged node changes
  const handleStagedNodeChange = useCallback(
    (nodeId: string, nodeData: form.SchemaType) => {
      const currentNode = useWorkflowStore
        .getState()
        .nodes.find((n) => n.id === nodeId);
      const normalizedCurrentData = normalizeNodeDataForStagingComparison(
        currentNode?.data as form.SchemaType | undefined
      );
      const normalizedIncomingData =
        normalizeNodeDataForStagingComparison(nodeData);
      const hasMeaningfulDiff =
        JSON.stringify(normalizedCurrentData) !==
        JSON.stringify(normalizedIncomingData);

      setStagedNodeChanges((prev) => {
        if (!hasMeaningfulDiff) {
          if (!prev[nodeId]) {
            return prev;
          }
          const next = { ...prev };
          delete next[nodeId];
          return next;
        }

        // Compare with existing staged data to prevent infinite loops
        const existingData = prev[nodeId];
        if (existingData) {
          const existingJson = JSON.stringify(existingData);
          const newJson = JSON.stringify(nodeData);
          if (existingJson === newJson) {
            // No change, return same reference to prevent re-render
            return prev;
          }
        }
        return {
          ...prev,
          [nodeId]: nodeData,
        };
      });
    },
    []
  );

  const handleResetNodeChanges = useCallback((nodeId: string) => {
    setStagedNodeChanges((prev) => {
      const next = { ...prev };
      delete next[nodeId];
      return next;
    });
  }, []);

  // Clean up staged changes when nodes are removed
  const workflowNodesLength = useWorkflowStore((state) => state.nodes.length);
  useEffect(() => {
    const currentNodeIds = new Set(
      useWorkflowStore.getState().nodes.map((n) => n.id)
    );

    setStagedNodeChanges((prev) => {
      const stagedIds = Object.keys(prev);
      const removedIds = stagedIds.filter((id) => !currentNodeIds.has(id));

      if (removedIds.length === 0) return prev;

      const next = { ...prev };
      removedIds.forEach((id) => delete next[id]);
      return next;
    });
  }, [workflowNodesLength]);

  const handleSubmit = async ({ name }: Record<string, any>) => {
    // Get the current state from Zustand store
    const { nodes, edges, isDirty, syncFromReactFlow } =
      useWorkflowStore.getState();

    // Check if the name has changed
    const nameChanged = name !== data.name;

    // Check if there are staged node changes
    const hasStagedNodeChanges = Object.keys(stagedNodeChanges).length > 0;

    // Check if there are staged workflow changes (metadata: variables, schemas, description)
    const hasStagedWorkflowChanges =
      Object.keys(stagedWorkflowChanges).length > 0;

    // Check if workflow has changed (dirty flag or staged node changes)
    const hasWorkflowChanges = isDirty || hasStagedNodeChanges;

    // If nothing has changed, just show a message and return
    if (!hasWorkflowChanges && !nameChanged && !hasStagedWorkflowChanges) {
      toast.info('No changes to save');
      return;
    }

    // Apply staged node changes to the workflow store before saving
    if (hasStagedNodeChanges) {
      let hasActualStagedUpdates = false;
      const updatedNodes = nodes.map((node) => {
        const stagedNodeData = stagedNodeChanges[node.id];
        if (stagedNodeData) {
          const normalizedCurrentData = normalizeNodeDataForStagingComparison(
            node.data as form.SchemaType
          );
          const normalizedStagedData =
            normalizeNodeDataForStagingComparison(stagedNodeData);
          const hasMeaningfulDiff =
            JSON.stringify(normalizedCurrentData) !==
            JSON.stringify(normalizedStagedData);

          if (!hasMeaningfulDiff) {
            return node;
          }

          hasActualStagedUpdates = true;
          return {
            ...node,
            data: {
              ...node.data,
              ...stagedNodeData,
            },
          };
        }
        return node;
      });

      if (hasActualStagedUpdates) {
        syncFromReactFlow(updatedNodes, edges);
      }
    }

    // Get the updated state after applying staged changes
    const finalState = useWorkflowStore.getState();

    // Validate workflow structure before saving
    const validation = validateWorkflowStructure(
      finalState.nodes,
      finalState.edges
    );

    // Convert client validation results to ValidationMessage format
    const clientErrors = convertClientErrors(
      validation.errors,
      finalState.nodes
    );
    const clientWarnings = convertClientWarnings(
      validation.warnings,
      finalState.nodes
    );

    // If there are errors, show in validation panel and stop
    if (!validation.isValid) {
      useValidationStore
        .getState()
        .setMessages([...clientErrors, ...clientWarnings]);
      return;
    }

    // If only warnings, still show them but continue with save
    if (clientWarnings.length > 0) {
      useValidationStore.getState().setMessages(clientWarnings);
    }

    // Build variables object for execution graph
    // Use staged changes if available, otherwise use original data
    // Convert from UI format [{ name, value, type }, ...] to API format { varName: { type, value }, ... }
    // Values are already typed correctly (boolean, number, etc.) from VariablesEditor
    const variablesArray =
      stagedWorkflowChanges.variables ?? data.variables ?? [];
    let variables: Record<string, { type: string; value: any }> | undefined;
    if (variablesArray.length > 0) {
      variables = variablesArray.reduce(
        (
          acc: Record<string, { type: string; value: any }>,
          variable: { name: string; value: any; type: string }
        ) => {
          acc[variable.name] = {
            type: variable.type || 'string',
            value: variable.value,
          };
          return acc;
        },
        {} as Record<string, { type: string; value: any }>
      );
    }

    // Build input/output schemas for execution graph
    // Use staged changes if available, otherwise use original data
    const inputSchemaFieldsToUse =
      stagedWorkflowChanges.inputSchemaFields ?? data.inputSchemaFields ?? [];
    const outputSchemaFieldsToUse =
      stagedWorkflowChanges.outputSchemaFields ?? data.outputSchemaFields ?? [];

    const inputSchema =
      inputSchemaFieldsToUse.length > 0
        ? buildSchemaFromFields(inputSchemaFieldsToUse)
        : undefined;
    const outputSchema =
      outputSchemaFieldsToUse.length > 0
        ? buildSchemaFromFields(outputSchemaFieldsToUse)
        : undefined;

    // Get execution timeout from staged changes or original data
    const executionTimeoutSeconds =
      stagedWorkflowChanges.executionTimeoutSeconds ??
      data.executionTimeoutSeconds;

    // Get rate limit budget from staged changes or original data
    const rateLimitBudgetMs =
      stagedWorkflowChanges.rateLimitBudgetMs ?? data.rateLimitBudgetMs;

    // Get name and description for the execution graph
    // Name is required - prefer staged changes, fall back to current data, ensure non-empty
    const workflowName = stagedWorkflowChanges.name || data.name || '';
    // Description is optional - only include if it has a value
    const workflowDescription =
      stagedWorkflowChanges.description ?? data.description ?? '';

    // Compose execution graph with name, description, variables, schemas, and timeout included
    const executionGraph = composeExecutionGraph(
      finalState.nodes,
      finalState.edges,
      {
        name: workflowName,
        description: workflowDescription,
        variables,
        inputSchema,
        outputSchema,
        executionTimeoutSeconds,
        rateLimitBudgetMs,
      }
    );

    // Single update call for workflow, metadata, and schemas
    try {
      await new Promise((resolve, reject) => {
        updateMutation.mutate(
          {
            id: workflowId!,
            data: executionGraph!,
          },
          {
            onSuccess: resolve,
            onError: reject,
          }
        );
      });
      // Clear staged changes after successful save
      setStagedNodeChanges({});
      setStagedWorkflowChanges({});
      // Clear any validation errors from previous failed saves
      useWorkflowStore.getState().clearValidationErrors();
      useValidationStore.getState().clearMessages();
    } catch (error: any) {
      // Extract validation errors from the API response
      const validationErrors = error?.response?.data?.validationErrors;
      if (validationErrors && validationErrors.length > 0) {
        // Convert server errors to ValidationMessage format and show in panel
        const serverErrors = convertServerErrors(
          validationErrors,
          useWorkflowStore.getState().nodes
        );
        // Combine server errors with client warnings so both are visible (SYN-234)
        useValidationStore
          .getState()
          .setMessages([...serverErrors, ...clientWarnings]);

        // Also set in workflowStore for node highlighting (existing behavior)
        useWorkflowStore.getState().setValidationErrors(validationErrors);

        // Focus the view on the first step with an error
        const firstErrorStepId =
          useValidationStore.getState().getFirstErrorStepId() ||
          useWorkflowStore.getState().getFirstErrorStepId();
        if (firstErrorStepId) {
          // Select and center on the first error node
          useWorkflowStore.getState().setSelectedNodeId(firstErrorStepId);
          useWorkflowStore.getState().setPendingCenterNodeId(firstErrorStepId);
        }
      }
      // Error toast is already handled by mutation's onError callbacks (for non-validation errors)
      console.error('Failed to save changes:', error);
    }
  };

  const handleSchedule = () => {
    if (!workflowId) {
      toast.error('Workflow ID is missing');
      return;
    }
    if (!hasInputSchema) {
      handleExecuteWorkflow({});
      return;
    }
    setExecuteError(null);
    setExecuteDialogOpen(true);
  };

  const handleExecuteWorkflow = async (inputs: Record<string, any>) => {
    if (!workflowId) {
      toast.error('Workflow ID is missing');
      return;
    }

    const trackEvents = data.trackEvents || false;
    setExecuteError(null);

    try {
      await scheduleMutation.mutateAsync({
        workflowId,
        trackEvents,
        inputs,
        version: selectedVersion,
      });
      setExecuteDialogOpen(false);
    } catch (error: any) {
      const apiError =
        error?.response?.data?.error ||
        error?.response?.data?.message ||
        error?.message;
      const errorMessage = apiError || 'Input validation failed';
      if (executeDialogOpen) {
        setExecuteError(errorMessage);
      } else {
        toast.error(errorMessage);
      }
    }
  };

  const handleResetExecution = () => {
    resetExecution();
    useWorkflowStore.getState().setSelectedNodeId(null);
    lastProcessedBreakpointRef.current = null;
    justResumedRef.current = false;
  };

  // ── Server-side Debug Execution (breakpoints) ──────────────────────────
  const [debugExecuteDialogOpen, setDebugExecuteDialogOpen] = useState(false);

  const handleDebugExecuteServer = () => {
    if (!workflowId) {
      toast.error('Workflow ID is missing');
      return;
    }
    if (!hasInputSchema) {
      handleDebugExecuteServerSubmit({});
      return;
    }
    setExecuteError(null);
    setDebugExecuteDialogOpen(true);
  };

  const handleDebugExecuteServerSubmit = async (
    inputs: Record<string, any>
  ) => {
    if (!workflowId) {
      toast.error('Workflow ID is missing');
      return;
    }

    setExecuteError(null);

    try {
      await scheduleMutation.mutateAsync({
        workflowId,
        trackEvents: true, // Always enable event tracking in debug mode
        inputs,
        version: selectedVersion,
        debug: true,
      });
      setDebugExecuteDialogOpen(false);
    } catch (error: any) {
      const apiError =
        error?.response?.data?.error ||
        error?.response?.data?.message ||
        error?.message;
      const errorMessage = apiError || 'Input validation failed';
      if (debugExecuteDialogOpen) {
        setExecuteError(errorMessage);
      } else {
        toast.error(errorMessage);
      }
    }
  };

  // ── Resume from breakpoint ──────────────────────────────────────────────
  // Track that we just resumed to prevent stale "suspended" data from re-triggering
  const justResumedRef = useRef(false);
  // Track the last breakpoint we acted on — persists across resume so dedup works
  const lastProcessedBreakpointRef = useRef<string | null>(null);

  const resumeMutation = useCustomMutation({
    mutationFn: resumeInstance,
    onSuccess: () => {
      justResumedRef.current = true;
      setSuspended(false, null);
      // Force immediate refetch so polling picks up the new status
      refetchInstanceData();
      refetchStepEvents();
      // Safety: clear the guard after 5s in case polling never sees a non-suspended status
      setTimeout(() => {
        justResumedRef.current = false;
      }, 5000);
      toast.info('Execution resumed');
    },
  });

  const handleResume = () => {
    if (!executingInstanceId) {
      toast.error('No active execution to resume');
      return;
    }
    resumeMutation.mutate(executingInstanceId);
  };

  // ── Stop execution ──────────────────────────────────────────────────
  const stopMutation = useCustomMutation({
    mutationFn: stopInstance,
    onSuccess: () => {
      resetExecution();
      toast.info('Execution stopped');
    },
  });

  const handleStop = () => {
    if (!executingInstanceId) {
      toast.error('No active execution to stop');
      return;
    }
    stopMutation.mutate(executingInstanceId);
  };

  /**
   * Navigate to a specific step in the workflow editor.
   * Called when user clicks on a validation message to jump to the affected step.
   */
  const handleNavigateToStep = useCallback((stepId: string) => {
    const { nodes, setSelectedNodeId, setPendingCenterNodeId } =
      useWorkflowStore.getState();
    const targetNode = nodes.find((n) => n.id === stepId);

    if (targetNode) {
      // Select the node to open sidebar
      setSelectedNodeId(stepId);
      // Center viewport on the node
      setPendingCenterNodeId(stepId);
    }
  }, []);

  const handleVersionActivate = (version: number) => {
    if (!workflowId) {
      toast.error('Workflow ID is missing');
      return;
    }
    setCurrentVersionMutation.mutate(
      {
        workflowId,
        versionNumber: version,
      },
      {
        onSuccess: () => {
          // Switch to the newly activated version in the editor
          setSelectedVersion(version);
          setQueryVersion(version);
        },
      }
    );
  };

  const handleExportJSON = () => {
    // Get the current state from Zustand store
    const { nodes, edges } = useWorkflowStore.getState();

    // Build variables object for export
    const variablesArray =
      stagedWorkflowChanges.variables ?? data.variables ?? [];
    let variables: Record<string, { type: string; value: string }> | undefined;
    if (variablesArray.length > 0) {
      variables = variablesArray.reduce(
        (
          acc: Record<string, { type: string; value: string }>,
          variable: { name: string; value: string; type: string }
        ) => {
          acc[variable.name] = {
            type: variable.type || 'string',
            value: variable.value,
          };
          return acc;
        },
        {} as Record<string, { type: string; value: string }>
      );
    }

    // Build schemas for export
    const inputSchemaFieldsToUse =
      stagedWorkflowChanges.inputSchemaFields ?? data.inputSchemaFields ?? [];
    const outputSchemaFieldsToUse =
      stagedWorkflowChanges.outputSchemaFields ?? data.outputSchemaFields ?? [];
    const inputSchema =
      inputSchemaFieldsToUse.length > 0
        ? buildSchemaFromFields(inputSchemaFieldsToUse)
        : undefined;
    const outputSchema =
      outputSchemaFieldsToUse.length > 0
        ? buildSchemaFromFields(outputSchemaFieldsToUse)
        : undefined;

    // Get execution timeout for export
    const exportExecutionTimeoutSeconds =
      stagedWorkflowChanges.executionTimeoutSeconds ??
      data.executionTimeoutSeconds;

    const executionGraph = composeExecutionGraph(nodes, edges, {
      variables,
      inputSchema,
      outputSchema,
      executionTimeoutSeconds: exportExecutionTimeoutSeconds,
    });

    if (executionGraph) {
      // Wrap with name and description for the new format
      const exportData = {
        name: data.name || '',
        description: data.description || '',
        executionGraph,
      };

      // Create a JSON blob and download it
      const json = JSON.stringify(exportData, null, 2);
      const blob = new Blob([json], { type: 'application/json' });
      const url = URL.createObjectURL(blob);

      // Create a temporary link and trigger download
      const a = document.createElement('a');
      a.href = url;

      // Use slugified workflow name and append version to the filename
      // Use selectedVersion (the version being viewed) instead of currentVersionNumber (the active version)
      const slugifiedName = data.name ? slugify(data.name) : 'export';
      const versionToExport = selectedVersion ?? data.currentVersionNumber;
      const versionSuffix = versionToExport ? `-v${versionToExport}` : '';
      a.download = `${slugifiedName}${versionSuffix}.json`;

      document.body.appendChild(a);
      a.click();

      // Clean up
      document.body.removeChild(a);
      URL.revokeObjectURL(url);

      toast.info('Execution graph exported successfully');
    } else {
      toast.error('No execution graph to export');
    }
  };

  // Internal import handler (bypasses unsaved changes check)
  const performImportJSON = useCallback((jsonString: string) => {
    try {
      const parsed = JSON.parse(jsonString);

      // Handle new format with wrapper: { name, description, executionGraph }
      const executionGraph = parsed.executionGraph ?? parsed;

      if (
        executionGraph &&
        (executionGraph.steps || executionGraph.executionPlan)
      ) {
        // Clear staged node changes when importing
        setStagedNodeChanges({});

        // Extract variables from execution graph and convert to UI format
        // API format: { varName: { type, value }, ... }
        // UI format: [{ name, value, type }, ...]
        const variablesObj = executionGraph.variables || {};
        const variables = Object.entries(variablesObj).map(
          ([name, val]: [string, any]) => ({
            name,
            value: val?.value ?? val ?? '',
            type: val?.type ?? 'string',
          })
        );

        // Extract and parse input/output schemas from execution graph
        // Convert SchemaField[] to the required format with all fields having defaults
        const inputSchemaFields = parseSchema(executionGraph.inputSchema).map(
          (field) => ({
            name: field.name,
            type: field.type ?? 'string',
            required: field.required ?? true,
            description: field.description ?? '',
            defaultValue: field.defaultValue,
          })
        );
        const outputSchemaFields = parseSchema(executionGraph.outputSchema).map(
          (field) => ({
            name: field.name,
            type: field.type ?? 'string',
            required: field.required ?? true,
            description: field.description ?? '',
            defaultValue: field.defaultValue,
          })
        );

        // Extract execution timeout from execution graph
        const executionTimeoutSeconds = executionGraph.executionTimeoutSeconds;

        // Stage the workflow changes (variables, schemas, timeout)
        const workflowChanges: typeof stagedWorkflowChanges = {};
        if (variables.length > 0) {
          workflowChanges.variables = variables;
        }
        if (inputSchemaFields.length > 0) {
          workflowChanges.inputSchemaFields = inputSchemaFields;
        }
        if (outputSchemaFields.length > 0) {
          workflowChanges.outputSchemaFields = outputSchemaFields;
        }
        if (executionTimeoutSeconds !== undefined) {
          workflowChanges.executionTimeoutSeconds = executionTimeoutSeconds;
        }

        setStagedWorkflowChanges(workflowChanges);

        // Use the store to set the execution graph (for nodes/edges)
        useWorkflowStore.getState().setExecutionGraph(executionGraph);

        toast.info('Execution graph imported successfully');
      } else {
        toast.error('Invalid execution graph format');
      }
    } catch (error) {
      console.error('Error importing execution graph:', error);
      toast.error('Failed to import execution graph');
    }
  }, []);

  const handleImportJSON = useCallback(
    (jsonString: string) => {
      confirmIfUnsavedChanges(() => performImportJSON(jsonString));
    },
    [confirmIfUnsavedChanges, performImportJSON]
  );

  const handleAutoLayout = () => {
    useWorkflowStore.getState().applyAutoLayout();
  };

  const handleAddNote = () => {
    // Get the viewport center position in flow coordinates
    const viewport = getViewport();
    const centerX = (-viewport.x + window.innerWidth / 2) / viewport.zoom;
    const centerY = (-viewport.y + window.innerHeight / 2) / viewport.zoom;

    // Add note at the center of the visible viewport
    useWorkflowStore.getState().addNote({ x: centerX, y: centerY });

    toast.info('Note added to canvas');
  };

  const isLoading =
    updateMutation.isPending ||
    scheduleMutation.isPending ||
    setCurrentVersionMutation.isPending ||
    toggleTrackEventsMutation.isPending ||
    resumeMutation.isPending ||
    stopMutation.isPending;

  // Check if any step has a breakpoint set
  const hasBreakpoints = useWorkflowStore((state) =>
    state.nodes.some((n) => (n.data as any)?.breakpoint)
  );

  // Memoize workflow object to prevent unnecessary re-renders in WorkflowPropertiesDialog
  const workflowForSettings = useMemo(
    () => ({
      id: workflowId || '',
      name: stagedWorkflowChanges.name ?? data.name ?? '',
      description: stagedWorkflowChanges.description ?? data.description ?? '',
      variables: stagedWorkflowChanges.variables ?? data.variables ?? [],
      inputSchemaFields:
        stagedWorkflowChanges.inputSchemaFields ?? data.inputSchemaFields ?? [],
      outputSchemaFields:
        stagedWorkflowChanges.outputSchemaFields ??
        data.outputSchemaFields ??
        [],
      executionTimeoutSeconds:
        stagedWorkflowChanges.executionTimeoutSeconds ??
        data.executionTimeoutSeconds,
      rateLimitBudgetMs:
        stagedWorkflowChanges.rateLimitBudgetMs ?? data.rateLimitBudgetMs,
    }),
    [
      workflowId,
      stagedWorkflowChanges.name,
      stagedWorkflowChanges.description,
      stagedWorkflowChanges.variables,
      stagedWorkflowChanges.inputSchemaFields,
      stagedWorkflowChanges.outputSchemaFields,
      stagedWorkflowChanges.executionTimeoutSeconds,
      stagedWorkflowChanges.rateLimitBudgetMs,
      data.name,
      data.description,
      data.variables,
      data.inputSchemaFields,
      data.outputSchemaFields,
      data.executionTimeoutSeconds,
      data.rateLimitBudgetMs,
    ]
  );

  if (!workflowId) {
    return <div className="p-6 text-center">Workflow ID is missing</div>;
  }

  // Only show full-page loader on initial load, not when switching versions
  if (isFetching && !isSwitchingVersion) {
    return <Loader />;
  }

  return (
    <div className="flex h-full flex-col bg-background">
      <div className="relative flex-1 min-h-0 overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 z-10 flex justify-center">
          <div className="pointer-events-auto">
            <WorkflowActionsForm
              isLoading={isLoading}
              workflowName={stagedWorkflowChanges.name ?? data.name ?? ''}
              onSchedule={handleSchedule}
              onSubmit={handleSubmit}
              onExportJSON={handleExportJSON}
              onImportJSON={handleImportJSON}
              onAutoLayout={handleAutoLayout}
              onAddNote={handleAddNote}
              isExecuting={!!executingInstanceId}
              isExecutionActive={
                executionInstanceData?.status &&
                [
                  ExecutionStatus.Queued,
                  ExecutionStatus.Compiling,
                  ExecutionStatus.Running,
                  ExecutionStatus.Suspended,
                ].includes(executionInstanceData.status)
              }
              isDirty={hasStructuralUnsavedChanges}
              onStop={handleStop}
              onClearExecution={handleResetExecution}
              onViewExecutionDetails={() => {
                // Switch to history tab (this also expands the panel)
                useValidationStore.getState().setActiveTab('history');
                // Select the running instance in history
                useExecutionStore
                  .getState()
                  .setSelectedInvocationId(executingInstanceId);
              }}
              executionStats={
                executionInstanceData
                  ? {
                      status: executionInstanceData.status,
                      queueDuration:
                        executionInstanceData.queueDurationSeconds ?? undefined,
                      executionDuration:
                        executionInstanceData.executionDurationSeconds ??
                        undefined,
                      maxMemory: executionInstanceData.maxMemoryMb ?? undefined,
                      terminationType:
                        executionInstanceData.terminationType ?? undefined,
                    }
                  : undefined
              }
              onDebugExecute={handleDebugExecuteServer}
              isSuspended={isSuspended}
              onResume={handleResume}
              isResuming={resumeMutation.isPending}
              hasBreakpoints={hasBreakpoints}
            />
          </div>
        </div>
        <div className="absolute inset-0">
          {isSwitchingVersion && (
            <div className="absolute inset-0 z-20 flex items-center justify-center bg-background/50">
              <Loader />
            </div>
          )}
          <WorkflowEditor
            nodes={data.nodes}
            edges={data.edges}
            readOnly={!!executingInstanceId}
            debugInspectMode={isSuspended}
            workflow={workflowForSettings}
            stagedNodeChanges={stagedNodeChanges}
            onStagedNodeChange={handleStagedNodeChange}
            onResetNodeChanges={handleResetNodeChanges}
          />
          {/* Debug step inspector — floating panel when suspended */}
          {isSuspended && <DebugStepInspector />}
        </div>
      </div>

      <ValidationPanel
        onNavigateToStep={handleNavigateToStep}
        workflowId={workflowId}
        workflow={workflowForSettings}
        onWorkflowChange={(changes) => {
          setStagedWorkflowChanges((prev) => ({
            ...prev,
            ...changes,
          }));
        }}
        readOnly={!!executingInstanceId}
        versions={versionsWithDebug}
        selectedVersion={selectedVersion}
        currentVersionNumber={data.currentVersionNumber}
        onVersionChange={handleVersionChange}
        onVersionActivate={handleVersionActivate}
        isVersionLoading={isLoading}
      />

      <WorkflowExecuteDialog
        open={executeDialogOpen}
        onOpenChange={(open) => {
          setExecuteError(null);
          setExecuteDialogOpen(open);
        }}
        workflowName={data.name}
        inputSchema={workflowInputSchema}
        onExecute={handleExecuteWorkflow}
        isSubmitting={scheduleMutation.isPending}
        serverError={executeError}
      />

      {/* Debug execute dialog (server-side breakpoints) — reuses WorkflowExecuteDialog */}
      <WorkflowExecuteDialog
        open={debugExecuteDialogOpen}
        onOpenChange={(open) => {
          setExecuteError(null);
          setDebugExecuteDialogOpen(open);
        }}
        workflowName={`${data.name} (Debug)`}
        inputSchema={workflowInputSchema}
        onExecute={handleDebugExecuteServerSubmit}
        isSubmitting={scheduleMutation.isPending}
        serverError={executeError}
      />

      <UnsavedChangesDialog
        open={isUnsavedChangesDialogOpen}
        onConfirm={handleConfirmUnsavedChanges}
        onCancel={handleCancelUnsavedChanges}
      />
    </div>
  );
}
