import { useState, useContext, useMemo, useEffect, useRef } from 'react';
import { Input } from '@/shared/components/ui/input';
import { Textarea } from '@/shared/components/ui/textarea';
import { coerceValueForMode, nextValueMode } from './value-mode';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Button } from '@/shared/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Icons } from '@/shared/components/icons';
import { ReferencePill } from './ReferencePill';
import { ModeToggleButton } from './ModeToggleButton';
import { VariablePickerModal } from './VariablePickerModal';
import { TemplateEditorModal } from './TemplateEditorModal';
import { VariableSuggestion } from '../InputMappingValueField/VariableSuggestions';
import { NodeFormContext } from '../NodeFormContext';
import {
  describeStepReference,
  referenceTypeMismatch,
  resolveReferenceType,
  validateReferencePath,
} from '../reference-type';
import { cn } from '@/lib/utils';

export type ValueMode = 'immediate' | 'reference' | 'template' | 'composite';

type MappingInputValue = string | number | boolean | null | undefined;

interface MappingValueInputProps {
  value: MappingInputValue;
  onChange: (value: string | null) => void;
  valueType: ValueMode;
  onValueTypeChange: (type: ValueMode) => void;
  fieldType?: string;
  /** Field name - used to determine if template editor should be shown */
  fieldName?: string;
  placeholder?: string;
  disabled?: boolean;
  enumOptions?: Array<string | { value: string; label: string }>;
  className?: string;
  /** Hide the reference mode toggle button (for testing/immediate-only contexts) */
  hideReferenceToggle?: boolean;
  /** Allow setting literal null for nullable-compatible immediate values */
  allowNull?: boolean;
  /**
   * ReferenceValue.default — fallback used at runtime when the referenced
   * path is missing or null. Only shown in reference mode.
   */
  defaultValue?: unknown;
  /**
   * Called when the fallback value changes. Pass-through of `undefined`
   * removes the key from the entry. When omitted, the fallback editor is
   * not rendered (for call sites without entry default semantics).
   */
  onDefaultValueChange?: (value: string | undefined) => void;
  /**
   * The consumer stringifies scalar reference values at runtime (Finish
   * outputs with a "string" type hint) — suppress the scalar→string
   * type-mismatch warning for those call sites.
   */
  scalarsCoerceToString?: boolean;
  /**
   * Modes this consumer actually supports, in case the runtime does not honour
   * all four. Defaults to every mode.
   */
  modes?: readonly ValueMode[];
}

function fieldTypeSupportsNull(fieldType: string): boolean {
  return (
    fieldType === 'string' ||
    fieldType === 'text' ||
    fieldType === 'str' ||
    fieldType === 'textarea' ||
    fieldType === 'json' ||
    fieldType === 'object' ||
    fieldType === 'array' ||
    fieldType === 'any' ||
    fieldType === 'unknown' ||
    fieldType.startsWith('array<') ||
    fieldType.startsWith('[') ||
    fieldType.includes('[]') ||
    fieldType.startsWith('{')
  );
}

/** Minimum visible rows for a long-form value editor. */
const LONG_FORM_MIN_ROWS = 3;

/** Autosize ceiling, past which the textarea scrolls internally. */
const LONG_FORM_MAX_HEIGHT_PX = 320;

/**
 * Field types whose values are routinely longer than one line: prose prompts,
 * SQL, and JSON/object/array literals. These get an autosizing textarea rather
 * than a 36px single-line input.
 */
function isLongFormFieldType(lowerFieldType: string): boolean {
  return (
    lowerFieldType === 'textarea' ||
    lowerFieldType === 'json' ||
    lowerFieldType === 'object' ||
    lowerFieldType === 'array'
  );
}

function isArrayFieldType(fieldType: string): boolean {
  return (
    fieldType === 'array' ||
    fieldType.startsWith('array<') ||
    fieldType.startsWith('[') ||
    fieldType.includes('[]')
  );
}

/**
 * Composite input component for mapping values
 * Supports both immediate (literal) values and reference (variable path) values
 */
export function MappingValueInput({
  value,
  onChange,
  valueType,
  onValueTypeChange,
  fieldType = 'text',
  fieldName,
  placeholder,
  disabled = false,
  enumOptions,
  className,
  hideReferenceToggle = false,
  allowNull = false,
  defaultValue,
  onDefaultValueChange,
  scalarsCoerceToString = false,
  modes,
}: MappingValueInputProps) {
  const [isPickerOpen, setIsPickerOpen] = useState(false);
  const [isTemplateEditorOpen, setIsTemplateEditorOpen] = useState(false);
  const {
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
    nodeId,
  } = useContext(NodeFormContext);

  const isReference = valueType === 'reference';
  const lowerFieldType = fieldType?.toLowerCase() || 'text';
  const lowerFieldName = fieldName?.toLowerCase() || '';

  const isTemplate = valueType === 'template';
  const isComposite = valueType === 'composite';
  const stringValue =
    value === null || value === undefined ? '' : String(value);
  // Display form of the reference fallback (defaultValue). Non-string
  // JSON-authored values are shown serialized; edits store the raw string
  // (the reference type hint coerces it at runtime).
  const defaultValueString =
    defaultValue === undefined
      ? ''
      : typeof defaultValue === 'string'
        ? defaultValue
        : JSON.stringify(defaultValue);
  const isLongForm = isLongFormFieldType(lowerFieldType);

  // Autosize the long-form editor to its content, between LONG_FORM_MIN_ROWS
  // and a cap that keeps a tall value from pushing the form's actions
  // off-screen — past that the textarea scrolls internally and can still be
  // dragged taller (resize-y) or opened in the template editor.
  const autosizeRef = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => {
    const el = autosizeRef.current;
    if (!el || !isLongForm) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, LONG_FORM_MAX_HEIGHT_PX)}px`;
  }, [stringValue, isLongForm]);

  const isNullValue = value === null;
  const canSetNull =
    allowNull &&
    valueType === 'immediate' &&
    fieldTypeSupportsNull(lowerFieldType);

  // Determine if we should show the template editor expand button
  const showTemplateEditor = useMemo(() => {
    // Always show for template mode
    if (isTemplate) return true;
    // Show for textarea type
    if (lowerFieldType === 'textarea') return true;
    // Show for fields with template-related names
    if (
      lowerFieldName.includes('template') ||
      lowerFieldName.includes('prompt') ||
      lowerFieldName.includes('body') ||
      lowerFieldName.includes('content') ||
      lowerFieldName.includes('message') ||
      lowerFieldName === 'sql'
    ) {
      return true;
    }
    return false;
  }, [lowerFieldType, lowerFieldName, isTemplate]);

  // Look up step info from the reference path (both path spellings)
  const stepInfo = useMemo(
    () =>
      isReference && stringValue
        ? describeStepReference(stringValue, previousSteps)
        : {},
    [isReference, stringValue, previousSteps]
  );

  // Resolved type of the referenced value, for the pill's badge/icon — and
  // an inline existence error when the path provably cannot resolve.
  const { referenceType, referenceError } = useMemo(() => {
    if (!isReference || !stringValue) {
      return { referenceType: undefined, referenceError: null };
    }
    const context = {
      previousSteps,
      inputSchemaFields,
      variables,
      insideSplitScope: isInsideSplit,
      insideWaitScope: isInsideWaitScope,
      splitItemSchemaFields,
      currentStepId: nodeId,
    };
    return {
      referenceType: resolveReferenceType(stringValue, context),
      referenceError: validateReferencePath(stringValue, context),
    };
  }, [
    isReference,
    stringValue,
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
    nodeId,
  ]);

  // Cycle: immediate → template → reference → composite → immediate.
  // The value carries across; see coerceValueForMode. Switching how a value is
  // interpreted is not a request to delete it.
  const handleModeToggle = () => {
    const next = nextValueMode(valueType, modes);
    const carried = coerceValueForMode(value, valueType, next, fieldType);
    onValueTypeChange(next);
    if (carried.changed) {
      onChange(carried.value as string | null);
    }
  };

  // Handle variable selection from picker
  const handleVariableSelect = (variable: VariableSuggestion) => {
    onValueTypeChange('reference');
    onChange(variable.value);
  };

  // Handle removing reference. Clearing the value is the explicit intent of
  // the button; dropping out of reference mode as well is not, and left the
  // author in a literal text box when they wanted to pick a different path.
  // Matches CompositeValueItem, which already stays in reference mode.
  const handleRemoveReference = () => {
    onChange('');
  };

  // Render the appropriate input based on field type and value mode
  const renderInput = () => {
    // Reference mode - show pill or empty state
    if (isReference) {
      if (stringValue) {
        const typeMismatch = referenceError
          ? null
          : referenceTypeMismatch(referenceType, fieldType, {
              scalarsCoerceToString,
            });
        return (
          <div className="min-w-0 flex-1">
            <div
              className={cn(
                'flex min-h-9 items-center rounded-md border bg-muted/30 px-2 py-1',
                referenceError && 'border-destructive'
              )}
            >
              <ReferencePill
                path={stringValue}
                type={referenceType}
                stepName={stepInfo.stepName}
                fieldPath={stepInfo.fieldPath ?? undefined}
                onRemove={handleRemoveReference}
                disabled={disabled}
              />
            </div>
            {referenceError && (
              <p
                className="mt-0.5 text-2xs text-destructive"
                data-testid="reference-error"
              >
                {referenceError}
              </p>
            )}
            {typeMismatch && (
              <p className="mt-0.5 text-2xs text-warning">{typeMismatch}</p>
            )}
            {onDefaultValueChange && (
              // The explanation lives on the info icon rather than a caption:
              // as a caption it wrapped to three lines on every reference row,
              // which is most of the row's height for text that never changes.
              <div className="mt-1 flex items-center gap-1">
                <Input
                  type="text"
                  value={defaultValueString}
                  onChange={(e) =>
                    onDefaultValueChange(
                      e.target.value === '' ? undefined : e.target.value
                    )
                  }
                  placeholder="Fallback value"
                  disabled={disabled}
                  className="h-7 font-mono text-xs"
                />
                <Icons.info
                  className="size-3 shrink-0 cursor-help text-muted-foreground"
                  aria-label="Fallback value help"
                  title="Used when the referenced path is missing or null"
                />
              </div>
            )}
          </div>
        );
      } else {
        return (
          <button
            type="button"
            onClick={() => setIsPickerOpen(true)}
            disabled={disabled}
            className={cn(
              'flex min-h-9 flex-1 items-center justify-center px-3 py-2',
              'text-sm text-muted-foreground',
              'rounded-md border border-dashed bg-muted/30',
              'transition-colors hover:border-muted-foreground/50 hover:bg-muted/50',
              disabled && 'cursor-not-allowed opacity-50'
            )}
          >
            Click to select a variable...
          </button>
        );
      }
    }

    // Composite mode - show indicator (parent renders the actual editor)
    if (isComposite) {
      const isArrayComposite = isArrayFieldType(lowerFieldType);
      const CompositeIcon = isArrayComposite ? Icons.list : Icons.braces;
      return (
        <div className="flex min-h-9 flex-1 items-center rounded-md border border-green-200 bg-green-50 px-3 py-1 dark:border-green-800 dark:bg-green-950/30">
          <CompositeIcon className="mr-2 size-4 shrink-0 text-green-600 dark:text-green-400" />
          <span className="text-sm text-green-700 dark:text-green-300">
            {isArrayComposite
              ? 'Composite array - configure below'
              : 'Composite object - configure below'}
          </span>
        </div>
      );
    }

    // Template mode - show text input for template string
    if (isTemplate) {
      return (
        <Input
          type="text"
          value={stringValue}
          onChange={(e) => onChange(e.target.value)}
          placeholder={
            placeholder ||
            'e.g., Bearer {{ steps.my_conn.outputs.parameters.api_key }}'
          }
          disabled={disabled}
          className="flex-1 border-0 font-mono shadow-none focus-visible:ring-0 focus-visible:ring-offset-0"
        />
      );
    }

    // Immediate mode - render based on field type
    if (isNullValue) {
      return (
        <div className="flex min-h-9 flex-1 items-center justify-between px-3">
          <span className="font-mono text-sm text-muted-foreground">null</span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={() => onChange('')}
            disabled={disabled}
            title="Clear null value"
          >
            Clear
          </Button>
        </div>
      );
    }

    // Boolean field
    if (lowerFieldType === 'boolean' || lowerFieldType === 'bool') {
      const boolValue = value === true || stringValue === 'true';
      return (
        <div className="flex min-h-9 flex-1 items-center px-3">
          <Checkbox
            checked={boolValue}
            onCheckedChange={(checked) => onChange(String(checked))}
            disabled={disabled}
          />
          <span className="ml-2 text-sm text-muted-foreground">
            {boolValue ? 'True' : 'False'}
          </span>
        </div>
      );
    }

    // Enum/Select field
    if (enumOptions && enumOptions.length > 0) {
      return (
        <Select
          value={stringValue}
          onValueChange={onChange}
          disabled={disabled}
        >
          <SelectTrigger className="flex-1">
            <SelectValue placeholder={placeholder || 'Select an option...'} />
          </SelectTrigger>
          <SelectContent>
            {enumOptions.map((option) => (
              <SelectItem
                key={typeof option === 'string' ? option : option.value}
                value={typeof option === 'string' ? option : option.value}
              >
                {typeof option === 'string' ? option : option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      );
    }

    // Long-form values (prose prompts, SQL, JSON literals) get a real
    // multi-line control that grows with its content. A 36px single-line box
    // for an AI system prompt is unusable, and the reason authors moved this
    // kind of editing out of the form entirely.
    if (isLongFormFieldType(lowerFieldType)) {
      return (
        <Textarea
          ref={autosizeRef}
          value={stringValue}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          disabled={disabled}
          rows={LONG_FORM_MIN_ROWS}
          spellCheck={lowerFieldType === 'textarea'}
          className={cn(
            'min-h-0 flex-1 resize-y border-0 py-2 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0',
            // Structured values read better monospaced; prose does not.
            lowerFieldType !== 'textarea' && 'font-mono'
          )}
        />
      );
    }

    // Number input
    if (
      lowerFieldType === 'number' ||
      lowerFieldType === 'integer' ||
      lowerFieldType === 'int' ||
      lowerFieldType === 'double' ||
      lowerFieldType === 'float'
    ) {
      return (
        <Input
          type="number"
          value={stringValue}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          disabled={disabled}
          className="flex-1 border-0 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0"
        />
      );
    }

    // Default: text input
    return (
      <Input
        type="text"
        value={stringValue}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        className="flex-1 border-0 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0"
      />
    );
  };

  // Check if we need the grouped wrapper (for inputs that aren't full-width components)
  const needsGroupedWrapper =
    !isReference &&
    !isComposite &&
    (isTemplate ||
      (lowerFieldType !== 'boolean' &&
        lowerFieldType !== 'bool' &&
        !(enumOptions && enumOptions.length > 0)));

  return (
    <>
      <div className={cn('flex items-start gap-2', className)}>
        {needsGroupedWrapper ? (
          <div
            className={cn(
              'flex flex-1 overflow-hidden rounded-md border border-input bg-background focus-within:ring-1 focus-within:ring-ring',
              // h-9 would clamp the autosizing textarea back to one line.
              isLongForm ? 'items-stretch' : 'h-9 items-center'
            )}
          >
            {renderInput()}
          </div>
        ) : (
          // Grow the ungrouped controls (checkbox, enum select) too, so the
          // trailing mode toggle lands at the same x on every row instead of
          // stepping in and out with the control's natural width.
          <div className="min-w-0 flex-1">{renderInput()}</div>
        )}
        {canSetNull && !isNullValue && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-9 shrink-0 px-2 font-mono text-xs text-muted-foreground hover:text-foreground"
            onClick={() => onChange(null)}
            disabled={disabled}
            title="Set literal null"
          >
            null
          </Button>
        )}
        {/* Template editor expand button - shown for template mode or template-capable fields in immediate mode */}
        {showTemplateEditor && !isReference && !isComposite && !isNullValue && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-9 shrink-0 text-muted-foreground hover:bg-primary/10 hover:text-primary"
            onClick={() => setIsTemplateEditorOpen(true)}
            disabled={disabled}
            title="Open template editor"
          >
            <Icons.maximize className="size-4" />
          </Button>
        )}
        {/* Single toggle cycling immediate → template → reference → composite,
            restricted to `modes` where the consumer supports fewer. */}
        {!hideReferenceToggle && (
          <ModeToggleButton
            mode={valueType}
            nextMode={nextValueMode(valueType, modes)}
            onClick={handleModeToggle}
            disabled={disabled}
          />
        )}
      </div>

      {/* Render variable picker modal when toggle is visible OR when already in reference mode */}
      {(!hideReferenceToggle || isReference) && (
        <VariablePickerModal
          open={isPickerOpen}
          onOpenChange={setIsPickerOpen}
          onSelect={handleVariableSelect}
        />
      )}

      {/* Template editor modal */}
      {showTemplateEditor && (
        <TemplateEditorModal
          open={isTemplateEditorOpen}
          onOpenChange={setIsTemplateEditorOpen}
          value={stringValue}
          onChange={(nextValue) => onChange(nextValue)}
          fieldName={fieldName}
          placeholder={placeholder}
        />
      )}
    </>
  );
}
