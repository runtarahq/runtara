import { Schema } from '@/generated/RuntaraRuntimeApi';
import { BarChart3, LineChart, Table2, Type } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Badge } from '@/shared/components/ui/badge';

interface StartStepProps {
  schemas: Schema[];
  selectedSchema: string;
  onSelect: (schemaName: string) => void;
}

const ICONS = [BarChart3, LineChart, Table2, Type];
const TONES = [
  'bg-blue-50 text-blue-600 dark:bg-blue-950 dark:text-blue-300',
  'bg-emerald-50 text-emerald-600 dark:bg-emerald-950 dark:text-emerald-300',
  'bg-amber-50 text-amber-600 dark:bg-amber-950 dark:text-amber-300',
  'bg-purple-50 text-purple-600 dark:bg-purple-950 dark:text-purple-300',
];

export function StartStep({
  schemas,
  selectedSchema,
  onSelect,
}: StartStepProps) {
  if (schemas.length === 0) {
    return (
      <div className="rounded-lg border border-dashed bg-muted/20 p-8 text-center text-sm text-muted-foreground">
        No object schemas yet. Create one in the Database section before
        building a report.
      </div>
    );
  }

  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
      {schemas.map((schema, index) => {
        const isSelected = schema.name === selectedSchema;
        const Icon = ICONS[index % ICONS.length];
        const tone = TONES[index % TONES.length];
        const fieldCount = schema.columns.length;
        const previewFields = schema.columns.slice(0, 3).map((column) => column.name);

        return (
          <button
            key={schema.id}
            type="button"
            onClick={() => onSelect(schema.name)}
            className={cn(
              'group grid min-h-[160px] content-start gap-3 rounded-lg border bg-background p-4 text-left transition-all',
              'hover:-translate-y-px hover:border-primary/40 hover:shadow-md',
              isSelected &&
                'border-primary bg-primary/5 shadow-md ring-1 ring-primary/30'
            )}
          >
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <div className="truncate text-base font-semibold text-foreground">
                  {schema.name}
                </div>
                <div className="mt-0.5 text-xs text-muted-foreground">
                  {fieldCount} {fieldCount === 1 ? 'field' : 'fields'}
                </div>
              </div>
              <span
                className={cn(
                  'grid h-9 w-9 place-items-center rounded-lg',
                  tone
                )}
              >
                <Icon className="h-4 w-4" />
              </span>
            </div>
            {schema.description ? (
              <p className="line-clamp-2 text-xs text-muted-foreground">
                {schema.description}
              </p>
            ) : null}
            <div className="flex flex-wrap gap-1.5">
              {previewFields.map((field) => (
                <Badge key={field} variant="secondary" className="font-medium">
                  {field}
                </Badge>
              ))}
              {fieldCount > previewFields.length ? (
                <Badge variant="outline" className="font-medium">
                  +{fieldCount - previewFields.length}
                </Badge>
              ) : null}
            </div>
          </button>
        );
      })}
    </div>
  );
}
