/* eslint-disable react-refresh/only-export-components */
/**
 * Dialog for adding a new custom field to the input mapping.
 */

import { useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
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

interface AddCustomFieldDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (fieldName: string, typeHint: string) => void;
  existingFieldNames: Set<string>;
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

    // Validate
    if (!trimmedName) {
      setError('Field name is required');
      return;
    }

    // Check for valid identifier (alphanumeric, underscores, starting with letter or underscore)
    if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(trimmedName)) {
      setError(
        'Field name must start with a letter or underscore and contain only letters, numbers, and underscores'
      );
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
            {error && <p className="text-sm text-destructive">{error}</p>}
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
