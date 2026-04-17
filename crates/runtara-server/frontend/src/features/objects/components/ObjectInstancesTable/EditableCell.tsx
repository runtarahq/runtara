import { useState, useEffect, useRef, useCallback, memo } from 'react';
import { Input } from '@/shared/components/ui/input';
import { Checkbox } from '@/shared/components/ui/checkbox';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { JsonEditor, githubLightTheme, githubDarkTheme } from 'json-edit-react';
import { Braces } from 'lucide-react';
import { formatDate } from '@/lib/utils';
import { Instance } from '@/generated/RuntaraRuntimeApi';

interface EditableCellProps {
  getValue: () => any;
  row: { original: Instance };
  column: { id: string };
  onUpdate: (instanceId: string, data: any) => void;
  dataType:
    | 'string'
    | 'integer'
    | 'boolean'
    | 'decimal'
    | 'timestamp'
    | 'json'
    | 'enum';
  enumValues?: string[];
  onFocus?: () => void;
  isEditing: boolean;
  setIsEditing: (editing: boolean) => void;
}

export const EditableCell = memo(function EditableCell({
  getValue,
  row,
  column,
  onUpdate,
  dataType,
  enumValues,
  onFocus,
  isEditing,
  setIsEditing,
}: EditableCellProps) {
  const initialValue = getValue();
  const [value, setValue] = useState(initialValue);
  const inputRef = useRef<HTMLInputElement>(null);
  const currentValueRef = useRef(value);
  const initialValueRef = useRef(initialValue);
  const isEditingRef = useRef(isEditing);
  const lastCommittedValueRef = useRef(initialValue);

  const normalizeValue = useCallback(
    (val: any) => {
      if (dataType === 'integer') {
        if (val === '' || val === null || val === undefined) return null;
        const parsed = parseInt(val, 10);
        return Number.isNaN(parsed) ? null : parsed;
      }

      if (dataType === 'decimal') {
        if (val === '' || val === null || val === undefined) return null;
        const parsed = parseFloat(val);
        return Number.isNaN(parsed) ? null : parsed;
      }

      return val;
    },
    [dataType]
  );

  // Keep isEditing ref in sync
  useEffect(() => {
    isEditingRef.current = isEditing;
  }, [isEditing]);

  // Keep refs in sync when not editing so dirty tracking stays accurate
  useEffect(() => {
    if (!isEditing) {
      initialValueRef.current = initialValue;
      const normalized = normalizeValue(initialValue);
      lastCommittedValueRef.current = normalized;
      setValue(initialValue);
      currentValueRef.current = initialValue;
    }
  }, [initialValue, isEditing, normalizeValue]);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isEditing]);

  const commitValue = useCallback(
    (rawValue: any) => {
      const payloadValue = normalizeValue(rawValue);

      if (Object.is(payloadValue, lastCommittedValueRef.current)) {
        return;
      }

      lastCommittedValueRef.current = payloadValue;

      onUpdate(row.original.id!, {
        properties: {
          [column.id]: payloadValue,
        },
      });
    },
    [column.id, normalizeValue, onUpdate, row.original.id]
  );

  const onBlur = () => {
    const normalizedCurrent = normalizeValue(currentValueRef.current);

    if (!Object.is(normalizedCurrent, lastCommittedValueRef.current)) {
      commitValue(currentValueRef.current);
    }
  };

  // Note: no cleanup commit on unmount — onBlur and handleClickOutside
  // already handle committing dirty values. An unmount cleanup here would
  // fire spurious API calls on deleted rows (producing 404 error toasts).

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      onBlur();
    } else if (e.key === 'Escape') {
      setValue(initialValueRef.current);
      currentValueRef.current = initialValueRef.current;
      commitValue(initialValueRef.current);
      setIsEditing(false);
    }
  };

  // JSON fields use a popover-based editor instead of inline editing.
  // Also detect object/array values regardless of declared type (e.g. JSONB[] or unmapped types).
  const isJsonValue =
    dataType === 'json' || (value != null && typeof value === 'object');
  if (isJsonValue) {
    return (
      <JsonCellEditor
        value={value}
        onUpdate={(newValue) => {
          setValue(newValue);
          currentValueRef.current = newValue;
          commitValue(newValue);
        }}
        onFocus={onFocus}
      />
    );
  }

  if (dataType === 'boolean') {
    return (
      <div className="flex h-10 w-full items-center justify-center">
        <Checkbox
          checked={!!value}
          onCheckedChange={(checked) => {
            setValue(checked);
            currentValueRef.current = checked;
            commitValue(checked);
            onFocus?.();
          }}
        />
      </div>
    );
  }

  if (isEditing) {
    if (dataType === 'enum' && enumValues) {
      return (
        <Select
          open={true}
          onOpenChange={(open) => {
            if (!open) setIsEditing(false);
          }}
          value={String(value || '')}
          onValueChange={(val) => {
            setValue(val);
            currentValueRef.current = val;
            commitValue(val);
            setIsEditing(false);
          }}
        >
          <SelectTrigger className="h-10 w-full border-0 px-2 py-2 ring-0 focus:ring-0 rounded-none box-border">
            <SelectValue placeholder="Select..." />
          </SelectTrigger>
          <SelectContent>
            {enumValues.map((v) => (
              <SelectItem key={v} value={v}>
                {v}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      );
    }

    return (
      <Input
        ref={inputRef}
        value={value === null || value === undefined ? '' : value}
        onChange={(e) => {
          const newValue = e.target.value;
          setValue(newValue);
          currentValueRef.current = newValue;
          commitValue(newValue);
        }}
        onMouseDown={() => {
          onFocus?.();
        }}
        onFocus={() => {
          onFocus?.();
        }}
        onBlur={onBlur}
        onKeyDown={handleKeyDown}
        className="h-10 w-full rounded-none border-0 px-2 py-2 focus-visible:ring-0 bg-blue-50 dark:bg-blue-950/30 box-border ring-2 ring-inset ring-blue-500 dark:ring-blue-600"
        type={
          dataType === 'integer' || dataType === 'decimal' ? 'number' : 'text'
        }
        step={dataType === 'decimal' ? '0.01' : undefined}
      />
    );
  }

  return (
    <div
      className="h-10 w-full cursor-text px-2 py-2 hover:bg-muted/50 flex items-center box-border"
      onMouseDown={(e) => {
        e.preventDefault(); // Prevent default to avoid issues
        onFocus?.();
        setIsEditing(true);
      }}
    >
      {renderValue(value, dataType)}
    </div>
  );
});

/** Compact JSON preview for the table cell */
function getJsonPreview(value: any): string {
  if (value === null || value === undefined) return 'null';
  if (Array.isArray(value)) {
    return `[${value.length} item${value.length !== 1 ? 's' : ''}]`;
  }
  if (typeof value === 'object') {
    const keys = Object.keys(value);
    if (keys.length <= 2) return keys.join(', ');
    return `{${keys.length} fields}`;
  }
  return JSON.stringify(value);
}

/** JSON cell with popover-based tree editor */
function JsonCellEditor({
  value,
  onUpdate,
  onFocus,
}: {
  value: any;
  onUpdate: (value: any) => void;
  onFocus?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const jsonValue = value ?? {};

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <div
          className="h-10 w-full cursor-pointer px-2 py-2 hover:bg-muted/50 flex items-center gap-1.5 box-border"
          onMouseDown={() => onFocus?.()}
        >
          <Braces className="h-3.5 w-3.5 text-muted-foreground/60 shrink-0" />
          <span className="font-mono text-xs text-muted-foreground truncate">
            {getJsonPreview(jsonValue)}
          </span>
        </div>
      </PopoverTrigger>
      <PopoverContent
        side="bottom"
        align="start"
        className="w-[400px] max-h-[400px] overflow-auto p-0"
        onOpenAutoFocus={(e) => e.preventDefault()}
      >
        <JsonEditor
          data={jsonValue}
          setData={(data: any) => {
            onUpdate(data);
          }}
          rootName=""
          collapse={2}
          theme={
            document.documentElement.classList.contains('dark')
              ? githubDarkTheme
              : githubLightTheme
          }
          minWidth="100%"
          rootFontSize="0.8rem"
        />
      </PopoverContent>
    </Popover>
  );
}

function renderValue(value: any, dataType: string) {
  if (value === undefined || value === null) {
    return <span className="text-muted-foreground/50 italic">Empty</span>;
  }

  // Handle transition period: extract value from object if it has a nested "value" property
  if (typeof value === 'object' && 'value' in value) {
    value = (value as { value: any }).value;
  }

  switch (dataType) {
    case 'boolean':
      return String(value).toLowerCase() === 'true' ||
        (typeof value === 'boolean' && value === true) ? (
        <span className="text-green-600 dark:text-green-400">True</span>
      ) : (
        <span className="text-muted-foreground">False</span>
      );
    case 'timestamp':
      return value ? formatDate(String(value)) : '—';
    case 'integer':
      return typeof value === 'number' ? value.toLocaleString() : String(value);
    case 'decimal':
      return typeof value === 'number' ? value.toFixed(2) : String(value);
    case 'json':
      return (
        <span className="font-mono text-xs text-muted-foreground truncate max-w-[200px] block">
          {JSON.stringify(value)}
        </span>
      );
    case 'enum':
      return (
        <span className="inline-flex rounded-lg bg-muted px-2 py-0.5 text-xs font-medium">
          {String(value)}
        </span>
      );
    case 'string':
    default:
      // Safety: if the value is an object/array that slipped through, stringify it
      if (typeof value === 'object') {
        return (
          <span className="font-mono text-xs text-muted-foreground truncate max-w-[200px] block">
            {JSON.stringify(value)}
          </span>
        );
      }
      return (
        <span className="truncate block max-w-[300px]">{String(value)}</span>
      );
  }
}
