import { useEffect, useRef, useState } from 'react';
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
import { ReportEditorConfig, ReportEditorKind } from '../../../types';

type FieldEditorProps = {
  value: unknown;
  format?: string | null;
  pillVariants?: Record<string, string> | null;
  editor?: ReportEditorConfig;
  busy?: boolean;
  onCommit: (next: unknown) => void;
  onCancel: () => void;
};

export function FieldEditor({
  value,
  format,
  pillVariants,
  editor,
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

type InferredEditor = {
  kind: ReportEditorKind;
  options?: Array<{ label: string; value: unknown }>;
};

export function inferEditorKind(
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
