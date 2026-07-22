import { useRef, useState } from 'react';
import { File as FileIcon, Loader2, Upload, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { fileToFileData, formatFileSize } from '@/shared/utils/file-utils';
import { MAX_FILE_SIZE_BYTES } from '@/shared/types/file';
import type { FileData } from '@/shared/types/file';
import {
  ReportBlockDefinition,
  ReportWorkflowActionConfig,
} from '../../types';
import {
  useReportWorkflowAction,
  type ReportWorkflowActionResult,
} from './useReportWorkflowAction';

/** Mirrors the server's fallback when `workflowAction.id` is unset. */
const FILE_UPLOAD_ACTION_FALLBACK_ID = 'upload';

type SelectedFile = {
  data: FileData;
  sizeBytes: number;
};

type FileUploadBlockProps = {
  reportId: string;
  activeViewId?: string | null;
  block: ReportBlockDefinition;
  filters: Record<string, unknown>;
  onRefresh: (
    result?: ReportWorkflowActionResult,
    action?: ReportWorkflowActionConfig
  ) => void | Promise<void>;
};

export function FileUploadBlock({
  reportId,
  activeViewId,
  block,
  filters,
  onRefresh,
}: FileUploadBlockProps) {
  const config = block.file_upload;
  const inputRef = useRef<HTMLInputElement>(null);
  const [selected, setSelected] = useState<SelectedFile | null>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const [isReading, setIsReading] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  const workflowAction = useReportWorkflowAction({
    onCompleted: async (result, action) => {
      if (result.status === 'completed') {
        setSelected(null);
      }
      await onRefresh(result, action);
    },
    report: {
      reportId,
      blockId: block.id,
      viewId: activeViewId,
      filters,
    },
  });

  if (!config) return null;
  const action = config.workflowAction;
  const actionKey = action.id ?? FILE_UPLOAD_ACTION_FALLBACK_ID;
  const automatic = config.trigger === 'automatic';
  const accept = config.accept ?? [];
  const maxSizeBytes = Math.min(
    config.maxSizeBytes ?? MAX_FILE_SIZE_BYTES,
    MAX_FILE_SIZE_BYTES
  );
  const phase = workflowAction.phase(actionKey);
  const isRunning = workflowAction.isRunning(actionKey);
  const busy = isRunning || isReading;

  const runWithFile = (data: FileData) =>
    workflowAction.run({
      key: actionKey,
      action,
      value: data,
      fallbackField: FILE_UPLOAD_ACTION_FALLBACK_ID,
    });

  const handleFile = async (file: File) => {
    setLocalError(null);
    const problem = validateFile(file, accept, maxSizeBytes);
    if (problem) {
      setLocalError(problem);
      return;
    }
    setIsReading(true);
    try {
      const data = await fileToFileData(file);
      if (automatic) {
        setSelected({ data, sizeBytes: file.size });
        await runWithFile(data);
      } else {
        setSelected({ data, sizeBytes: file.size });
      }
    } catch {
      setLocalError('Failed to read file');
    } finally {
      setIsReading(false);
    }
  };

  const openPicker = () => {
    if (!busy) inputRef.current?.click();
  };

  const runningLabel =
    phase === 'refreshing'
      ? 'Refreshing…'
      : (action.runningLabel ?? 'Running workflow…');

  return (
    <div className="grid gap-2">
      <input
        ref={inputRef}
        type="file"
        accept={accept.join(',') || undefined}
        disabled={busy}
        className="sr-only"
        data-testid={`file-upload-input-${block.id}`}
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) void handleFile(file);
          // Reset so re-selecting the same file fires change again.
          event.target.value = '';
        }}
      />

      <div
        role="button"
        tabIndex={busy ? -1 : 0}
        aria-disabled={busy}
        data-testid={`file-upload-dropzone-${block.id}`}
        onClick={openPicker}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            openPicker();
          }
        }}
        onDrop={(event) => {
          event.preventDefault();
          setIsDragOver(false);
          if (busy) return;
          const file = event.dataTransfer.files?.[0];
          if (file) void handleFile(file);
        }}
        onDragOver={(event) => {
          event.preventDefault();
          if (!busy) setIsDragOver(true);
        }}
        onDragLeave={(event) => {
          event.preventDefault();
          setIsDragOver(false);
        }}
        className={cn(
          'flex cursor-pointer flex-col items-center justify-center gap-2 rounded-lg border-2 border-dashed border-input bg-background px-6 py-8 text-center transition-colors',
          isDragOver && 'border-primary bg-primary/5',
          busy && 'cursor-not-allowed opacity-70',
          localError && 'border-destructive'
        )}
      >
        {busy ? (
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        ) : (
          <Upload className="h-6 w-6 text-muted-foreground" />
        )}
        <div className="grid gap-0.5">
          <span className="text-sm font-medium text-foreground">
            {isRunning
              ? runningLabel
              : (config.title ?? 'Click to upload or drag and drop')}
          </span>
          {config.description && !isRunning ? (
            <span className="text-xs text-muted-foreground">
              {config.description}
            </span>
          ) : null}
          {!isRunning ? (
            <span className="text-xs text-muted-foreground">
              {accept.length > 0 ? `${accept.join(', ')} · ` : ''}
              Max size: {formatFileSize(maxSizeBytes)}
            </span>
          ) : null}
        </div>
      </div>

      {localError ? (
        <p className="text-xs text-destructive">{localError}</p>
      ) : null}

      {!automatic && selected ? (
        <div className="flex items-center gap-3 rounded-md border border-input bg-background px-3 py-2">
          <FileIcon className="h-4 w-4 flex-shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1 truncate text-sm">
            {selected.data.filename ?? 'Selected file'}
            <span className="ml-2 text-xs text-muted-foreground">
              {formatFileSize(selected.sizeBytes)}
            </span>
          </span>
          {!isRunning ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6 w-6 p-0"
              aria-label="Clear selected file"
              onClick={() => {
                setSelected(null);
                setLocalError(null);
              }}
            >
              <X className="h-3.5 w-3.5" />
            </Button>
          ) : null}
          <Button
            type="button"
            size="sm"
            disabled={busy}
            data-testid={`file-upload-run-${block.id}`}
            onClick={() => void runWithFile(selected.data)}
          >
            {isRunning ? (
              <>
                <Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />
                {runningLabel}
              </>
            ) : (
              (action.label ?? 'Run workflow')
            )}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function validateFile(
  file: File,
  accept: string[],
  maxSizeBytes: number
): string | null {
  if (file.size > maxSizeBytes) {
    return `File size (${formatFileSize(file.size)}) exceeds maximum allowed (${formatFileSize(maxSizeBytes)})`;
  }
  // The native picker enforces `accept`, but drag-and-drop bypasses it.
  if (accept.length > 0 && !matchesAccept(file, accept)) {
    return `File type not accepted. Allowed: ${accept.join(', ')}`;
  }
  return null;
}

function matchesAccept(file: File, accept: string[]): boolean {
  const name = file.name.toLowerCase();
  const mime = (file.type || '').toLowerCase();
  return accept.some((entry) => {
    const pattern = entry.trim().toLowerCase();
    if (!pattern) return false;
    if (pattern.startsWith('.')) return name.endsWith(pattern);
    if (pattern.endsWith('/*')) return mime.startsWith(pattern.slice(0, -1));
    return mime === pattern;
  });
}
