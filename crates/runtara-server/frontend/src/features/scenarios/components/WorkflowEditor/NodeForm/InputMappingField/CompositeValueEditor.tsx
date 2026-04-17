/**
 * CompositeValueEditor - Main entry point for composite value editing.
 *
 * This component serves as the primary interface for editing composite values.
 * It determines whether the composite is an object or array and delegates to
 * the appropriate specialized editor.
 *
 * Can be used:
 * - Inline within field rows
 * - In modal dialogs for full-screen editing
 * - As a standalone editor panel
 */

import { useCallback, useMemo } from 'react';
import { Braces, List } from 'lucide-react';
import { cn } from '@/lib/utils';
import type {
  CompositeObjectValue,
  CompositeArrayValue,
} from '@/features/scenarios/stores/nodeFormStore';
import { CompositeObjectEditor } from './CompositeObjectEditor';
import { CompositeArrayEditor } from './CompositeArrayEditor';
import { validateCompositeValue } from './compositeValidation';

type CompositeMode = 'object' | 'array';

interface CompositeValueEditorProps {
  /** The composite value to edit */
  value: CompositeObjectValue | CompositeArrayValue;
  /** Called when the value changes */
  onChange: (value: CompositeObjectValue | CompositeArrayValue) => void;
  /** Called when the editor should be closed */
  onClose?: () => void;
  /** Called when the mode changes (object <-> array) */
  onModeChange?: (mode: CompositeMode) => void;
  /** Available reference paths for validation */
  availablePaths?: Set<string>;
  /** Current nesting depth */
  depth?: number;
  /** Custom title */
  title?: string;
  /** Whether to show mode switcher */
  showModeSwitcher?: boolean;
  /** Whether to show close button */
  showCloseButton?: boolean;
  /** Disable all editing */
  disabled?: boolean;
}

export function CompositeValueEditor({
  value,
  onChange,
  onClose,
  onModeChange,
  availablePaths,
  depth = 0,
  title,
  showModeSwitcher = true,
  showCloseButton = true,
  disabled = false,
}: CompositeValueEditorProps) {
  // Determine if value is object or array
  const isArray = Array.isArray(value);
  const mode: CompositeMode = isArray ? 'array' : 'object';

  // Validate the composite value
  const validationResult = useMemo(
    () => validateCompositeValue(value, availablePaths),
    [value, availablePaths]
  );

  // Handle mode switch
  const handleModeChange = useCallback(
    (newMode: CompositeMode) => {
      if (newMode === mode) return;

      // Warn if there's existing data
      const hasData = isArray
        ? value.length > 0
        : Object.keys(value).length > 0;
      if (hasData) {
        // In a real app, you might want to show a confirmation dialog
        // For now, we'll just switch and clear the data
        console.warn('Switching composite mode will clear existing data');
      }

      // Create empty value of new type
      const newValue = newMode === 'array' ? [] : {};
      onChange(newValue);
      onModeChange?.(newMode);
    },
    [mode, value, isArray, onChange, onModeChange]
  );

  // Determine the title to display
  const displayTitle =
    title || (isArray ? 'Composite Array' : 'Composite Object');

  return (
    <div className="flex flex-col h-full">
      {/* Mode switcher (if enabled) */}
      {showModeSwitcher && (
        <div className="flex gap-2 px-4 py-3 border-b shrink-0">
          <button
            type="button"
            onClick={() => handleModeChange('object')}
            disabled={disabled}
            className={cn(
              'flex-1 flex items-center justify-center gap-2 px-4 py-2',
              'text-sm border rounded-md transition-colors',
              mode === 'object'
                ? 'bg-green-50 border-green-300 text-green-700 dark:bg-green-950 dark:border-green-800 dark:text-green-400'
                : 'bg-background border-input text-muted-foreground hover:bg-muted/50',
              disabled && 'opacity-50 cursor-not-allowed'
            )}
          >
            <Braces className="h-4 w-4" />
            Composite Object
          </button>
          <button
            type="button"
            onClick={() => handleModeChange('array')}
            disabled={disabled}
            className={cn(
              'flex-1 flex items-center justify-center gap-2 px-4 py-2',
              'text-sm border rounded-md transition-colors',
              mode === 'array'
                ? 'bg-green-50 border-green-300 text-green-700 dark:bg-green-950 dark:border-green-800 dark:text-green-400'
                : 'bg-background border-input text-muted-foreground hover:bg-muted/50',
              disabled && 'opacity-50 cursor-not-allowed'
            )}
          >
            <List className="h-4 w-4" />
            Composite Array
          </button>
        </div>
      )}

      {/* Validation summary */}
      {!validationResult.isValid && (
        <div className="px-4 py-2 bg-destructive/10 border-b border-destructive/20">
          <p className="text-sm text-destructive">
            {validationResult.errors.length} validation{' '}
            {validationResult.errors.length === 1 ? 'error' : 'errors'}
          </p>
        </div>
      )}

      {/* Editor content */}
      <div className="flex-1 overflow-hidden">
        {isArray ? (
          <CompositeArrayEditor
            value={value as CompositeArrayValue}
            onChange={onChange as (value: CompositeArrayValue) => void}
            onClose={onClose}
            depth={depth}
            validationErrors={validationResult.errors}
            title={showModeSwitcher ? undefined : displayTitle}
            showCloseButton={!showModeSwitcher && showCloseButton}
            disabled={disabled}
          />
        ) : (
          <CompositeObjectEditor
            value={value as CompositeObjectValue}
            onChange={onChange as (value: CompositeObjectValue) => void}
            onClose={onClose}
            depth={depth}
            validationErrors={validationResult.errors}
            title={showModeSwitcher ? undefined : displayTitle}
            showCloseButton={!showModeSwitcher && showCloseButton}
            disabled={disabled}
          />
        )}
      </div>
    </div>
  );
}
