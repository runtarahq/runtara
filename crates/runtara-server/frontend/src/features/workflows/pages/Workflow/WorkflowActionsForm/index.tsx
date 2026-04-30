import { z } from 'zod';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { NextForm } from '@/shared/components/NextForm';
import { Button } from '@/shared/components/ui/button';
import * as form from './WorkflowActionsItem';
import { useRef } from 'react';
import {
  Save,
  Play,
  Square,
  Download,
  Upload,
  Eye,
  X,
  Lock,
  Bug,
  SkipForward,
  Pause,
} from 'lucide-react';

const { schema, initialValues } = form;

type SchemaType = z.infer<typeof schema>;

type ExecutionStats = {
  status?: string;
  queueDuration?: number;
  executionDuration?: number;
  maxMemory?: number;
  terminationType?: string;
};

type Props = {
  isLoading: boolean;
  workflowName: string;
  onSchedule: () => void;
  onSubmit: (values: Record<string, any>) => void;
  onExportJSON: () => void;
  onImportJSON: (json: string) => void;
  isExecuting?: boolean;
  isExecutionActive?: boolean;
  isDirty?: boolean;
  onStop?: () => void;
  onClearExecution?: () => void;
  onViewExecutionDetails?: () => void;
  executionStats?: ExecutionStats;
  onDebugExecute?: () => void;
  isSuspended?: boolean;
  onResume?: () => void;
  isResuming?: boolean;
  hasBreakpoints?: boolean;
};

export function WorkflowActionsForm(props: Props) {
  const {
    isLoading,
    workflowName,
    onSchedule,
    onSubmit,
    onExportJSON,
    onImportJSON,
    isExecuting,
    isExecutionActive,
    isDirty,
    onStop,
    onClearExecution,
    onViewExecutionDetails,
    executionStats,
    onDebugExecute,
    isSuspended,
    onResume,
    isResuming,
    hasBreakpoints,
  } = props;
  const fileInputRef = useRef<HTMLInputElement>(null);

  const form = useForm<SchemaType>({
    resolver: zodResolver(schema),
    defaultValues: initialValues,
  });

  const handleImportClick = () => {
    if (fileInputRef.current) {
      fileInputRef.current.click();
    }
  };

  const handleFileChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) {
      const reader = new FileReader();
      reader.onload = (e) => {
        try {
          const json = e.target?.result as string;
          onImportJSON(json);
        } catch (error) {
          console.error('Error parsing JSON file:', error);
        }
      };
      reader.readAsText(file);
    }
    // Reset the file input so the same file can be selected again
    if (event.target) {
      event.target.value = '';
    }
  };

  return (
    <div className="flex flex-col items-center">
      <h1 className="mt-3 mb-2 text-lg font-semibold text-slate-900/90 drop-shadow-sm">
        {workflowName || 'Untitled Workflow'}
      </h1>

      <NextForm
        className="inline-flex items-center gap-1 rounded-lg border border-border/60 bg-white/95 px-2 py-1.5 shadow-lg backdrop-blur supports-[backdrop-filter]:backdrop-blur"
        form={form}
        renderContent={() => null}
        renderActions={() => (
          <>
            <input
              type="file"
              ref={fileInputRef}
              style={{ display: 'none' }}
              accept=".json"
              onChange={handleFileChange}
            />

            {/* Execution status indicator */}
            {isExecuting && (
              <>
                <div className="flex items-center gap-2 px-1">
                  {isSuspended ? (
                    <Pause className="h-3.5 w-3.5 text-amber-600" />
                  ) : (
                    <Lock className="h-3.5 w-3.5 text-muted-foreground" />
                  )}
                  <span className="text-xs font-medium text-slate-700">
                    {isSuspended
                      ? 'Paused at breakpoint'
                      : isExecutionActive
                        ? 'Execution in progress'
                        : executionStats?.status === 'completed'
                          ? 'Completed'
                          : executionStats?.status === 'failed'
                            ? 'Execution failed'
                            : executionStats?.status === 'timeout'
                              ? 'Execution timed out'
                              : executionStats?.status === 'cancelled'
                                ? 'Execution cancelled'
                                : 'Execution in progress'}
                  </span>
                  {executionStats?.executionDuration !== undefined &&
                    executionStats?.executionDuration !== null && (
                      <span className="text-xs text-muted-foreground">
                        {executionStats.executionDuration.toFixed(2)}s
                      </span>
                    )}
                </div>
                <div className="mx-1 h-4 w-px bg-border" />
              </>
            )}

            {/* Start button */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-7 rounded p-0 text-blue-600 hover:bg-blue-50 hover:text-blue-700"
              disabled={isLoading || isExecuting || isDirty}
              onClick={onSchedule}
              title={
                isDirty
                  ? 'Please save your changes before starting execution'
                  : 'Start workflow'
              }
            >
              <Play className="h-4 w-4" />
            </Button>

            {/* Debug execute button (server-side with breakpoints) */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-7 rounded p-0 text-orange-600 hover:bg-orange-50 hover:text-orange-700"
              disabled={isLoading || isExecuting || isDirty || !hasBreakpoints}
              onClick={onDebugExecute}
              title={
                isDirty
                  ? 'Please save your changes before debugging'
                  : !hasBreakpoints
                    ? 'Set breakpoints on steps first (right-click a step node)'
                    : 'Debug workflow (pause at breakpoints)'
              }
            >
              <Bug className="h-4 w-4" />
            </Button>

            {/* Resume button - only show when suspended at breakpoint */}
            {isSuspended && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 w-7 rounded p-0 text-green-600 hover:bg-green-50 hover:text-green-700"
                disabled={isLoading || isResuming}
                onClick={onResume}
                title="Continue execution to next breakpoint"
              >
                <SkipForward className="h-4 w-4" />
              </Button>
            )}

            {/* Stop button - only show when executing */}
            {isExecutionActive && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 w-7 rounded p-0 text-red-600 hover:bg-red-50 hover:text-red-700"
                disabled={isLoading}
                onClick={onStop}
                title="Stop execution"
              >
                <Square className="h-3.5 w-3.5" />
              </Button>
            )}

            {/* Details button - only show when executing */}
            {isExecuting && onViewExecutionDetails && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-muted hover:text-foreground"
                onClick={onViewExecutionDetails}
                title="View execution details"
              >
                <Eye className="h-4 w-4" />
              </Button>
            )}

            {/* Clear button - only show when executing */}
            {isExecuting && onClearExecution && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-muted hover:text-foreground"
                onClick={onClearExecution}
                title="Clear execution results"
              >
                <X className="h-4 w-4" />
              </Button>
            )}

            <div className="mx-1 h-4 w-px bg-border" />

            {/* Save button */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-7 rounded p-0 text-foreground/70 hover:bg-muted hover:text-foreground"
              disabled={isLoading || isExecuting || !isDirty}
              title={isDirty ? 'Save changes' : 'No changes to save'}
              onClick={form.handleSubmit(onSubmit)}
            >
              <Save className="h-4 w-4" />
            </Button>

            <div className="mx-1 h-4 w-px bg-border" />

            {/* Export button */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-muted hover:text-foreground"
              disabled={isLoading}
              onClick={onExportJSON}
              title="Export workflow"
            >
              <Upload className="h-4 w-4" />
            </Button>

            {/* Import button */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-7 rounded p-0 text-muted-foreground hover:bg-muted hover:text-foreground"
              disabled={isLoading}
              onClick={handleImportClick}
              title="Import workflow"
            >
              <Download className="h-4 w-4" />
            </Button>
          </>
        )}
        onSubmit={onSubmit}
      />
    </div>
  );
}
