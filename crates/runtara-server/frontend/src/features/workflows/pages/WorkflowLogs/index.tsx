import { useNavigate, useParams } from 'react-router';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { Loader } from '@/shared/components/loader.tsx';
import { formatDate } from '@/lib/utils.ts';
import { Badge } from '@/shared/components/ui/badge.tsx';
import { Button } from '@/shared/components/ui/button';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Filter,
  Info,
  Search,
} from 'lucide-react';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import React, { useEffect, useRef, useState } from 'react';
import { Input } from '@/shared/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/shared/components/ui/collapsible';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import { StepEventResponse } from '@/generated/RuntaraRuntimeApi';
import { StructuredErrorDisplay } from '@/shared/components/StructuredErrorDisplay';
import { parseStructuredError } from '@/shared/utils/structured-error';
import { resolveRecordPayloads } from '@/shared/utils/truncated-payload';
import {
  isStepDebugStartPayload,
  isStepDebugEndPayload,
  isWorkflowLogPayload,
} from '@/features/workflows/types/step-events';

const getLogLevelIcon = (level: string) => {
  switch (level?.toLowerCase()) {
    case 'error':
      return <AlertCircle className="h-4 w-4 text-destructive" />;
    case 'systemerror':
      return <AlertCircle className="h-4 w-4 text-red-700" />;
    case 'warning':
    case 'warn':
      return <AlertTriangle className="h-4 w-4 text-yellow-500" />;
    case 'success':
    case 'info':
      return <CheckCircle className="h-4 w-4 text-green-500" />;
    default:
      return <Info className="h-4 w-4 text-muted-foreground" />;
  }
};

const getLogLevelBadgeVariant = (
  level: string
): 'default' | 'secondary' | 'destructive' | 'outline' => {
  switch (level?.toLowerCase()) {
    case 'error':
    case 'systemerror':
      return 'destructive';
    case 'warning':
    case 'warn':
      return 'secondary';
    case 'success':
      return 'default';
    default:
      return 'outline';
  }
};

// Helper function to highlight search terms in text
const highlightText = (
  text: string | null | undefined,
  searchTerm: string
): React.ReactNode => {
  if (!text || !searchTerm) return text || '';

  const escapedSearchTerm = searchTerm.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const regex = new RegExp(`(${escapedSearchTerm})`, 'gi');
  const parts = text.split(regex);

  return parts.map((part, index) =>
    regex.test(part) ? (
      <span
        key={index}
        className="bg-yellow-200 dark:bg-yellow-800 px-0.5 rounded"
      >
        {part}
      </span>
    ) : (
      part
    )
  );
};

// Helper function to check if object contains search term
const objectContainsSearchTerm = (
  obj: Record<string, unknown> | undefined,
  searchTerm: string
): boolean => {
  if (!obj || !searchTerm) return false;

  const searchLower = searchTerm.toLowerCase();
  const str = JSON.stringify(obj).toLowerCase();
  return str.includes(searchLower);
};

// Component to render JSON with highlighted search terms
const HighlightedJson: React.FC<{
  data: Record<string, unknown>;
  searchTerm: string;
}> = ({ data, searchTerm }) => {
  const jsonString = JSON.stringify(data, null, 2);

  if (!searchTerm) return <>{jsonString}</>;

  const escapedSearchTerm = searchTerm.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const regex = new RegExp(`(${escapedSearchTerm})`, 'gi');
  const parts = jsonString.split(regex);

  return (
    <>
      {parts.map((part, index) =>
        regex.test(part) ? (
          <span
            key={index}
            className="bg-yellow-200 dark:bg-yellow-800 px-0.5 rounded"
          >
            {part}
          </span>
        ) : (
          part
        )
      )}
    </>
  );
};

// Transform StepEvent to log-like format
interface LogEntry {
  id: string;
  createdAt: string;
  stepName: string;
  logLevel: string;
  message: string;
  contextData?: Record<string, unknown>;
  itemIndex?: number;
  totalItems?: number;
  workflowName?: string;
}

export function WorkflowLogs() {
  const { workflowId, instanceId } = useParams();
  const navigate = useNavigate();
  const [searchInput, setSearchInput] = useState('');
  const [searchTerm, setSearchTerm] = useState('');
  const [logLevelFilter, setLogLevelFilter] = useState<string>('all');
  const [pageIndex, setPageIndex] = useState(0);
  const [pageSize] = useState(20);
  const [expandedContextIds, setExpandedContextIds] = useState<Set<string>>(
    new Set()
  );
  const isInitialLoadRef = useRef(true);

  // Debounce search input
  useEffect(() => {
    const timer = setTimeout(() => {
      setSearchTerm(searchInput);
      if (searchInput !== searchTerm) {
        setPageIndex(0); // Reset to first page on search
      }
    }, 500);

    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchInput]);

  // Fetch step events from RuntimeAPI
  const { data, isLoading } = useCustomQuery({
    queryKey: [
      ...queryKeys.workflows.logs(instanceId ?? ''),
      workflowId,
      pageIndex,
      pageSize,
      searchTerm,
      logLevelFilter,
    ],
    queryFn: async (token: string) => {
      const response = await RuntimeREST.api.getStepEvents(
        workflowId!,
        instanceId!,
        {
          limit: 1000, // Get more results since we'll filter client-side
        },
        createAuthHeaders(token)
      );

      // Transform step events to LogEntry[] format
      // New API structure: events have subtype (step_debug_start, step_debug_end, workflow_log)
      // with payload containing step details
      const rawEvents: StepEventResponse[] = response.data?.data?.events || [];

      // Process events - pair start/end events and handle workflow logs
      const startEvents = new Map<string, StepEventResponse>();
      const logs: LogEntry[] = [];

      // Sort events by id to process in order
      const sortedEvents = [...rawEvents].sort((a, b) => a.id - b.id);

      for (const event of sortedEvents) {
        const payload = event.payload;

        if (event.subtype === 'workflow_log' && isWorkflowLogPayload(payload)) {
          // Handle workflow log events directly
          logs.push({
            id: `log-${event.id}`,
            createdAt: event.createdAt,
            stepName: payload.step_name || payload.step_id || 'Workflow',
            logLevel: payload.level || 'Info',
            message: payload.message || JSON.stringify(payload),
            contextData: payload.context_data,
          });
        } else if (
          event.subtype === 'step_debug_start' &&
          isStepDebugStartPayload(payload)
        ) {
          // Store start event by step_id
          startEvents.set(payload.step_id, event);
        } else if (
          event.subtype === 'step_debug_end' &&
          isStepDebugEndPayload(payload)
        ) {
          // Find matching start event and combine into a log entry
          const startEvent = startEvents.get(payload.step_id);
          const startPayload = isStepDebugStartPayload(startEvent?.payload)
            ? startEvent.payload
            : undefined;
          const stepName = payload.step_name || payload.step_id;

          const contextData: Record<string, unknown> = {};
          if (startPayload?.inputs) {
            contextData.inputs = startPayload.inputs;
          }
          if (payload.outputs !== undefined) {
            contextData.outputs = payload.outputs;
          }

          logs.push({
            id: `step-${payload.step_id}-${event.id}`,
            createdAt: event.createdAt,
            stepName: stepName,
            logLevel: 'Success',
            message: `Step ${stepName} completed (${payload.step_type})${payload.duration_ms ? ` in ${payload.duration_ms}ms` : ''}`,
            contextData:
              Object.keys(contextData).length > 0 ? contextData : undefined,
          });

          startEvents.delete(payload.step_id);
        } else if (event.eventType === 'started') {
          // Handle execution started event
          logs.push({
            id: `started-${event.id}`,
            createdAt: event.createdAt,
            stepName: 'Execution',
            logLevel: 'Info',
            message: 'Workflow execution started',
          });
        }
      }

      // Add any remaining start events as "running" entries
      for (const [stepId, startEvent] of startEvents) {
        const startPayload = isStepDebugStartPayload(startEvent.payload)
          ? startEvent.payload
          : undefined;
        const stepName = startPayload?.step_name || stepId;
        logs.push({
          id: `running-${stepId}-${startEvent.id}`,
          createdAt: startEvent.createdAt,
          stepName: stepName,
          logLevel: 'Info',
          message: `Step ${stepName} running (${startPayload?.step_type})`,
          contextData: startPayload?.inputs
            ? { inputs: startPayload.inputs }
            : undefined,
        });
      }

      // Sort logs by createdAt descending (newest first)
      logs.sort(
        (a, b) =>
          new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );

      // Client-side filtering for log level and search term
      let filteredLogs = logs;

      // Filter by log level
      if (logLevelFilter !== 'all') {
        filteredLogs = filteredLogs.filter(
          (log) => log.logLevel?.toLowerCase() === logLevelFilter.toLowerCase()
        );
      }

      // Filter by search term
      if (searchTerm) {
        const searchLower = searchTerm.toLowerCase();
        filteredLogs = filteredLogs.filter(
          (log) =>
            log.stepName?.toLowerCase().includes(searchLower) ||
            log.message?.toLowerCase().includes(searchLower) ||
            JSON.stringify(log.contextData || {})
              .toLowerCase()
              .includes(searchLower)
        );
      }

      // Pagination
      const totalElements = filteredLogs.length;
      const totalPages = Math.ceil(totalElements / pageSize);
      const paginatedLogs = filteredLogs.slice(
        pageIndex * pageSize,
        (pageIndex + 1) * pageSize
      );

      return {
        content: paginatedLogs,
        totalElements,
        totalPages,
      };
    },
    refetchInterval: 10000, // Refresh every 10 seconds
  });

  // Set page title
  usePageTitle(`Workflow Logs - Instance ${instanceId}`);

  // Update isInitialLoadRef after the first successful data load
  if (data && isInitialLoadRef.current) {
    isInitialLoadRef.current = false;
  }

  const handleBack = () => {
    if (window.history.state && window.history.state.idx > 0) {
      navigate(-1);
    } else {
      navigate('/workflows');
    }
  };

  const toggleContextExpanded = (logId: string) => {
    setExpandedContextIds((prev) => {
      const newSet = new Set(prev);
      if (newSet.has(logId)) {
        newSet.delete(logId);
      } else {
        newSet.add(logId);
      }
      return newSet;
    });
  };

  // Use data directly since filtering is now server-side
  const logs = data?.content || [];

  // Only show loader on initial load
  if ((isLoading || !data) && isInitialLoadRef.current) {
    return <Loader />;
  }

  const totalPages = data?.totalPages || 0;
  const hasLogs = logs.length > 0;

  return (
    <div className="py-6 px-4 max-w-full overflow-x-hidden">
      {/* Header */}
      <div className="flex items-center justify-between mb-6 bg-background">
        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            size="icon"
            className="rounded-full hover:bg-muted"
            onClick={handleBack}
          >
            <ChevronLeft className="h-5 w-5" />
          </Button>
          <div>
            <h1 className="text-2xl font-bold">Activity Log</h1>
            <p className="text-sm text-muted-foreground">
              Instance: {instanceId}
            </p>
          </div>
        </div>
      </div>

      {/* Search and Filters */}
      <div className="bg-card p-4 rounded-lg border mb-4">
        <div className="flex gap-3">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="Search logs..."
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              className="pl-9"
            />
          </div>
          <Select
            value={logLevelFilter}
            onValueChange={(value) => {
              setLogLevelFilter(value);
              setPageIndex(0); // Reset to first page on filter change
            }}
          >
            <SelectTrigger className="w-[180px]">
              <Filter className="h-4 w-4 mr-2" />
              <SelectValue placeholder="All logs" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All logs</SelectItem>
              <SelectItem value="error">Errors only</SelectItem>
              <SelectItem value="systemerror">System Errors only</SelectItem>
              <SelectItem value="warning">Warnings only</SelectItem>
              <SelectItem value="info">Info only</SelectItem>
              <SelectItem value="success">Success only</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      {/* Logs Container */}
      <div className="bg-card rounded-lg border overflow-hidden">
        {hasLogs ? (
          <>
            {/* Log Entries */}
            <div className="divide-y">
              {logs.map((log: LogEntry) => (
                <div
                  key={log.id}
                  className="p-4 hover:bg-muted/50 transition-colors"
                >
                  <div className="flex items-start gap-3">
                    {/* Log Level Icon */}
                    <div className="mt-1">
                      {getLogLevelIcon(log.logLevel || '')}
                    </div>

                    {/* Log Content */}
                    <div className="flex-1 min-w-0">
                      {/* Header */}
                      <div className="flex items-center gap-3 mb-2 flex-wrap">
                        <span className="text-xs text-muted-foreground">
                          {log.createdAt
                            ? formatDate(
                                new Date(log.createdAt),
                                'yyyy-MM-dd HH:mm:ss'
                              )
                            : 'N/A'}
                        </span>
                        {log.stepName && (
                          <Badge variant="secondary" className="text-xs">
                            {highlightText(log.stepName, searchTerm)}
                          </Badge>
                        )}
                        {log.logLevel && (
                          <Badge
                            variant={getLogLevelBadgeVariant(log.logLevel)}
                            className="text-xs"
                          >
                            {log.logLevel}
                          </Badge>
                        )}
                      </div>

                      {/* Message */}
                      <div className="text-sm">
                        {(() => {
                          const structuredError = parseStructuredError(
                            log.message
                          );
                          if (structuredError) {
                            return (
                              <StructuredErrorDisplay
                                error={log.message}
                                mode="compact"
                              />
                            );
                          }
                          return highlightText(log.message, searchTerm);
                        })()}
                      </div>

                      {/* Additional Details */}
                      <div className="flex gap-4 mt-2 text-xs text-muted-foreground">
                        {log.itemIndex != null && log.totalItems != null && (
                          <span>
                            Item {log.itemIndex}/{log.totalItems}
                          </span>
                        )}
                        {log.workflowName && (
                          <span>
                            Workflow:{' '}
                            {highlightText(log.workflowName, searchTerm)}
                          </span>
                        )}
                      </div>

                      {/* Context Data if present */}
                      {log.contextData && (
                        <Collapsible
                          open={expandedContextIds.has(log.id)}
                          onOpenChange={() => toggleContextExpanded(log.id)}
                          className="mt-3"
                        >
                          <CollapsibleTrigger asChild>
                            <Button
                              variant="ghost"
                              size="sm"
                              className={`h-auto p-1 text-xs ${
                                objectContainsSearchTerm(
                                  log.contextData,
                                  searchTerm
                                )
                                  ? 'text-yellow-600 dark:text-yellow-400 font-semibold'
                                  : 'text-muted-foreground'
                              } hover:text-foreground`}
                            >
                              {expandedContextIds.has(log.id) ? (
                                <ChevronDown className="h-3 w-3 mr-1" />
                              ) : (
                                <ChevronRight className="h-3 w-3 mr-1" />
                              )}
                              Context Data
                              {objectContainsSearchTerm(
                                log.contextData,
                                searchTerm
                              ) && (
                                <span className="ml-1 text-xs">
                                  (match found)
                                </span>
                              )}
                            </Button>
                          </CollapsibleTrigger>
                          <CollapsibleContent>
                            <div className="mt-2 p-3 bg-muted/50 rounded-md">
                              <pre className="text-xs overflow-auto">
                                {(() => {
                                  const resolved = resolveRecordPayloads(
                                    log.contextData!
                                  );
                                  return expandedContextIds.has(log.id) &&
                                    searchTerm ? (
                                    <HighlightedJson
                                      data={resolved}
                                      searchTerm={searchTerm}
                                    />
                                  ) : (
                                    JSON.stringify(resolved, null, 2)
                                  );
                                })()}
                              </pre>
                            </div>
                          </CollapsibleContent>
                        </Collapsible>
                      )}
                    </div>
                  </div>
                </div>
              ))}
            </div>

            {/* Pagination */}
            <div className="p-4 border-t bg-muted/50">
              <div className="flex items-center justify-between">
                <div className="text-sm text-muted-foreground">
                  Showing{' '}
                  {Math.min(pageIndex * pageSize + 1, data?.totalElements || 0)}
                  -
                  {Math.min(
                    (pageIndex + 1) * pageSize,
                    data?.totalElements || 0
                  )}{' '}
                  of {data?.totalElements || 0} logs
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setPageIndex(Math.max(0, pageIndex - 1))}
                    disabled={pageIndex === 0}
                  >
                    Previous
                  </Button>
                  <div className="flex items-center gap-1">
                    {Array.from({ length: Math.min(5, totalPages) }, (_, i) => {
                      const pageNum = pageIndex - 2 + i;
                      if (pageNum < 0 || pageNum >= totalPages) return null;
                      return (
                        <Button
                          key={pageNum}
                          variant={
                            pageNum === pageIndex ? 'default' : 'outline'
                          }
                          size="sm"
                          onClick={() => setPageIndex(pageNum)}
                          className="w-10"
                        >
                          {pageNum + 1}
                        </Button>
                      );
                    }).filter(Boolean)}
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setPageIndex(pageIndex + 1)}
                    disabled={pageIndex >= totalPages - 1}
                  >
                    Next
                  </Button>
                </div>
              </div>
            </div>
          </>
        ) : (
          /* Empty State */
          <div className="p-16 text-center">
            <Info className="h-12 w-12 mx-auto mb-4 text-muted-foreground" />
            <h3 className="text-lg font-semibold mb-2">No logs found</h3>
            <p className="text-sm text-muted-foreground">
              {searchTerm || logLevelFilter !== 'all'
                ? 'Try adjusting your search or filters'
                : 'No logs available for this workflow instance'}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
