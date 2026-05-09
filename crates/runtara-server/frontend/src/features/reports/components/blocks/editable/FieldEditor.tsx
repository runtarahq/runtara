import { useEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Switch } from '@/shared/components/ui/switch';
import { Input } from '@/shared/components/ui/input';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/shared/components/ui/command';
import { ReportEditorConfig, ReportEditorKind } from '../../../types';
import { useReportLookupOptions } from '../../../hooks/useReports';

type FieldEditorProps = {
  value: unknown;
  displayValue?: unknown;
  format?: string | null;
  pillVariants?: Record<string, string> | null;
  editor?: ReportEditorConfig;
  lookupContext?: LookupContext;
  busy?: boolean;
  onCommit: (next: unknown) => void;
  onCancel: () => void;
};

type LookupContext = {
  reportId: string;
  blockId: string;
  field: string;
  filters: Record<string, unknown>;
  blockFilters?: Record<string, unknown>;
};

export function FieldEditor({
  value,
  displayValue,
  format,
  pillVariants,
  editor,
  lookupContext,
  busy,
  onCommit,
  onCancel,
}: FieldEditorProps) {
  const inferred = inferEditorKind(value, format, pillVariants, editor);
  const initial = stringifyForInput(value, inferred.kind);
  const [draft, setDraft] = useState<string>(initial);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    if (inputRef.current && 'select' in inputRef.current) {
      try {
        (inputRef.current as HTMLInputElement).select();
      } catch {
        // noop
      }
    }
  }, []);

  const commit = () => {
    const parsed = parseFromInput(draft, inferred.kind);
    if (parsed.error) {
      onCancel();
      return;
    }
    if (valuesEqual(parsed.value, value)) {
      onCancel();
      return;
    }
    onCommit(parsed.value);
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key === 'Enter' && inferred.kind !== 'textarea') {
      event.preventDefault();
      commit();
    }
  };

  if (inferred.kind === 'toggle') {
    return (
      <Switch
        checked={draft === 'true'}
        disabled={busy}
        onCheckedChange={(checked) => {
          setDraft(checked ? 'true' : 'false');
          onCommit(checked);
        }}
      />
    );
  }

  if (inferred.kind === 'select') {
    const options = inferred.options ?? [];
    return (
      <Select
        value={draft}
        disabled={busy}
        onValueChange={(next) => {
          setDraft(next);
          const opt = options.find((o) => stringifyForInput(o.value, 'select') === next);
          onCommit(opt ? opt.value : next);
        }}
      >
        <SelectTrigger className="h-8 w-full text-sm">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map((opt) => {
            const optValue = stringifyForInput(opt.value, 'select');
            return (
              <SelectItem key={optValue} value={optValue}>
                {opt.label}
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>
    );
  }

  if (inferred.kind === 'lookup') {
    return (
      <LookupEditor
        value={value}
        displayValue={displayValue}
        editor={editor}
        lookupContext={lookupContext}
        busy={busy}
        onCommit={onCommit}
        onCancel={onCancel}
      />
    );
  }

  if (inferred.kind === 'textarea') {
    return (
      <Textarea
        ref={inputRef as React.RefObject<HTMLTextAreaElement>}
        value={draft}
        disabled={busy}
        placeholder={editor?.placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            onCancel();
            return;
          }
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
        className="min-h-16 text-sm"
      />
    );
  }

  const inputType =
    inferred.kind === 'number'
      ? 'number'
      : inferred.kind === 'date'
        ? 'date'
        : inferred.kind === 'datetime'
          ? 'datetime-local'
          : 'text';

  return (
    <Input
      ref={inputRef as React.RefObject<HTMLInputElement>}
      type={inputType}
      value={draft}
      disabled={busy}
      placeholder={editor?.placeholder}
      min={editor?.min}
      max={editor?.max}
      step={editor?.step}
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={handleKeyDown}
      onBlur={commit}
      className="h-8 text-sm"
    />
  );
}

function LookupEditor({
  value,
  displayValue,
  editor,
  lookupContext,
  busy,
  onCommit,
  onCancel,
}: {
  value: unknown;
  displayValue?: unknown;
  editor?: ReportEditorConfig;
  lookupContext?: LookupContext;
  busy?: boolean;
  onCommit: (next: unknown) => void;
  onCancel: () => void;
}) {
  const [open, setOpen] = useState(true);
  const [query, setQuery] = useState('');
  const [debouncedQuery, setDebouncedQuery] = useState('');
  const committedRef = useRef(false);

  useEffect(() => {
    const timeout = window.setTimeout(() => setDebouncedQuery(query), 200);
    return () => window.clearTimeout(timeout);
  }, [query]);

  const request = useMemo(
    () => ({
      filters: lookupContext?.filters ?? {},
      blockFilters: lookupContext?.blockFilters ?? {},
      query: debouncedQuery,
      limit: 50,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }),
    [debouncedQuery, lookupContext?.blockFilters, lookupContext?.filters]
  );
  const canLoad = Boolean(lookupContext && editor?.lookup);
  const { data, isFetching } = useReportLookupOptions(
    lookupContext?.reportId,
    lookupContext?.blockId,
    lookupContext?.field,
    request,
    open && canLoad
  );
  const options = data?.options ?? [];
  const selectedKey = optionKey(value);
  const label = lookupDisplayLabel(value, displayValue);

  if (!canLoad) {
    return (
      <Input
        value={String(value ?? '')}
        disabled
        className="h-8 text-sm"
        aria-label="Lookup editor unavailable"
      />
    );
  }

  const commit = (next: unknown) => {
    committedRef.current = true;
    onCommit(next);
  };

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next && !committedRef.current) {
          onCancel();
        }
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={busy}
          className="flex h-8 w-full min-w-40 items-center justify-between gap-2 rounded-md border bg-background px-3 text-left text-sm"
        >
          <span className="truncate">{label}</span>
          <ChevronDown className="h-3.5 w-3.5 shrink-0 opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-80 p-0" align="start">
        <Command shouldFilter={false}>
          <CommandInput
            value={query}
            onValueChange={setQuery}
            placeholder={`Search ${editor?.lookup?.schema ?? 'records'}...`}
          />
          <CommandList>
            <CommandEmpty>
              {isFetching ? 'Loading...' : 'No matching records.'}
            </CommandEmpty>
            <CommandGroup>
              {!isEmptyLookupValue(value) && (
                <CommandItem value="__clear__" onSelect={() => commit(null)}>
                  <span className="flex-1 text-muted-foreground">Clear</span>
                </CommandItem>
              )}
              {options.map((option) => {
                const key = optionKey(option.value);
                const selected = key === selectedKey;
                return (
                  <CommandItem
                    key={key}
                    value={`${option.label} ${key}`}
                    onSelect={() => commit(option.value)}
                  >
                    <span className="flex-1 truncate">{option.label}</span>
                    {selected && <Check className="h-4 w-4 opacity-70" />}
                  </CommandItem>
                );
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

type InferredEditor = {
  kind: ReportEditorKind;
  options?: Array<{ label: string; value: unknown }>;
};

function inferEditorKind(
  value: unknown,
  format: string | null | undefined,
  pillVariants: Record<string, string> | null | undefined,
  editor: ReportEditorConfig | undefined
): InferredEditor {
  if (editor?.kind) {
    return {
      kind: editor.kind,
      options: editor.options ?? variantsToOptions(pillVariants),
    };
  }
  if (format === 'pill' && pillVariants && Object.keys(pillVariants).length > 0) {
    return { kind: 'select', options: variantsToOptions(pillVariants) };
  }
  if (
    format === 'currency' ||
    format === 'currency_compact' ||
    format === 'decimal' ||
    format === 'percent' ||
    format === 'number'
  ) {
    return { kind: 'number' };
  }
  if (format === 'datetime') return { kind: 'datetime' };
  if (format === 'date') return { kind: 'date' };
  if (typeof value === 'boolean') return { kind: 'toggle' };
  if (typeof value === 'number') return { kind: 'number' };
  return { kind: 'text' };
}

function variantsToOptions(
  pillVariants: Record<string, string> | null | undefined
): Array<{ label: string; value: unknown }> {
  if (!pillVariants) return [];
  return Object.keys(pillVariants).map((key) => ({
    label: key,
    value: key,
  }));
}

function lookupDisplayLabel(value: unknown, displayValue: unknown): string {
  if (!isEmptyLookupValue(displayValue)) return String(displayValue);
  if (!isEmptyLookupValue(value)) return String(value);
  return 'Select...';
}

function isEmptyLookupValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  return false;
}

function optionKey(value: unknown): string {
  if (value === null || value === undefined) return '__empty__';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

function stringifyForInput(value: unknown, kind: ReportEditorKind): string {
  if (value === null || value === undefined) return '';
  if (kind === 'toggle') return value ? 'true' : 'false';
  if (kind === 'date') {
    const iso = typeof value === 'string' ? value : String(value);
    return iso.slice(0, 10);
  }
  if (kind === 'datetime') {
    const iso = typeof value === 'string' ? value : new Date(String(value)).toISOString();
    return iso.slice(0, 16);
  }
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

function parseFromInput(
  draft: string,
  kind: ReportEditorKind
): { value: unknown; error?: string } {
  const trimmed = draft.trim();
  if (trimmed === '') return { value: null };
  switch (kind) {
    case 'number': {
      const n = Number(trimmed);
      if (!Number.isFinite(n)) return { value: null, error: 'Not a number' };
      return { value: n };
    }
    case 'toggle':
      return { value: trimmed === 'true' };
    case 'date':
      return { value: `${trimmed}T00:00:00Z` };
    case 'datetime': {
      const d = new Date(trimmed);
      if (Number.isNaN(d.getTime())) return { value: null, error: 'Bad date' };
      return { value: d.toISOString() };
    }
    default:
      return { value: trimmed };
  }
}

function valuesEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  if (typeof a !== typeof b) return false;
  if (typeof a === 'object') return JSON.stringify(a) === JSON.stringify(b);
  return false;
}
