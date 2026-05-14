import { useState, useContext, useMemo } from 'react';
import { Input } from '@/shared/components/ui/input';
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
import { cn } from '@/lib/utils';

/**
 * Extract step ID from a reference path like "steps['stepId'].outputs.field"
 */
function extractStepIdFromPath(path: string): string | null {
  const match = path.match(/steps\['([^']+)'\]/);
  return match ? match[1] : null;
}

/**
 * Extract field path from a reference path like "steps['stepId'].outputs.field"
 * Returns the part after ".outputs" (e.g., "field" or "field.subfield")
 */
function extractFieldPathFromPath(path: string): string | null {
  const match = path.match(/\.outputs\.?(.*)$/);
  return match ? match[1] || 'outputs' : null;
}

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
  enumOptions?: string[];
  className?: string;
  /** Hide the reference mode toggle button (for testing/immediate-only contexts) */
  hideReferenceToggle?: boolean;
  /** Allow setting literal null for nullable-compatible immediate values */
  allowNull?: boolean;
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
}: MappingValueInputProps) {
  const [isPickerOpen, setIsPickerOpen] = useState(false);
  const [isTemplateEditorOpen, setIsTemplateEditorOpen] = useState(false);
  const { previousSteps } = useContext(NodeFormContext);

  const isReference = valueType === 'reference';
  const lowerFieldType = fieldType?.toLowerCase() || 'text';
  const lowerFieldName = fieldName?.toLowerCase() || '';

  const isTemplate = valueType === 'template';
  const isComposite = valueType === 'composite';
  const stringValue =
    value === null || value === undefined ? '' : String(value);
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
      lowerFieldName.includes('message')
    ) {
      return true;
    }
    return false;
  }, [lowerFieldType, lowerFieldName, isTemplate]);

  // Look up step info from the reference path
  const stepInfo = useMemo(() => {
    if (!isReference || !stringValue)
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    const stepId = extractStepIdFromPath(stringValue);
    if (!stepId)
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    const step = previousSteps.find((s) => s.id === stepId);
    const fieldPath = extractFieldPathFromPath(stringValue);
    return {
      stepName: step?.name,
      stepId,
      fieldPath,
    };
  }, [isReference, stringValue, previousSteps]);

  // Cycle: immediate → template → reference → composite → immediate
  const handleModeToggle = () => {
    if (valueType === 'immediate') {
      onValueTypeChange('template');
      onChange('');
    } else if (valueType === 'template') {
      onValueTypeChange('reference');
      onChange('');
    } else if (valueType === 'reference') {
      onValueTypeChange('composite');
      onChange('');
    } else {
      // composite → immediate
      onValueTypeChange('immediate');
      onChange('');
    }
  };

  // Handle variable selection from picker
  const handleVariableSelect = (variable: VariableSuggestion) => {
    onValueTypeChange('reference');
    onChange(variable.value);
  };

  // Handle removing reference
  const handleRemoveReference = () => {
    onValueTypeChange('immediate');
    onChange('');
  };

  // Render the appropriate input based on field type and value mode
  const renderInput = () => {
    // Reference mode - show pill or empty state
    if (isReference) {
      if (stringValue) {
        return (
          <div className="flex-1 flex items-center min-h-9 px-2 py-1 bg-muted/30 rounded-md border">
            <ReferencePill
              path={stringValue}
              stepName={stepInfo.stepName}
              fieldPath={stepInfo.fieldPath ?? undefined}
              onRemove={handleRemoveReference}
              disabled={disabled}
            />
          </div>
        );
      } else {
        return (
          <button
            type="button"
            onClick={() => setIsPickerOpen(true)}
            disabled={disabled}
            className={cn(
              'flex-1 flex items-center justify-center min-h-9 px-3 py-2',
              'text-sm text-muted-foreground',
              'bg-muted/30 border border-dashed rounded-md',
              'hover:bg-muted/50 hover:border-muted-foreground/50 transition-colors',
              disabled && 'opacity-50 cursor-not-allowed'
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
        <div className="flex-1 flex items-center min-h-9 px-3 py-1 bg-green-50 dark:bg-green-950/30 rounded-md border border-green-200 dark:border-green-800">
          <CompositeIcon className="h-4 w-4 text-green-600 dark:text-green-400 mr-2 shrink-0" />
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
          className="flex-1 font-mono focus-visible:ring-0 focus-visible:ring-offset-0 border-0 shadow-none"
        />
      );
    }

    // Immediate mode - render based on field type
    if (isNullValue) {
      return (
        <div className="flex-1 flex items-center justify-between min-h-9 px-3">
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
        <div className="flex-1 flex items-center min-h-9 px-3">
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
              <SelectItem key={option} value={option}>
                {option}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      );
    }

    // JSON, object, array types - use regular text input (same height as other fields)
    if (
      lowerFieldType === 'textarea' ||
      lowerFieldType === 'json' ||
      lowerFieldType === 'object' ||
      lowerFieldType === 'array'
    ) {
      return (
        <Input
          type="text"
          value={stringValue}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          disabled={disabled}
          className="flex-1 font-mono focus-visible:ring-0 focus-visible:ring-offset-0 border-0 shadow-none"
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
          className="flex-1 focus-visible:ring-0 focus-visible:ring-offset-0 border-0 shadow-none"
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
        className="flex-1 focus-visible:ring-0 focus-visible:ring-offset-0 border-0 shadow-none"
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
          <div className="flex-1 flex items-center h-9 border border-input rounded-md focus-within:ring-1 focus-within:ring-ring bg-background overflow-hidden">
            {renderInput()}
          </div>
        ) : (
          renderInput()
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
            className="h-9 w-9 shrink-0 text-muted-foreground hover:text-primary hover:bg-primary/10"
            onClick={() => setIsTemplateEditorOpen(true)}
            disabled={disabled}
            title="Open template editor"
          >
            <Icons.maximize className="h-4 w-4" />
          </Button>
        )}
        {/* Single toggle cycling: immediate → template → reference → immediate */}
        {!hideReferenceToggle && (
          <ModeToggleButton
            mode={valueType}
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
