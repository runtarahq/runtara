import { useCallback, useState, KeyboardEvent } from 'react';
import { Plus, X } from 'lucide-react';
import { cn } from '@/lib/utils';

interface KeyValueInputProps {
  /** Current value as a string→string map. */
  value: Record<string, string>;
  onChange: (value: Record<string, string>) => void;
  /** Placeholder for the key column. Default: "Key". */
  keyPlaceholder?: string;
  /** Placeholder for the value column. Default: "Value". */
  valuePlaceholder?: string;
  className?: string;
  id?: string;
}

type Row = { key: string; value: string };

/**
 * A key/value editor that renders an ordered list of (key, value) rows with
 * remove/add controls. Designed for connection params whose backend type is
 * `HashMap<String, String>` — e.g. extra HTTP headers, per-tool hint maps.
 *
 * Renders one row per existing entry plus an always-visible "Add" row. Empty
 * keys are filtered out of the emitted object so callers never see
 * `{"": ""}`. Duplicate keys overwrite — last write wins, matching how the
 * backend's `HashMap` would deserialize.
 */
export function KeyValueInput({
  value = {},
  onChange,
  keyPlaceholder = 'Key',
  valuePlaceholder = 'Value',
  className,
  id,
}: KeyValueInputProps) {
  // Preserve key order across renders by deriving rows from the object's
  // own iteration order. JS objects preserve insertion order for string
  // keys, which is what we want here.
  const rows: Row[] = Object.entries(value).map(([key, val]) => ({
    key,
    value: val ?? '',
  }));

  const emit = useCallback(
    (next: Row[]) => {
      const obj: Record<string, string> = {};
      for (const row of next) {
        const k = row.key.trim();
        if (!k) continue;
        obj[k] = row.value;
      }
      onChange(obj);
    },
    [onChange]
  );

  const updateRow = (index: number, patch: Partial<Row>) => {
    const next = [...rows];
    next[index] = { ...next[index], ...patch };
    emit(next);
  };

  const removeRow = (index: number) => {
    emit(rows.filter((_, i) => i !== index));
  };

  const addRow = (key: string, val: string) => {
    const trimmed = key.trim();
    if (!trimmed) return;
    emit([...rows, { key: trimmed, value: val }]);
  };

  return (
    <div
      id={id}
      className={cn(
        'flex w-full flex-col gap-2 rounded-2xl border border-border/50 bg-background p-3',
        className
      )}
    >
      {rows.map((row, index) => (
        <KeyValueRow
          key={`row-${index}-${row.key}`}
          row={row}
          keyPlaceholder={keyPlaceholder}
          valuePlaceholder={valuePlaceholder}
          onKeyChange={(k) => updateRow(index, { key: k })}
          onValueChange={(v) => updateRow(index, { value: v })}
          onRemove={() => removeRow(index)}
        />
      ))}
      <AddRow
        keyPlaceholder={keyPlaceholder}
        valuePlaceholder={valuePlaceholder}
        onAdd={addRow}
      />
    </div>
  );
}

interface KeyValueRowProps {
  row: Row;
  keyPlaceholder: string;
  valuePlaceholder: string;
  onKeyChange: (key: string) => void;
  onValueChange: (value: string) => void;
  onRemove: () => void;
}

function KeyValueRow({
  row,
  keyPlaceholder,
  valuePlaceholder,
  onKeyChange,
  onValueChange,
  onRemove,
}: KeyValueRowProps) {
  return (
    <div className="flex items-center gap-2">
      <input
        type="text"
        value={row.key}
        onChange={(e) => onKeyChange(e.target.value)}
        placeholder={keyPlaceholder}
        className="flex-1 rounded-lg border border-border/50 bg-background px-2 py-1 text-sm outline-none focus:ring-2 focus:ring-ring focus:ring-offset-0"
      />
      <input
        type="text"
        value={row.value}
        onChange={(e) => onValueChange(e.target.value)}
        placeholder={valuePlaceholder}
        className="flex-[2] rounded-lg border border-border/50 bg-background px-2 py-1 text-sm outline-none focus:ring-2 focus:ring-ring focus:ring-offset-0"
      />
      <button
        type="button"
        onClick={onRemove}
        aria-label="Remove entry"
        className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md hover:bg-muted-foreground/10"
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}

interface AddRowProps {
  keyPlaceholder: string;
  valuePlaceholder: string;
  onAdd: (key: string, value: string) => void;
}

function AddRow({ keyPlaceholder, valuePlaceholder, onAdd }: AddRowProps) {
  // Local draft state so half-typed entries don't pollute parent state.
  // Committed on Enter or "Add" click.
  const [draftKey, setDraftKey] = useState('');
  const [draftValue, setDraftValue] = useState('');

  const commit = () => {
    if (!draftKey.trim()) return;
    onAdd(draftKey, draftValue);
    setDraftKey('');
    setDraftValue('');
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      commit();
    }
  };

  return (
    <div className="flex items-center gap-2 border-t border-border/30 pt-2">
      <input
        type="text"
        value={draftKey}
        onChange={(e) => setDraftKey(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={keyPlaceholder}
        className="flex-1 rounded-lg border border-border/50 bg-background px-2 py-1 text-sm outline-none focus:ring-2 focus:ring-ring focus:ring-offset-0"
      />
      <input
        type="text"
        value={draftValue}
        onChange={(e) => setDraftValue(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={valuePlaceholder}
        className="flex-[2] rounded-lg border border-border/50 bg-background px-2 py-1 text-sm outline-none focus:ring-2 focus:ring-ring focus:ring-offset-0"
      />
      <button
        type="button"
        onClick={commit}
        aria-label="Add entry"
        disabled={!draftKey.trim()}
        className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md hover:bg-muted-foreground/10 disabled:cursor-not-allowed disabled:opacity-40"
      >
        <Plus className="h-4 w-4" />
      </button>
    </div>
  );
}
