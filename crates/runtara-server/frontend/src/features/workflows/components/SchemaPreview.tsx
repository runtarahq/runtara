import { Badge } from '@/shared/components/ui/badge';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { parseSchema, SchemaField } from '@/features/workflows/utils/schema';

type SchemaPreviewProps = {
  title?: string;
  schema?: any;
  fields?: SchemaField[];
  emptyLabel?: string;
  compact?: boolean;
};

function renderField(field: SchemaField) {
  return (
    <div
      key={field.name}
      className="flex flex-col gap-0.5 rounded-md border border-border/50 bg-muted/30 p-3"
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <span className="font-mono text-sm">{field.name}</span>
          {field.required ? (
            <Badge variant="default" className="h-5 rounded">
              required
            </Badge>
          ) : (
            <Badge variant="secondary" className="h-5 rounded">
              optional
            </Badge>
          )}
        </div>
        <Badge variant="outline" className="h-5 rounded px-2 text-xs uppercase">
          {field.type || 'string'}
        </Badge>
      </div>
      {field.description && (
        <p className="text-xs text-muted-foreground">{field.description}</p>
      )}
      {field.defaultValue !== undefined && field.defaultValue !== '' && (
        <p className="text-xs text-muted-foreground">
          Default:{' '}
          <span className="font-mono">{String(field.defaultValue)}</span>
        </p>
      )}
    </div>
  );
}

export function SchemaPreview({
  title = 'Schema',
  schema,
  fields,
  emptyLabel = 'No fields defined',
  compact = false,
}: SchemaPreviewProps) {
  const normalizedFields = fields ?? parseSchema(schema);
  const hasFields = normalizedFields.length > 0;

  if (!hasFields) {
    return (
      <Card className="bg-muted/30">
        <CardHeader className="py-3">
          <CardTitle className="text-sm font-semibold text-muted-foreground">
            {title}
          </CardTitle>
        </CardHeader>
        <CardContent className="py-0 pb-3">
          <p className="text-xs text-muted-foreground">{emptyLabel}</p>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="bg-muted/30">
      <CardHeader className="py-3">
        <CardTitle className="text-sm font-semibold text-muted-foreground">
          {title}
        </CardTitle>
      </CardHeader>
      <CardContent className="pt-0">
        <div
          className={
            compact ? 'max-h-48 space-y-2 overflow-y-auto pr-1' : 'space-y-2'
          }
        >
          {normalizedFields.map((field) => renderField(field))}
        </div>
      </CardContent>
    </Card>
  );
}
