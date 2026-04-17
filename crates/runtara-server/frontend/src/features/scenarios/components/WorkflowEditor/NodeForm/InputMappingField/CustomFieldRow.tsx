/**
 * Row component for custom (user-defined) fields in the input mapping editor.
 * Unlike FieldRow, this allows editing the field name and type.
 */

import React, { useState, useEffect } from 'react';
import { Icons } from '@/shared/components/icons';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { TableCell, TableRow } from '@/shared/components/ui/table';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { useNodeFormStore } from '@/features/scenarios/stores/nodeFormStore';
import { MappingValueInput, ValueMode } from './MappingValueInput';
import { FileInputWithReferences } from './FileInputWithReferences';

/** Available types for custom fields - values match API ValueType convention */
const CUSTOM_FIELD_TYPES = [
  { value: 'string', label: 'String' },
  { value: 'integer', label: 'Integer' },
  { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'json', label: 'JSON Object' },
  { value: 'json', label: 'Array' },
  { value: 'file', label: 'File' },
];

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

  // Local state for editing the name
  const [isEditingName, setIsEditingName] = useState(false);
  const [editedName, setEditedName] = useState(fieldName);
  const [nameError, setNameError] = useState<string | null>(null);

  // Sync editedName when fieldName prop changes
  useEffect(() => {
    setEditedName(fieldName);
  }, [fieldName]);

  const value = entry?.value ?? '';
  const valueType = (entry?.valueType ?? 'immediate') as ValueMode;

  const handleValueChange = (newValue: string) => {
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
  const getTypeLabel = () => {
    const currentType = getDisplayType();
    const typeInfo = CUSTOM_FIELD_TYPES.find((t) => t.value === currentType);
    return typeInfo?.label || currentType;
  };

  return (
    <TableRow className="hover:bg-muted/30 bg-amber-50/30 dark:bg-amber-950/10">
      {/* Type column - editable with dropdown */}
      <TableCell className="align-top pt-3">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="text-[11px] font-mono px-1.5 py-0.5 rounded text-muted-foreground bg-muted/40 hover:bg-muted/60 transition-colors cursor-pointer flex items-center gap-1"
            >
              <span>{getTypeLabel()}</span>
              <Icons.chevronDown className="h-3 w-3" />
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

      {/* Name column - editable */}
      <TableCell className="align-top pt-3 overflow-hidden">
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
            {nameError && (
              <p className="text-xs text-destructive mt-1">{nameError}</p>
            )}
          </div>
        ) : (
          <button
            type="button"
            onClick={handleNameEdit}
            className="text-sm text-slate-900/90 dark:text-slate-100 hover:text-primary transition-colors text-left"
            title="Click to rename"
          >
            {fieldName}
          </button>
        )}
      </TableCell>

      {/* Value column */}
      <TableCell>
        {getDisplayType() === 'file' ? (
          <FileInputWithReferences
            value={typeof value === 'string' ? value : ''}
            onChange={handleValueChange}
            placeholder="Upload a file"
          />
        ) : (
          <MappingValueInput
            value={
              typeof value === 'object'
                ? JSON.stringify(value, null, 2)
                : String(value)
            }
            onChange={handleValueChange}
            valueType={valueType}
            onValueTypeChange={handleValueTypeChange}
            fieldType={getDisplayType()}
            placeholder="Enter value..."
            hideReferenceToggle={hideReferenceToggle}
          />
        )}
      </TableCell>

      {/* Actions column - always show remove for custom fields */}
      <TableCell className="align-top pt-2">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 text-muted-foreground hover:text-destructive"
          onClick={onRemove}
          title="Remove custom field"
        >
          <Icons.remove className="h-3.5 w-3.5" />
        </Button>
      </TableCell>
    </TableRow>
  );
}
