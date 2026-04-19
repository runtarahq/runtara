/**
 * File input component with support for both direct file upload
 * and step output references ({{steps['stepId'].outputs.file}})
 */

import { useState, useMemo } from 'react';
import { Upload, Link } from 'lucide-react';
import { FileInput } from '@/shared/components/ui/file-input';
import { AutocompleteInput } from './AutocompleteInput';
import { Button } from '@/shared/components/ui/button';
import { ValueType } from '../TypeHintSelector';

type Mode = 'upload' | 'reference';

interface FileInputWithReferencesProps {
  /** Current value - JSON string of FileData or template reference */
  value?: string;
  /** Callback when value changes */
  onChange?: (value: string) => void;
  /** Whether the input is disabled */
  disabled?: boolean;
  /** Placeholder text */
  placeholder?: string;
  /** Value type for the field */
  typeHint?: ValueType;
}

/**
 * Detects if a value looks like a partial template reference (starts with {{)
 */
function isPartialReference(value: string | undefined): boolean {
  if (!value) return false;
  return value.includes('{{');
}

export function FileInputWithReferences({
  value = '',
  onChange,
  disabled,
  placeholder,
  typeHint,
}: FileInputWithReferencesProps) {
  // Detect initial mode from value
  const initialMode: Mode = useMemo(() => {
    if (isPartialReference(value)) {
      return 'reference';
    }
    return 'upload';
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [mode, setMode] = useState<Mode>(initialMode);

  // Switch to reference mode if value becomes a reference
  const effectiveMode = useMemo(() => {
    if (isPartialReference(value) && mode === 'upload') {
      return 'reference';
    }
    return mode;
  }, [value, mode]);

  const handleModeChange = (newMode: Mode) => {
    // Clear value when switching modes to avoid confusion
    if (newMode !== mode && value) {
      onChange?.('');
    }
    setMode(newMode);
  };

  const handleFileChange = (fileValue: string) => {
    onChange?.(fileValue);
  };

  const handleReferenceChange = (refValue: string) => {
    onChange?.(refValue);
  };

  return (
    <div className="space-y-2">
      {/* Mode toggle */}
      <div className="flex gap-1">
        <Button
          type="button"
          variant={effectiveMode === 'upload' ? 'secondary' : 'ghost'}
          size="sm"
          onClick={() => handleModeChange('upload')}
          disabled={disabled}
          className="h-7 text-xs"
        >
          <Upload className="h-3 w-3 mr-1" />
          Upload
        </Button>
        <Button
          type="button"
          variant={effectiveMode === 'reference' ? 'secondary' : 'ghost'}
          size="sm"
          onClick={() => handleModeChange('reference')}
          disabled={disabled}
          className="h-7 text-xs"
        >
          <Link className="h-3 w-3 mr-1" />
          Reference
        </Button>
      </div>

      {/* Render appropriate input based on mode */}
      {effectiveMode === 'upload' ? (
        <FileInput
          value={!isPartialReference(value) ? value : ''}
          onChange={handleFileChange}
          disabled={disabled}
          placeholder={placeholder || 'Upload a file'}
        />
      ) : (
        <div className="space-y-1">
          <AutocompleteInput
            value={value}
            onChange={handleReferenceChange}
            placeholder="{{steps['stepId'].outputs.file}}"
            type="text"
            typeHint={typeHint}
          />
          <p className="text-xs text-muted-foreground">
            Reference a file from a previous step output
          </p>
        </div>
      )}
    </div>
  );
}
