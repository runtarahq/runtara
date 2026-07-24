import * as React from 'react';
import { Upload, File, X, Loader2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from './button';
import {
  fileToFileData,
  validateFileSize,
  parseFileDataFromString,
} from '@/shared/utils/file-utils';
import { MAX_FILE_SIZE_DISPLAY } from '@/shared/types/file';

interface FileInputProps {
  id?: string;
  labelledBy?: string;
  /** JSON string of FileData or empty string */
  value?: string;
  /** Callback when file is selected or cleared, emits JSON string */
  onChange?: (value: string) => void;
  /** Accepted file types (e.g., ".pdf,.csv" or "image/*") */
  accept?: string;
  /** Whether the input is disabled */
  disabled?: boolean;
  /** Placeholder text when no file selected */
  placeholder?: string;
  /** Additional class names */
  className?: string;
  /** Error message to display */
  error?: string;
}

export const FileInput = React.forwardRef<HTMLInputElement, FileInputProps>(
  (
    {
      value,
      id,
      labelledBy,
      onChange,
      accept,
      disabled,
      placeholder = 'Click to upload or drag and drop',
      className,
      error,
    },
    ref
  ) => {
    const inputRef = React.useRef<HTMLInputElement>(null);
    const [isLoading, setIsLoading] = React.useState(false);
    const [isDragOver, setIsDragOver] = React.useState(false);
    const [localError, setLocalError] = React.useState<string | null>(null);

    // Parse current value to get file info
    const fileInfo = React.useMemo(() => {
      return parseFileDataFromString(value);
    }, [value]);

    const hasValue = fileInfo !== null;
    const displayError = error || localError;

    const handleFileSelect = async (file: File) => {
      setLocalError(null);

      // Validate file size
      const validation = validateFileSize(file);
      if (!validation.valid) {
        setLocalError(validation.error || 'Invalid file');
        return;
      }

      setIsLoading(true);
      try {
        const fileData = await fileToFileData(file);
        onChange?.(JSON.stringify(fileData));
      } catch {
        setLocalError('Failed to read file');
      } finally {
        setIsLoading(false);
      }
    };

    const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (file) {
        handleFileSelect(file);
      }
      // Reset input so same file can be selected again
      e.target.value = '';
    };

    const handleDrop = (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragOver(false);
      if (disabled) return;

      const file = e.dataTransfer.files?.[0];
      if (file) {
        handleFileSelect(file);
      }
    };

    const handleDragOver = (e: React.DragEvent) => {
      e.preventDefault();
      if (!disabled) {
        setIsDragOver(true);
      }
    };

    const handleDragLeave = (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragOver(false);
    };

    const handleClear = (e: React.MouseEvent) => {
      e.stopPropagation();
      onChange?.('');
      setLocalError(null);
    };

    const handleClick = () => {
      if (!disabled && inputRef.current) {
        inputRef.current.click();
      }
    };

    const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        handleClick();
      }
    };

    return (
      <div className={cn('relative', className)}>
        <input
          ref={(node) => {
            (
              inputRef as React.MutableRefObject<HTMLInputElement | null>
            ).current = node;
            if (typeof ref === 'function') ref(node);
            else if (ref) ref.current = node;
          }}
          id={id}
          aria-labelledby={labelledBy}
          type="file"
          accept={accept}
          onChange={handleInputChange}
          disabled={disabled}
          className="sr-only"
        />

        {hasValue ? (
          // File selected state
          <div
            className={cn(
              'flex h-8 items-center gap-3 rounded-md border border-input bg-background px-3 py-2',
              displayError && 'border-destructive',
              disabled && 'cursor-not-allowed opacity-50'
            )}
          >
            <File className="h-4 w-4 flex-shrink-0 text-muted-foreground" />
            <span className="flex-1 truncate text-sm">
              {fileInfo?.filename || 'File selected'}
            </span>
            {!disabled && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={handleClear}
                className="h-5 w-5 p-0 hover:bg-muted"
              >
                <X className="h-3 w-3" />
              </Button>
            )}
          </div>
        ) : (
          // Drop zone / upload button
          <div
            onClick={handleClick}
            onKeyDown={handleKeyDown}
            onDrop={handleDrop}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            role="button"
            tabIndex={disabled ? -1 : 0}
            aria-labelledby={labelledBy}
            aria-disabled={disabled}
            className={cn(
              'flex h-8 cursor-pointer items-center gap-2 rounded-md border border-dashed border-input bg-background px-3 py-2 transition-colors',
              isDragOver && 'border-primary bg-primary/5',
              disabled && 'cursor-not-allowed opacity-50',
              displayError && 'border-destructive'
            )}
          >
            {isLoading ? (
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            ) : (
              <Upload className="h-4 w-4 text-muted-foreground" />
            )}
            <span className="truncate text-sm text-muted-foreground">
              {isLoading ? 'Reading file...' : placeholder}
            </span>
          </div>
        )}

        {displayError && (
          <p className="mt-1 text-xs text-destructive">{displayError}</p>
        )}

        {!hasValue && !displayError && (
          <p className="mt-1 text-xs text-muted-foreground">
            Max size: {MAX_FILE_SIZE_DISPLAY}
          </p>
        )}
      </div>
    );
  }
);

FileInput.displayName = 'FileInput';
