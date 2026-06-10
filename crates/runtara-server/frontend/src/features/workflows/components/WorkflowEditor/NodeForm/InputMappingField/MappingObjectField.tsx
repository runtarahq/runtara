/**
 * MappingObjectField - Standalone structured editor for an InputMapping-shaped
 * object value (name → MappingValue), used by Log.context, Error.context and
 * WaitForSignal.action.correlation/.context.
 *
 * Unlike SimpleInputMappingEditor (which is coupled to the node form's
 * inputMapping field array and the nodeFormStore), this component is a plain
 * controlled value/onChange editor:
 *
 *   - `value` is whatever object the call site holds in form state (UI format,
 *     raw DSL format typed via JSON, or bare literals — see
 *     normalizeMappingObject in mapping-entries.ts);
 *   - `onChange` receives the full updated object in UI format (entries with
 *     typeHint/defaultValue), which the existing serializers
 *     (serializeMappingObject → processCompositeValue in CustomNodes/utils.tsx)
 *     already convert losslessly to the DSL shape;
 *   - an "Edit as JSON" toggle reveals the legacy textarea bound to the same
 *     value, so exotic shapes remain reachable and JSON/structured edits
 *     round-trip through the same form state.
 *
 * Removing every row emits {} — the same value the empty legacy textarea
 * produced — so the serializers' clear-when-empty (delete key) behavior is
 * preserved.
 */

import { useMemo, useRef, useState } from 'react';
import { Plus, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { cn } from '@/lib/utils';
import type {
  CompositeObjectValue,
  CompositeArrayValue,
} from '@/features/workflows/stores/nodeFormStore';
import { MappingValueInput, ValueMode } from './MappingValueInput';
import { CompositeValueEditor } from './CompositeValueEditor';
import {
  formatMappingObjectJson,
  normalizeMappingObject,
  parseMappingObjectJson,
  type MappingObjectEntry,
} from './mapping-entries';

/**
 * Type-hint options per row. 'auto' clears the hint. For reference entries
 * the hint serializes as the backend `type`; for immediate entries it drives
 * value coercion on save (e.g. "5" + integer → 5).
 */
const TYPE_HINT_OPTIONS: Array<{ value: string; label: string }> = [
  { value: 'auto', label: 'Auto' },
  { value: 'string', label: 'String' },
  { value: 'integer', label: 'Integer' },
  { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'json', label: 'JSON' },
];

interface MappingObjectFieldProps {
  /** The mapping object as held in form state (see normalizeMappingObject). */
  value: unknown;
  /**
   * Receives the updated value: a UI-format mapping object from structured
   * edits, or whatever the JSON textarea parses to (object, or the raw string
   * while the JSON is invalid — identical to the legacy textarea behavior).
   */
  onChange: (value: unknown) => void;
  /** Placeholder shown in the JSON textarea. */
  jsonPlaceholder?: string;
  disabled?: boolean;
}

export function MappingObjectField({
  value,
  onChange,
  jsonPlaceholder,
  disabled = false,
}: MappingObjectFieldProps) {
  const [showJson, setShowJson] = useState(false);
  const [isAddingField, setIsAddingField] = useState(false);
  const [newFieldName, setNewFieldName] = useState('');
  const [addError, setAddError] = useState<string | null>(null);

  const normalized = useMemo(() => normalizeMappingObject(value), [value]);
  const structuredEditable = normalized !== null;

  // Latest object for composing sequential callbacks. MappingValueInput's
  // mode toggle fires onValueTypeChange then onChange('') synchronously;
  // composing both against this ref (instead of render-bound props) keeps the
  // second call from clobbering the first.
  const objRef = useRef<Record<string, MappingObjectEntry>>({});
  objRef.current = normalized ?? objRef.current;

  const emit = (next: Record<string, MappingObjectEntry>) => {
    objRef.current = next;
    onChange(next);
  };

  const patchEntry = (
    key: string,
    updater: (current: MappingObjectEntry) => MappingObjectEntry
  ) => {
    const current = objRef.current[key];
    if (!current) return;
    emit({ ...objRef.current, [key]: updater(current) });
  };

  const removeEntry = (key: string) => {
    const { [key]: _removed, ...rest } = objRef.current;
    void _removed;
    emit(rest);
  };

  const renameEntry = (oldKey: string, newKey: string) => {
    // Rebuild preserving row order.
    const next: Record<string, MappingObjectEntry> = {};
    for (const [key, entry] of Object.entries(objRef.current)) {
      next[key === oldKey ? newKey : key] = entry;
    }
    emit(next);
  };

  const handleAddField = () => {
    const trimmed = newFieldName.trim();
    if (!trimmed) {
      setAddError('Key is required');
      return;
    }
    if (objRef.current[trimmed] !== undefined) {
      setAddError('Duplicate key');
      return;
    }
    emit({
      ...objRef.current,
      [trimmed]: { valueType: 'immediate', value: '' },
    });
    setNewFieldName('');
    setAddError(null);
    setIsAddingField(false);
  };

  const keys = normalized ? Object.keys(normalized) : [];
  const jsonVisible = showJson || !structuredEditable;

  return (
    <div className="space-y-2">
      {!structuredEditable && (
        <p className="text-xs text-muted-foreground">
          The current value is not a simple mapping object — edit it as JSON
          below.
        </p>
      )}

      {structuredEditable && !showJson && (
        <div className="space-y-2">
          {keys.length === 0 && !isAddingField && (
            <p className="text-xs text-muted-foreground italic">
              No fields defined.
            </p>
          )}

          {keys.map((key) => (
            <MappingObjectRow
              key={key}
              entryKey={key}
              entry={normalized![key]}
              existingKeys={keys}
              disabled={disabled}
              onPatch={(updater) => patchEntry(key, updater)}
              onRename={(newKey) => renameEntry(key, newKey)}
              onRemove={() => removeEntry(key)}
            />
          ))}

          {isAddingField && (
            <div className="rounded-md border border-dashed bg-muted/30 p-2">
              <div className="flex items-center gap-2">
                <Input
                  type="text"
                  value={newFieldName}
                  onChange={(e) => {
                    setNewFieldName(e.target.value);
                    setAddError(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      handleAddField();
                    }
                    if (e.key === 'Escape') {
                      setIsAddingField(false);
                      setNewFieldName('');
                      setAddError(null);
                    }
                  }}
                  placeholder="Field name..."
                  className="flex-1 h-8 text-sm"
                  autoFocus
                />
                <Button
                  type="button"
                  size="sm"
                  onClick={handleAddField}
                  disabled={!newFieldName.trim()}
                >
                  Add
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setIsAddingField(false);
                    setNewFieldName('');
                    setAddError(null);
                  }}
                >
                  Cancel
                </Button>
              </div>
              {addError && (
                <p className="text-xs text-destructive mt-1">{addError}</p>
              )}
            </div>
          )}

          {!disabled && !isAddingField && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-full border-dashed"
              onClick={() => setIsAddingField(true)}
            >
              <Plus className="h-3.5 w-3.5 mr-1.5" />
              Add Field
            </Button>
          )}
        </div>
      )}

      <div className="flex justify-end">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-6 px-1 text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setShowJson(!showJson)}
          disabled={!structuredEditable}
        >
          {jsonVisible && structuredEditable ? 'Hide JSON' : 'Edit as JSON'}
        </Button>
      </div>

      {jsonVisible && (
        <Textarea
          value={formatMappingObjectJson(value)}
          onChange={(event) => onChange(parseMappingObjectJson(event.target.value))}
          placeholder={jsonPlaceholder}
          disabled={disabled}
          className="min-h-24 font-mono text-sm"
        />
      )}
    </div>
  );
}

interface MappingObjectRowProps {
  entryKey: string;
  entry: MappingObjectEntry;
  existingKeys: string[];
  disabled: boolean;
  onPatch: (updater: (current: MappingObjectEntry) => MappingObjectEntry) => void;
  onRename: (newKey: string) => void;
  onRemove: () => void;
}

function MappingObjectRow({
  entryKey,
  entry,
  existingKeys,
  disabled,
  onPatch,
  onRename,
  onRemove,
}: MappingObjectRowProps) {
  const [keyDraft, setKeyDraft] = useState(entryKey);
  const [keyError, setKeyError] = useState<string | null>(null);

  // Reset the draft when the committed key changes (e.g. rename round-trip).
  const lastCommittedKey = useRef(entryKey);
  if (lastCommittedKey.current !== entryKey) {
    lastCommittedKey.current = entryKey;
    setKeyDraft(entryKey);
    setKeyError(null);
  }

  const commitKey = () => {
    const trimmed = keyDraft.trim();
    if (trimmed === entryKey) {
      setKeyDraft(entryKey);
      setKeyError(null);
      return;
    }
    if (!trimmed) {
      setKeyError('Key is required');
      return;
    }
    if (existingKeys.includes(trimmed)) {
      setKeyError('Duplicate key');
      return;
    }
    setKeyError(null);
    onRename(trimmed);
  };

  const isComposite = entry.valueType === 'composite';
  const showTypeHint =
    entry.valueType === 'immediate' || entry.valueType === 'reference';

  // Derive an effective hint for untyped immediates so editing a loaded
  // boolean/number keeps its DSL type (the serializer coerces by typeHint).
  const effectiveTypeHint =
    entry.typeHint ??
    (entry.valueType === 'immediate' && typeof entry.value === 'boolean'
      ? 'boolean'
      : entry.valueType === 'immediate' && typeof entry.value === 'number'
        ? 'number'
        : undefined);

  const handleValueChange = (newValue: string | null) => {
    onPatch((current) => {
      if (current.valueType === 'composite') {
        // MappingValueInput clears the value right after a mode switch;
        // composite payloads live in the editor below, so keep/seed the
        // object instead of storing ''.
        const compositeValue =
          current.value && typeof current.value === 'object'
            ? current.value
            : {};
        return { ...current, value: compositeValue };
      }
      const next: MappingObjectEntry = { ...current, value: newValue };
      if (
        current.valueType === 'immediate' &&
        current.typeHint === undefined &&
        effectiveTypeHint !== undefined
      ) {
        next.typeHint = effectiveTypeHint;
      }
      return next;
    });
  };

  const handleValueTypeChange = (newType: ValueMode) => {
    onPatch((current) => ({
      ...current,
      valueType: newType,
      value: newType === 'composite' ? {} : '',
    }));
  };

  const handleTypeHintChange = (newHint: string) => {
    onPatch((current) => {
      const next = { ...current };
      if (newHint === 'auto') {
        delete next.typeHint;
      } else {
        next.typeHint = newHint;
      }
      return next;
    });
  };

  const handleDefaultValueChange = (newDefault: string | undefined) => {
    onPatch((current) => {
      const next = { ...current };
      if (newDefault === undefined) {
        delete next.defaultValue;
      } else {
        next.defaultValue = newDefault;
      }
      return next;
    });
  };

  const handleCompositeChange = (
    compositeValue: CompositeObjectValue | CompositeArrayValue
  ) => {
    onPatch((current) => ({ ...current, value: compositeValue }));
  };

  const compositeEditorValue: CompositeObjectValue | CompositeArrayValue =
    entry.value && typeof entry.value === 'object'
      ? (entry.value as CompositeObjectValue | CompositeArrayValue)
      : {};

  return (
    <div className="rounded-md border bg-background p-2 space-y-1">
      <div className="flex items-start gap-2">
        <div className="w-36 shrink-0">
          <Input
            type="text"
            value={keyDraft}
            onChange={(e) => {
              setKeyDraft(e.target.value);
              setKeyError(null);
            }}
            onBlur={commitKey}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                commitKey();
              }
              if (e.key === 'Escape') {
                setKeyDraft(entryKey);
                setKeyError(null);
              }
            }}
            placeholder="Key"
            disabled={disabled}
            className={cn(
              'h-9 font-mono text-sm',
              keyError && 'border-destructive'
            )}
          />
        </div>

        {showTypeHint && (
          <Select
            value={effectiveTypeHint ?? 'auto'}
            onValueChange={handleTypeHintChange}
            disabled={disabled}
          >
            <SelectTrigger className="h-9 w-[92px] text-xs shrink-0">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {TYPE_HINT_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}

        <div className="flex-1 min-w-0">
          <MappingValueInput
            value={
              isComposite
                ? ''
                : typeof entry.value === 'object' && entry.value !== null
                  ? JSON.stringify(entry.value, null, 2)
                  : (entry.value as string | number | boolean | null | undefined)
            }
            onChange={handleValueChange}
            valueType={entry.valueType}
            onValueTypeChange={handleValueTypeChange}
            fieldType={effectiveTypeHint ?? 'string'}
            allowNull
            placeholder="Enter value..."
            defaultValue={entry.defaultValue}
            onDefaultValueChange={handleDefaultValueChange}
            disabled={disabled}
          />
        </div>

        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-9 w-7 shrink-0 text-muted-foreground hover:text-destructive"
          onClick={onRemove}
          disabled={disabled}
          title="Remove field"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>

      {keyError && <p className="text-xs text-destructive">{keyError}</p>}

      {isComposite && (
        <div className="rounded-md border bg-muted/20">
          <CompositeValueEditor
            value={compositeEditorValue}
            onChange={handleCompositeChange}
            showCloseButton={false}
            disabled={disabled}
          />
        </div>
      )}
    </div>
  );
}
