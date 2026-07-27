import { useState, useCallback, useRef, useMemo } from 'react';
import { Upload, FileSpreadsheet, ArrowRight } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Label } from '@/shared/components/ui/label';
import { Badge } from '@/shared/components/ui/badge';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { toast } from 'sonner';
import { Spinner } from '@/shared/components/ui/spinner';
import {
  Schema,
  ImportPreviewResponse,
  CsvImportResponse,
  CsvValidationError,
} from '@/generated/RuntaraRuntimeApi';
import {
  useImportCsvPreview,
  useImportCsv,
} from '@/features/objects/hooks/useObjectRecords';

type ImportStep = 'upload' | 'mapping' | 'importing' | 'results';

interface ImportCsvDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  objectSchemaDto: Schema;
  connectionId?: string | null;
}

const SKIP_COLUMN = '__skip__';

export function ImportCsvDialog({
  open,
  onOpenChange,
  objectSchemaDto,
  connectionId,
}: ImportCsvDialogProps) {
  const [step, setStep] = useState<ImportStep>('upload');
  const [csvBase64, setCsvBase64] = useState<string>('');
  const [fileName, setFileName] = useState<string>('');
  const [preview, setPreview] = useState<ImportPreviewResponse | null>(null);
  const [columnMapping, setColumnMapping] = useState<Record<string, string>>(
    {}
  );
  const [importMode, setImportMode] = useState<'create' | 'upsert'>('create');
  const [conflictColumns, setConflictColumns] = useState<string[]>([]);
  const [skipErrors, setSkipErrors] = useState(false);
  const [importResult, setImportResult] = useState<CsvImportResponse | null>(
    null
  );
  const fileInputRef = useRef<HTMLInputElement>(null);

  const previewMutation = useImportCsvPreview(connectionId);
  const importMutation = useImportCsv(connectionId);

  const schemaName = objectSchemaDto.name || '';

  const resetState = useCallback(() => {
    setStep('upload');
    setCsvBase64('');
    setFileName('');
    setPreview(null);
    setColumnMapping({});
    setImportMode('create');
    setConflictColumns([]);
    setSkipErrors(false);
    setImportResult(null);
  }, []);

  const handleOpenChange = useCallback(
    (newOpen: boolean) => {
      if (!newOpen) {
        resetState();
      }
      onOpenChange(newOpen);
    },
    [onOpenChange, resetState]
  );

  const handleFileSelect = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (!file) return;

      if (!file.name.endsWith('.csv')) {
        toast.error('Please select a CSV file');
        return;
      }

      setFileName(file.name);

      const reader = new FileReader();
      reader.onload = async (event) => {
        const base64 = (event.target?.result as string).split(',')[1];
        setCsvBase64(base64);

        try {
          const result = await previewMutation.mutateAsync({
            schemaName,
            data: { data: base64 },
          });

          if (result.success) {
            setPreview(result);
            // Initialize column mapping from suggested mappings
            const initialMapping: Record<string, string> = {};
            for (const [csvHeader, schemaCol] of Object.entries(
              result.suggestedMappings
            )) {
              initialMapping[csvHeader] = schemaCol || SKIP_COLUMN;
            }
            setColumnMapping(initialMapping);
            setConflictColumns(result.uniqueColumns ?? []);
            setStep('mapping');
          } else {
            toast.error('Failed to parse CSV file');
          }
        } catch {
          // Error toast handled by useCustomMutation
        }
      };
      reader.readAsDataURL(file);

      // Reset input so same file can be re-selected
      e.target.value = '';
    },
    [previewMutation, schemaName]
  );

  const handleImport = useCallback(async () => {
    if (!preview) return;

    // Build the mapping excluding skipped columns
    const finalMapping: Record<string, string> = {};
    for (const [csvHeader, schemaCol] of Object.entries(columnMapping)) {
      if (schemaCol && schemaCol !== SKIP_COLUMN) {
        finalMapping[csvHeader] = schemaCol;
      }
    }

    if (Object.keys(finalMapping).length === 0) {
      toast.error('Please map at least one column');
      return;
    }

    setStep('importing');

    try {
      const result = await importMutation.mutateAsync({
        schemaId: objectSchemaDto.id || '',
        schemaName,
        data: {
          data: csvBase64,
          columnMapping: finalMapping,
          mode: importMode,
          onError: skipErrors ? 'skip' : 'abort',
          ...(importMode === 'upsert' && conflictColumns.length > 0
            ? { conflictColumns }
            : {}),
        },
      });

      if (result.success) {
        const hasSkipped = result.skippedRows != null && result.skippedRows > 0;
        if (hasSkipped) {
          setImportResult(result);
          setStep('results');
        } else {
          toast.success(
            `Imported ${result.affectedRows} row${result.affectedRows !== 1 ? 's' : ''} successfully`
          );
          handleOpenChange(false);
        }
      } else {
        toast.error(result.message || 'Import failed');
        setStep('mapping');
      }
    } catch {
      // Error toast handled by useCustomMutation
      setStep('mapping');
    }
  }, [
    preview,
    columnMapping,
    importMode,
    conflictColumns,
    skipErrors,
    csvBase64,
    objectSchemaDto.id,
    schemaName,
    importMutation,
    handleOpenChange,
  ]);

  const handleMappingChange = useCallback(
    (csvHeader: string, schemaCol: string) => {
      setColumnMapping((prev) => ({ ...prev, [csvHeader]: schemaCol }));
    },
    []
  );

  const mappedCount = Object.values(columnMapping).filter(
    (v) => v && v !== SKIP_COLUMN
  ).length;

  const handleConflictColumnToggle = useCallback(
    (col: string, checked: boolean) => {
      setConflictColumns((prev) =>
        checked ? [...prev, col] : prev.filter((c) => c !== col)
      );
    },
    []
  );

  // Track which schema columns are already mapped to prevent duplicates
  const usedSchemaColumns = useMemo(() => {
    const used = new Map<string, string>();
    for (const [csvHeader, schemaCol] of Object.entries(columnMapping)) {
      if (schemaCol && schemaCol !== SKIP_COLUMN) {
        used.set(schemaCol, csvHeader);
      }
    }
    return used;
  }, [columnMapping]);

  // Reverse map: for each CSV header, the schema column it maps to (if any)
  const mappedSchemaColumnForHeader = useCallback(
    (csvHeader: string): string | null => {
      const val = columnMapping[csvHeader];
      return val && val !== SKIP_COLUMN ? val : null;
    },
    [columnMapping]
  );

  const showMatchBy = importMode === 'upsert';

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-h-[85vh] w-[min(56rem,calc(100vw-2rem))] max-w-none overflow-y-auto overflow-x-hidden">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FileSpreadsheet className="size-5" />
            Import CSV
          </DialogTitle>
          <DialogDescription>
            Import data into {objectSchemaDto.name} from a CSV file.
          </DialogDescription>
        </DialogHeader>

        {step === 'upload' && (
          <div className="py-6">
            <input
              ref={fileInputRef}
              type="file"
              accept=".csv"
              onChange={handleFileSelect}
              className="hidden"
            />
            <button
              type="button"
              onClick={() => fileInputRef.current?.click()}
              disabled={previewMutation.isPending}
              className="w-full cursor-pointer rounded-lg border-2 border-dashed border-muted-foreground/25 p-8 text-center transition-colors hover:border-muted-foreground/50 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {previewMutation.isPending ? (
                <>
                  <Spinner className="mx-auto mb-3 size-10 text-muted-foreground" />
                  <p className="text-sm text-muted-foreground">
                    Parsing {fileName}...
                  </p>
                </>
              ) : (
                <>
                  <Upload className="mx-auto mb-3 size-10 text-muted-foreground" />
                  <p className="text-sm font-medium">
                    Click to select a CSV file
                  </p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    Supports .csv files
                  </p>
                </>
              )}
            </button>
          </div>
        )}

        {step === 'mapping' && preview && (
          <div className="min-w-0 space-y-4">
            <div className="flex items-center justify-between text-sm text-muted-foreground">
              <span>
                {fileName} &mdash; {preview.totalRows} row
                {preview.totalRows !== 1 ? 's' : ''} detected
              </span>
              <Badge variant="outline">
                {mappedCount} of {preview.csvHeaders.length} columns mapped
              </Badge>
            </div>

            {/* Column Mapping */}
            <div className="space-y-3">
              <Label className="text-sm font-medium">Column Mapping</Label>
              <div className="max-h-[40vh] overflow-y-auto rounded-lg border">
                <div
                  className={`sticky top-0 z-10 grid items-center gap-0 border-b bg-card px-3 py-2 ${
                    showMatchBy
                      ? 'grid-cols-[minmax(0,1fr)_32px_minmax(0,1fr)_60px]'
                      : 'grid-cols-[minmax(0,1fr)_32px_minmax(0,1fr)]'
                  }`}
                >
                  <span className="text-xs font-medium text-muted-foreground">
                    CSV Column
                  </span>
                  <span />
                  <span className="text-xs font-medium text-muted-foreground">
                    Schema Column
                  </span>
                  {showMatchBy && (
                    <span className="text-center text-xs font-medium text-muted-foreground">
                      Match by
                    </span>
                  )}
                </div>
                {preview.csvHeaders.map((header) => {
                  const schemaCol = mappedSchemaColumnForHeader(header);
                  const isMapped = schemaCol !== null;
                  const isConflictCol = schemaCol
                    ? conflictColumns.includes(schemaCol)
                    : false;
                  const isUnique = schemaCol
                    ? preview.uniqueColumns?.includes(schemaCol)
                    : false;

                  return (
                    <div
                      key={header}
                      className={`grid items-center gap-0 border-b px-3 py-2 last:border-b-0 ${
                        showMatchBy
                          ? 'grid-cols-[minmax(0,1fr)_32px_minmax(0,1fr)_60px]'
                          : 'grid-cols-[minmax(0,1fr)_32px_minmax(0,1fr)]'
                      }`}
                    >
                      <span
                        className="min-w-0 truncate font-mono text-sm"
                        title={header}
                      >
                        {header}
                      </span>
                      <ArrowRight className="mx-auto size-4 text-muted-foreground" />
                      <Select
                        value={columnMapping[header] || SKIP_COLUMN}
                        onValueChange={(val) =>
                          handleMappingChange(header, val)
                        }
                      >
                        <SelectTrigger className="h-8 text-xs">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value={SKIP_COLUMN}>
                            <span className="italic text-muted-foreground">
                              Skip
                            </span>
                          </SelectItem>
                          {preview.schemaColumns.map((col) => {
                            const mappedBy = usedSchemaColumns.get(col.name);
                            const isUsedByOther =
                              mappedBy !== undefined && mappedBy !== header;
                            return (
                              <SelectItem
                                key={col.name}
                                value={col.name}
                                disabled={isUsedByOther}
                              >
                                {col.name}{' '}
                                <span className="ml-1 text-muted-foreground">
                                  ({col.type})
                                </span>
                                {isUsedByOther && (
                                  <span className="ml-1 text-muted-foreground">
                                    — mapped
                                  </span>
                                )}
                              </SelectItem>
                            );
                          })}
                        </SelectContent>
                      </Select>
                      {showMatchBy && (
                        <div
                          className="flex items-center justify-center"
                          title={
                            !isMapped
                              ? 'Map this column first'
                              : isUnique
                                ? 'Unique column (auto-selected)'
                                : 'Use this column to match existing rows'
                          }
                        >
                          <Checkbox
                            checked={isConflictCol}
                            disabled={!isMapped}
                            onCheckedChange={(checked) => {
                              if (schemaCol) {
                                handleConflictColumnToggle(
                                  schemaCol,
                                  !!checked
                                );
                              }
                            }}
                          />
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>

            {/* Sample Data Preview */}
            {preview.sampleRows.length > 0 && (
              <div className="space-y-2">
                <Label className="text-sm font-medium">Sample Data</Label>
                <div className="overflow-x-auto rounded-lg border">
                  <table className="w-full text-xs">
                    <thead>
                      <tr className="border-b bg-muted/50">
                        {preview.csvHeaders.map((h) => (
                          <th
                            key={h}
                            className="whitespace-nowrap px-3 py-2 text-left font-medium text-muted-foreground"
                          >
                            {h}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {preview.sampleRows.slice(0, 3).map((row, i) => (
                        <tr key={i} className="border-b last:border-b-0">
                          {row.map((cell, j) => (
                            <td
                              key={j}
                              className="max-w-[200px] truncate whitespace-nowrap px-3 py-1.5"
                              title={cell}
                            >
                              {cell}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            )}

            {/* Import Mode */}
            <div className="space-y-2">
              <Label className="text-sm font-medium">Import Mode</Label>
              <Select
                value={importMode}
                onValueChange={(val) =>
                  setImportMode(val as 'create' | 'upsert')
                }
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="create">
                    Create &mdash; Insert new rows only
                  </SelectItem>
                  <SelectItem value="upsert">
                    Upsert &mdash; Insert or update existing rows
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            {/* Skip errors checkbox */}
            <label className="flex cursor-pointer items-center gap-2">
              <Checkbox
                checked={skipErrors}
                onCheckedChange={(checked) => setSkipErrors(!!checked)}
              />
              <span className="text-sm">Skip invalid rows</span>
              <span className="text-xs text-muted-foreground">
                Import valid rows and report errors instead of aborting
              </span>
            </label>
          </div>
        )}

        {step === 'importing' && (
          <div className="flex flex-col items-center justify-center gap-3 py-10">
            <Spinner className="size-8 text-muted-foreground" />
            <p className="text-sm text-muted-foreground">Importing data...</p>
          </div>
        )}

        {step === 'results' && importResult && (
          <div className="min-w-0 space-y-4">
            <div className="flex items-center gap-3 text-sm">
              <Badge variant="default">
                {importResult.affectedRows} imported
              </Badge>
              <Badge variant="destructive">
                {importResult.skippedRows} skipped
              </Badge>
            </div>
            <p className="text-sm text-muted-foreground">
              {importResult.message}
            </p>

            {(importResult.validationErrors as CsvValidationError[] | null)
              ?.length ? (
              <div className="space-y-2">
                <Label className="text-sm font-medium">Validation Errors</Label>
                <div className="max-h-[40vh] overflow-y-auto rounded-lg border">
                  <table className="w-full text-xs">
                    <thead>
                      <tr className="sticky top-0 border-b bg-muted/50">
                        <th className="w-16 px-3 py-2 text-left font-medium text-muted-foreground">
                          Row
                        </th>
                        <th className="px-3 py-2 text-left font-medium text-muted-foreground">
                          Column
                        </th>
                        <th className="px-3 py-2 text-left font-medium text-muted-foreground">
                          Error
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {(
                        importResult.validationErrors as CsvValidationError[]
                      ).map((err, i) => (
                        <tr key={i} className="border-b last:border-b-0">
                          <td className="px-3 py-1.5 font-mono">{err.row}</td>
                          <td className="px-3 py-1.5 font-mono">
                            {err.column}
                          </td>
                          <td className="px-3 py-1.5">{err.error}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}
          </div>
        )}

        {step === 'results' && (
          <DialogFooter>
            <Button onClick={() => handleOpenChange(false)}>Done</Button>
          </DialogFooter>
        )}

        {step === 'mapping' && (
          <DialogFooter>
            <Button
              variant="secondary"
              bordered
              onClick={() => {
                setStep('upload');
                setPreview(null);
                setCsvBase64('');
                setFileName('');
              }}
            >
              Back
            </Button>
            <Button
              onClick={handleImport}
              disabled={
                mappedCount === 0 ||
                importMutation.isPending ||
                (importMode === 'upsert' && conflictColumns.length === 0)
              }
            >
              Import {preview?.totalRows ?? 0} rows
            </Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}
