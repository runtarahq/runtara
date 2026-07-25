/**
 * Dialog for adding a new custom field to the input mapping.
 */

import { useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { FieldError } from '@/shared/components/ui/form';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { CUSTOM_FIELD_TYPES } from './custom-field-types';

interface AddCustomFieldDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (fieldName: string, typeHint: string) => void;
  existingFieldNames: Set<string>;
}

/**
 * Validate a custom mapping key. Returns an error message or null when valid.
 *
 * Dot-separated keys are legal: the DSL validates them by root segment
 * (validation.rs) and the runtime builds nested objects from them
 * (direct_json.rs insert_nested), so each dot-separated segment must be a
 * valid identifier.
 */
export function validateCustomFieldName(name: string): string | null {
  if (!name) {
    return 'Field name is required';
  }
  const segments = name.split('.');
  const segmentPattern = /^[a-zA-Z_][a-zA-Z0-9_]*$/;
  if (!segments.every((segment) => segmentPattern.test(segment))) {
    return 'Each dot-separated segment must start with a letter or underscore and contain only letters, numbers, and underscores';
  }
  return null;
}

export function AddCustomFieldDialog({
  open,
  onOpenChange,
  onAdd,
  existingFieldNames,
}: AddCustomFieldDialogProps) {
  const [fieldName, setFieldName] = useState('');
  const [fieldType, setFieldType] = useState('string');
  const [error, setError] = useState<string | null>(null);

  const handleClose = () => {
    setFieldName('');
    setFieldType('string');
    setError(null);
    onOpenChange(false);
  };

  const handleAdd = () => {
    const trimmedName = fieldName.trim();

    // Validate (dot-separated segments build nested objects in the DSL)
    const validationError = validateCustomFieldName(trimmedName);
    if (validationError) {
      setError(validationError);
      return;
    }

    if (existingFieldNames.has(trimmedName)) {
      setError('A field with this name already exists');
      return;
    }

    onAdd(trimmedName, fieldType);
    handleClose();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleAdd();
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent className="sm:max-w-[400px]">
        <DialogHeader>
          <DialogTitle>Add Custom Parameter</DialogTitle>
          <DialogDescription>
            Add a custom parameter that is not defined in the operation schema.
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-4 py-4">
          <div className="grid gap-2">
            <Label htmlFor="fieldName">Parameter Name</Label>
            <Input
              id="fieldName"
              value={fieldName}
              onChange={(e) => {
                setFieldName(e.target.value);
                setError(null);
              }}
              onKeyDown={handleKeyDown}
              placeholder="my_parameter"
              autoFocus
            />
            <p className="text-xs text-muted-foreground">
              Use dots to build nested objects, e.g.{' '}
              <code className="rounded bg-muted px-1">payload.user.name</code>.
            </p>
            {error && <FieldError>{error}</FieldError>}
          </div>

          <div className="grid gap-2">
            <Label htmlFor="fieldType">Parameter Type</Label>
            <Select value={fieldType} onValueChange={setFieldType}>
              <SelectTrigger id="fieldType">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CUSTOM_FIELD_TYPES.map((type) => (
                  <SelectItem key={type.value} value={type.value}>
                    {type.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={handleClose}>
            Cancel
          </Button>
          <Button type="button" onClick={handleAdd}>
            Add Parameter
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
