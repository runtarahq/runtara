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
              'flex items-center gap-3 rounded-md border border-input bg-background px-3 py-2 h-8',
              displayError && 'border-destructive',
              disabled && 'opacity-50 cursor-not-allowed'
            )}
          >
            <File className="h-4 w-4 text-muted-foreground flex-shrink-0" />
            <span className="text-sm truncate flex-1">
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
            onDrop={handleDrop}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            className={cn(
              'flex items-center gap-2 rounded-md border border-dashed border-input bg-background px-3 py-2 h-8 cursor-pointer transition-colors',
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
            <span className="text-sm text-muted-foreground truncate">
              {isLoading ? 'Reading file...' : placeholder}
            </span>
          </div>
        )}

        {displayError && (
          <p className="text-xs text-destructive mt-1">{displayError}</p>
        )}

        {!hasValue && !displayError && (
          <p className="text-xs text-muted-foreground mt-1">
            Max size: {MAX_FILE_SIZE_DISPLAY}
          </p>
        )}
      </div>
    );
  }
);

FileInput.displayName = 'FileInput';

export { FileInput as default };
