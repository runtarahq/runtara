/**
 * Row component for custom (user-defined) fields in the input mapping editor.
 * Unlike FieldRow, this allows editing the field name and type.
 */

import React, { useState, useEffect } from 'react';
import { Icons } from '@/shared/components/icons';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { FieldError } from '@/shared/components/ui/form';
import { TableCell, TableRow } from '@/shared/components/ui/table';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { useNodeFormStore } from '@/features/workflows/stores/nodeFormStore';
import { MappingValueInput, ValueMode } from './MappingValueInput';
import { FileInputWithReferences } from './FileInputWithReferences';
import { ObjectMappingEditor } from './ObjectMappingEditor';
import { CUSTOM_FIELD_TYPES, customFieldTypeLabel } from './custom-field-types';
import { TOGGLE_GUTTER_CLASS, VALUE_CELL_CLASS } from './value-cell-layout';

interface CustomFieldRowProps {
  nodeId: string;
  fieldName: string;
  fieldType: string;
  onRemove: () => void;
  onFieldChange: () => void;
  onRename: (oldName: string, newName: string) => void;
  existingFieldNames: Set<string>;
  hideReferenceToggle?: boolean;
}

export function CustomFieldRow({
  nodeId,
  fieldName,
  fieldType,
  onRemove,
  onFieldChange,
  onRename,
  existingFieldNames,
  hideReferenceToggle = false,
}: CustomFieldRowProps) {
  const entry = useNodeFormStore((s) => s.getFieldEntry(nodeId, fieldName));
  const setFieldValue = useNodeFormStore((s) => s.setFieldValue);
  const setFieldValueType = useNodeFormStore((s) => s.setFieldValueType);
  const setFieldTypeHint = useNodeFormStore((s) => s.setFieldTypeHint);
  const setFieldDefaultValue = useNodeFormStore((s) => s.setFieldDefaultValue);

  // Local state for editing the name
  const [isEditingName, setIsEditingName] = useState(false);
  const [editedName, setEditedName] = useState(fieldName);
  const [nameError, setNameError] = useState<string | null>(null);

  // Sync editedName when fieldName prop changes
  useEffect(() => {
    setEditedName(fieldName);
  }, [fieldName]);

  const value = entry ? entry.value : '';
  const valueType = (entry?.valueType ?? 'immediate') as ValueMode;

  const handleValueChange = (newValue: string | null) => {
    setFieldValue(nodeId, fieldName, newValue);
    onFieldChange();
  };

  const handleValueTypeChange = (newType: ValueMode) => {
    setFieldValueType(nodeId, fieldName, newType);
    onFieldChange();
  };

  const handleTypeChange = (newType: string) => {
    setFieldTypeHint(nodeId, fieldName, newType);
    onFieldChange();
  };

  const handleDefaultValueChange = (newDefault: string | undefined) => {
    setFieldDefaultValue(nodeId, fieldName, newDefault);
    onFieldChange();
  };

  const handleNameEdit = () => {
    setIsEditingName(true);
    setNameError(null);
  };

  const handleNameSave = () => {
    const trimmedName = editedName.trim();

    // Validate
    if (!trimmedName) {
      setNameError('Name is required');
      return;
    }

    if (trimmedName !== fieldName && existingFieldNames.has(trimmedName)) {
      setNameError('Field name already exists');
      return;
    }

    // Rename if changed
    if (trimmedName !== fieldName) {
      onRename(fieldName, trimmedName);
    }

    setIsEditingName(false);
    setNameError(null);
  };

  const handleNameCancel = () => {
    setEditedName(fieldName);
    setIsEditingName(false);
    setNameError(null);
  };

  const handleNameKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      handleNameSave();
    } else if (e.key === 'Escape') {
      handleNameCancel();
    }
  };

  // Get the display type from typeHint
  const getDisplayType = () => {
    const typeHint = entry?.typeHint;
    // Don't treat 'auto' as a valid type - fall back to fieldType or 'string'
    if (!typeHint || typeHint === 'auto') {
      return fieldType || 'string';
    }
    const typeInfo = CUSTOM_FIELD_TYPES.find((t) => t.value === typeHint);
    return typeInfo?.value || fieldType || 'string';
  };

  // Get short label for compact display
  const getTypeLabel = () => customFieldTypeLabel(getDisplayType());

  return (
    <TableRow className="bg-warning/5 hover:bg-muted/30">
      {/* Name column - editable, with the type selector beside it.
          A custom field's type is editable, so unlike a schema field it cannot
          collapse into a hint; it sits under the name instead of holding open
          a column the docked panel cannot spare. */}
      <TableCell className="overflow-hidden pt-3 align-top">
        {isEditingName ? (
          <div>
            <Input
              value={editedName}
              onChange={(e) => setEditedName(e.target.value)}
              onKeyDown={handleNameKeyDown}
              onBlur={handleNameSave}
              autoFocus
              className="h-7 text-sm"
            />
            {nameError && <FieldError className="mt-1">{nameError}</FieldError>}
          </div>
        ) : (
          <button
            type="button"
            onClick={handleNameEdit}
            className="block text-left text-sm text-foreground transition-colors hover:text-primary"
            title="Click to rename"
          >
            {fieldName}
          </button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="mt-1 flex cursor-pointer items-center gap-1 rounded bg-muted/40 px-1.5 py-0.5 font-mono text-2xs text-muted-foreground transition-colors hover:bg-muted/60"
            >
              <span>{getTypeLabel()}</span>
              <Icons.chevronDown className="size-3" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {CUSTOM_FIELD_TYPES.map((type) => (
              <DropdownMenuItem
                key={type.value}
                onClick={() => handleTypeChange(type.value)}
                className="text-xs"
              >
                {type.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </TableCell>

      {/* Value column */}
      <TableCell className={VALUE_CELL_CLASS}>
        {getDisplayType() === 'file' ? (
          <div className={TOGGLE_GUTTER_CLASS}>
            <FileInputWithReferences
              value={typeof value === 'string' ? value : ''}
              onChange={handleValueChange}
              placeholder="Upload a file"
            />
          </div>
        ) : valueType === 'composite' ? (
          // Render the structure editor right here. MappingValueInput's
          // composite state is only a banner reading 'configure below', and
          // unlike a schema field row this row has no expansion sibling — so
          // 'below' was nothing at all and the value was unreachable.
          <ObjectMappingEditor
            value={
              typeof value === 'object' && value !== null
                ? (value as never)
                : ({} as never)
            }
            valueType="composite"
            untyped
            onChange={(next) => handleValueChange(next as never)}
            onValueTypeChange={(next) => handleValueTypeChange(next as never)}
            onClose={() => handleValueTypeChange('immediate' as never)}
          />
        ) : (
          <MappingValueInput
            value={
              typeof value === 'object' && value !== null
                ? JSON.stringify(value, null, 2)
                : value
            }
            onChange={handleValueChange}
            valueType={valueType}
            onValueTypeChange={handleValueTypeChange}
            fieldType={getDisplayType()}
            allowNull
            placeholder="Enter value..."
            hideReferenceToggle={hideReferenceToggle}
            defaultValue={entry?.defaultValue}
            onDefaultValueChange={handleDefaultValueChange}
          />
        )}
      </TableCell>

      {/* Actions column - always show remove for custom fields */}
      <TableCell className="pt-2 align-top">
        <Button
          type="button"
          variant="quietDestructive"
          size="icon-sm"
          onClick={onRemove}
          title="Remove custom field"
        >
          <Icons.remove className="size-3.5" />
        </Button>
      </TableCell>
    </TableRow>
  );
}
