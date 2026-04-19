import { useMemo, useState } from 'react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Label } from '@/shared/components/ui/label';
import { Textarea } from '@/shared/components/ui/textarea';
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
import {
  BulkConflictMode,
  BulkValidationMode,
  BulkCreateResult,
} from '../../queries';

interface BulkInsertDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  schema: Schema;
  onSubmit: (
    instances: unknown[],
    onConflict: BulkConflictMode,
    onError: BulkValidationMode,
    conflictColumns: string[]
  ) => Promise<BulkCreateResult | undefined>;
  isSubmitting?: boolean;
}

/**
 * Paste a JSON array, pick a conflict mode + error mode, submit to
 * POST /instances/{schema_id}/bulk. Shows the per-row error list returned
 * by the server when `onError=skip` tripped.
 */
export function BulkInsertDialog({
  open,
  onOpenChange,
  schema,
  onSubmit,
  isSubmitting,
}: BulkInsertDialogProps) {
  const [rawJson, setRawJson] = useState('');
  const [parseError, setParseError] = useState<string | null>(null);
  const [onConflict, setOnConflict] = useState<BulkConflictMode>('error');
  const [onError, setOnError] = useState<BulkValidationMode>('stop');
  const [conflictCols, setConflictCols] = useState<Set<string>>(new Set());
  const [result, setResult] = useState<BulkCreateResult | null>(null);

  const columnNames = useMemo(
    () => (schema.columns ?? []).map((c) => c.name),
    [schema]
  );

  const needsConflictColumns = onConflict !== 'error';
  const conflictColsSelected = conflictCols.size > 0;

  const toggleCol = (name: string, picked: boolean) => {
    setConflictCols((prev) => {
      const next = new Set(prev);
      if (picked) next.add(name);
      else next.delete(name);
      return next;
    });
  };

  const handleSubmit = async () => {
    setParseError(null);
    setResult(null);
    let parsed: unknown;
    try {
      parsed = JSON.parse(rawJson);
    } catch (e) {
      setParseError((e as Error).message);
      return;
    }
    if (!Array.isArray(parsed)) {
      setParseError('Input must be a JSON array of objects');
      return;
    }
    if (needsConflictColumns && !conflictColsSelected) {
      setParseError(
        `Select at least one conflict column for onConflict=${onConflict}`
      );
      return;
    }

    const out = await onSubmit(
      parsed,
      onConflict,
      onError,
      Array.from(conflictCols)
    );
    if (out) setResult(out);
  };

  const handleOpenChange = (next: boolean) => {
    if (!next) {
      setRawJson('');
      setParseError(null);
      setResult(null);
      setOnConflict('error');
      setOnError('stop');
      setConflictCols(new Set());
    }
    onOpenChange(next);
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Bulk Insert (JSON)</DialogTitle>
          <DialogDescription>
            Paste a JSON array of records. Each element is an object keyed by
            column name.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1">
            <Label htmlFor="bulk-insert-json">Records (JSON array)</Label>
            <Textarea
              id="bulk-insert-json"
              value={rawJson}
              onChange={(e) => setRawJson(e.target.value)}
              placeholder='[{"sku": "A", "quantity": 1}, {"sku": "B", "quantity": 2}]'
              rows={8}
              spellCheck={false}
              className="font-mono text-sm"
            />
          </div>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="onConflict-select">On conflict</Label>
              <Select
                value={onConflict}
                onValueChange={(v) => setOnConflict(v as BulkConflictMode)}
              >
                <SelectTrigger id="onConflict-select">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="error">Error on conflict</SelectItem>
                  <SelectItem value="skip">Skip existing</SelectItem>
                  <SelectItem value="upsert">Upsert</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-2">
              <Label htmlFor="onError-select">On validation error</Label>
              <Select
                value={onError}
                onValueChange={(v) => setOnError(v as BulkValidationMode)}
              >
                <SelectTrigger id="onError-select">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="stop">Stop on first error</SelectItem>
                  <SelectItem value="skip">Skip failed rows</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          {needsConflictColumns && (
            <div className="space-y-2">
              <Label>
                Conflict columns <span className="text-destructive">*</span>
              </Label>
              <div className="grid grid-cols-2 gap-2 max-h-40 overflow-y-auto rounded border p-2">
                {columnNames.map((name) => (
                  <label
                    key={name}
                    className="flex items-center gap-2 text-sm"
                  >
                    <Checkbox
                      checked={conflictCols.has(name)}
                      onCheckedChange={(state) =>
                        toggleCol(name, state === true)
                      }
                    />
                    {name}
                  </label>
                ))}
              </div>
              <p className="text-xs text-muted-foreground">
                Rows are matched on these column values.
              </p>
            </div>
          )}

          {parseError && (
            <p className="text-sm text-destructive">{parseError}</p>
          )}

          {result && (
            <div className="rounded border bg-muted/40 p-3 space-y-1 text-sm">
              <div>
                <strong>{result.createdCount}</strong> created,{' '}
                <strong>{result.skippedCount}</strong> skipped
              </div>
              {result.errors.length > 0 && (
                <details className="mt-1">
                  <summary className="cursor-pointer">
                    {result.errors.length} row error
                    {result.errors.length === 1 ? '' : 's'}
                  </summary>
                  <ul className="mt-1 list-disc pl-5 space-y-0.5 max-h-40 overflow-y-auto">
                    {result.errors.map((e, i) => (
                      <li key={`${e.index}-${i}`}>
                        <span className="font-mono">#{e.index}</span>:{' '}
                        {e.reason}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => handleOpenChange(false)}
            disabled={isSubmitting}
          >
            Close
          </Button>
          <Button onClick={handleSubmit} disabled={isSubmitting || !rawJson}>
            {isSubmitting ? 'Inserting…' : 'Insert'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
