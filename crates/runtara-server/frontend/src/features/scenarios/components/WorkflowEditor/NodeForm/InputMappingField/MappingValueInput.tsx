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

interface MappingValueInputProps {
  value: string;
  onChange: (value: string) => void;
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
}: MappingValueInputProps) {
  const [isPickerOpen, setIsPickerOpen] = useState(false);
  const [isTemplateEditorOpen, setIsTemplateEditorOpen] = useState(false);
  const { previousSteps } = useContext(NodeFormContext);

  const isReference = valueType === 'reference';
  const lowerFieldType = fieldType?.toLowerCase() || 'text';
  const lowerFieldName = fieldName?.toLowerCase() || '';

  const isTemplate = valueType === 'template';
  const isComposite = valueType === 'composite';

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
    if (!isReference || !value)
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    const stepId = extractStepIdFromPath(value);
    if (!stepId)
      return { stepName: undefined, stepId: undefined, fieldPath: undefined };
    const step = previousSteps.find((s) => s.id === stepId);
    const fieldPath = extractFieldPathFromPath(value);
    return {
      stepName: step?.name,
      stepId,
      fieldPath,
    };
  }, [isReference, value, previousSteps]);

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
      if (value) {
        return (
          <div className="flex-1 flex items-center min-h-9 px-2 py-1 bg-muted/30 rounded-md border">
            <ReferencePill
              path={value}
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
      return (
        <div className="flex-1 flex items-center min-h-9 px-3 py-1 bg-green-50 dark:bg-green-950/30 rounded-md border border-green-200 dark:border-green-800">
          <Icons.braces className="h-4 w-4 text-green-600 dark:text-green-400 mr-2 shrink-0" />
          <span className="text-sm text-green-700 dark:text-green-300">
            Composite object — configure below
          </span>
        </div>
      );
    }

    // Template mode - show text input for template string
    if (isTemplate) {
      return (
        <Input
          type="text"
          value={value || ''}
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

    // Boolean field
    if (lowerFieldType === 'boolean' || lowerFieldType === 'bool') {
      const boolValue = value === 'true';
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
          value={value || ''}
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
          value={value || ''}
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
          value={value || ''}
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
        value={value || ''}
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
        {/* Template editor expand button - shown for template mode or template-capable fields in immediate mode */}
        {showTemplateEditor && !isReference && !isComposite && (
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
          value={value}
          onChange={onChange}
          fieldName={fieldName}
          placeholder={placeholder}
        />
      )}
    </>
  );
}
