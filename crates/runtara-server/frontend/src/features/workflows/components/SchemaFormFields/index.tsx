import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Switch } from '@/shared/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Textarea } from '@/shared/components/ui/textarea';
import { type SchemaField } from '@/features/workflows/utils/schema';
import { humanizeKey, isFieldVisible } from './utils';

// ─── Individual field renderer ───────────────────────────────────────────────

interface FieldRendererProps {
  field: SchemaField;
  rawSchema?: Record<string, any>;
  value: any;
  formValues: Record<string, any>;
  onChange: (value: any) => void;
  disabled: boolean;
}

function FieldRenderer({
  field,
  rawSchema,
  value,
  formValues,
  onChange,
  disabled,
}: FieldRendererProps) {
  if (!isFieldVisible(field, formValues)) return null;

  const displayLabel = field.label || humanizeKey(field.name);
  const placeholderText =
    field.placeholder || field.description || `Enter ${field.name}...`;
  const enumValues: string[] | undefined = field.enum ?? rawSchema?.enum;
  const format = field.format;

  return (
    <div className="space-y-1.5">
      <Label className="text-sm">
        {displayLabel}
        {field.required !== false && (
          <span className="text-destructive ml-0.5">*</span>
        )}
      </Label>
      {field.description && (
        <p className="text-xs text-muted-foreground">{field.description}</p>
      )}

      {/* Boolean → Switch */}
      {field.type === 'boolean' && (
        <div className="flex items-center gap-2">
          <Switch
            checked={!!value}
            onCheckedChange={(checked) => onChange(!!checked)}
            disabled={disabled}
          />
        </div>
      )}

      {/* String with enum → Select */}
      {field.type === 'string' && enumValues && enumValues.length > 0 && (
        <Select
          value={value || ''}
          onValueChange={(val) => onChange(val)}
          disabled={disabled}
        >
          <SelectTrigger className="h-8 text-sm">
            <SelectValue placeholder={placeholderText} />
          </SelectTrigger>
          <SelectContent>
            {enumValues.map((opt: string) => (
              <SelectItem key={opt} value={opt}>
                {opt}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}

      {/* String with format=textarea or format=markdown → Textarea */}
      {field.type === 'string' &&
        !enumValues &&
        (format === 'textarea' || format === 'markdown') && (
          <Textarea
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholderText}
            disabled={disabled}
            className="text-sm min-h-[80px]"
            maxLength={field.max ? field.max : undefined}
          />
        )}

      {/* String with date/datetime format → date input */}
      {field.type === 'string' &&
        !enumValues &&
        (format === 'date' ||
          format === 'datetime' ||
          format === 'date-time') && (
          <Input
            type={format === 'date' ? 'date' : 'datetime-local'}
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            disabled={disabled}
            className="h-8 text-sm"
          />
        )}

      {/* String with specialized format (email, url, tel, color) */}
      {field.type === 'string' &&
        !enumValues &&
        format &&
        ['email', 'url', 'tel', 'color', 'password'].includes(format) && (
          <Input
            type={format}
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholderText}
            disabled={disabled}
            className="h-8 text-sm"
            pattern={field.pattern}
            minLength={field.min ? field.min : undefined}
            maxLength={field.max ? field.max : undefined}
          />
        )}

      {/* String without enum, no special format → Text input */}
      {field.type === 'string' && !enumValues && !format && (
        <Input
          value={value || ''}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholderText}
          disabled={disabled}
          className="h-8 text-sm"
          pattern={field.pattern}
          minLength={field.min ? field.min : undefined}
          maxLength={field.max ? field.max : undefined}
        />
      )}

      {/* String with unknown format → fallback to text input */}
      {field.type === 'string' &&
        !enumValues &&
        format &&
        ![
          'textarea',
          'markdown',
          'date',
          'datetime',
          'date-time',
          'email',
          'url',
          'tel',
          'color',
          'password',
        ].includes(format) && (
          <Input
            value={value || ''}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholderText}
            disabled={disabled}
            className="h-8 text-sm"
          />
        )}

      {/* Number/Integer → Number input */}
      {(field.type === 'number' || field.type === 'integer') && (
        <Input
          type="number"
          value={value ?? ''}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholderText}
          disabled={disabled}
          className="h-8 text-sm"
          min={field.min}
          max={field.max}
          step={field.type === 'integer' ? 1 : undefined}
        />
      )}
    </div>
  );
}

// ─── Schema form fields container ────────────────────────────────────────────

interface SchemaFormFieldsProps {
  fields: SchemaField[];
  rawSchema?: Record<string, any>;
  formValues: Record<string, any>;
  onChange: (name: string, value: any) => void;
  disabled: boolean;
}

export function SchemaFormFields({
  fields,
  rawSchema,
  formValues,
  onChange,
  disabled,
}: SchemaFormFieldsProps) {
  return (
    <div className="space-y-3">
      {fields.map((field) => (
        <FieldRenderer
          key={field.name}
          field={field}
          rawSchema={rawSchema?.[field.name]}
          value={formValues[field.name]}
          formValues={formValues}
          onChange={(value) => onChange(field.name, value)}
          disabled={disabled}
        />
      ))}
    </div>
  );
}
