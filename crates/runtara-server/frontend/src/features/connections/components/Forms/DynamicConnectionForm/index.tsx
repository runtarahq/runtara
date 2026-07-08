import { useMemo } from 'react';
import { z } from 'zod';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { Server, Key, Shield } from 'lucide-react';
import { NextForm } from '@/shared/components/NextForm';
import { FormContent } from '@/shared/components/NextForm/form-content';
import { ConnectionFormLayout } from '../ConnectionFormLayout';
import { FormSection } from '../FormSection';
import { ServiceIcon } from '@/shared/components/service-icon';
import {
  ConnectionTypeDto,
  ConnectionFieldDto,
  RateLimitConfigDto,
  RateLimitStatusDto,
} from '@/generated/RuntaraRuntimeApi';
import { RateLimitSection } from '../RateLimitSection';
import { DefaultFileStorageSection } from '../DefaultFileStorageSection';
import { DefaultForSection } from '../DefaultForSection';

type FieldConfig = {
  type: string;
  label: string;
  name: string;
  initialValue: unknown;
  placeholder?: string;
  description?: string;
  colSpan?: string;
  isSecret?: boolean;
  isOptional?: boolean;
  fieldName?: string; // Original field name for smarter grouping
  /** Populated when the backend declared `enumValues` for this field —
   * triggers select rendering via the FormField switch. */
  options?: Array<{ value: string; label: string }>;
};

/**
 * Derive a human-readable label for an enum option value.
 *
 * `"none"` → `"None"`, `"api_key"` → `"API Key"`, `"snake_case_thing"`
 * → `"Snake Case Thing"`. Known acronyms are upper-cased so the select
 * looks right without per-value backend metadata.
 */
const ENUM_LABEL_ACRONYMS = new Set([
  'api',
  'url',
  'uri',
  'sql',
  'ssh',
  'ssl',
  'tls',
  'json',
  'xml',
  'html',
  'http',
  'https',
  'csv',
  'pdf',
  'aws',
  'gcp',
  'mcp',
  'oauth',
  'oauth2',
  'jwt',
  'id',
]);

function enumValueToLabel(value: string): string {
  return value
    .split(/[_\s-]+/)
    .filter(Boolean)
    .map((token) =>
      ENUM_LABEL_ACRONYMS.has(token.toLowerCase())
        ? token.toUpperCase()
        : token.charAt(0).toUpperCase() + token.slice(1).toLowerCase()
    )
    .join(' ');
}

// Type names that indicate an array of strings
const ARRAY_TYPE_NAMES = ['vec<string>', 'array', 'list', 'string[]', 'tags'];

// Type names that indicate a string→string map (e.g. extra HTTP headers,
// per-tool hint maps on the MCP connection). Rendered as a key/value editor.
const KEYVALUE_TYPE_NAMES = [
  'hashmap<string, string>',
  'hashmap<string,string>',
  'map<string, string>',
  'map<string,string>',
  'record<string, string>',
];

// Field names that hint at key/certificate content (multiline)
const MULTILINE_FIELD_NAMES = [
  'privatekey',
  'private_key',
  'publickey',
  'public_key',
  'certificate',
  'cert',
  'sshkey',
  'ssh_key',
];
const FILE_STORAGE_CATEGORIES = new Set(['file_storage', 'storage']);

/**
 * Maps ConnectionFieldDto.typeName to form field type
 */
function getFieldType(field: ConnectionFieldDto): string {
  const typeName = field.typeName?.toLowerCase() ?? '';
  const fieldNameLower = field.name.toLowerCase().replace(/[_-]/g, '');

  // Bounded-set fields → select. Takes precedence over secret/text fallback
  // so an enum-typed secret (rare) would still render as a select; we don't
  // expect that combination in practice.
  if (field.enumValues && field.enumValues.length > 0) {
    return 'select';
  }

  // Array types → tag input
  if (ARRAY_TYPE_NAMES.includes(typeName)) {
    return 'tags';
  }

  // String→string maps → key/value editor
  if (KEYVALUE_TYPE_NAMES.includes(typeName)) {
    return 'keyvalue';
  }

  // Multiline secret fields (keys, certs) → textarea
  if (
    field.isSecret &&
    MULTILINE_FIELD_NAMES.some((name) =>
      fieldNameLower.includes(name.replace(/[_-]/g, ''))
    )
  ) {
    return 'textarea';
  }

  // Regular secrets → password
  if (field.isSecret) {
    return 'password';
  }

  switch (typeName) {
    case 'u16':
    case 'u32':
    case 'i32':
    case 'number':
      return 'number';
    case 'bool':
      return 'checkbox';
    default:
      return 'text';
  }
}

/**
 * Builds Zod schema from ConnectionFieldDto array
 */
function buildSchema(
  fields: ConnectionFieldDto[]
): z.ZodObject<Record<string, z.ZodTypeAny>> {
  const shape: Record<string, z.ZodTypeAny> = {
    // Title is always required
    title: z.string().min(1, 'Please enter a title'),
  };

  for (const field of fields) {
    const typeName = field.typeName?.toLowerCase() ?? '';
    let fieldSchema: z.ZodTypeAny;

    // Array types
    if (ARRAY_TYPE_NAMES.includes(typeName)) {
      fieldSchema = field.isOptional
        ? z.array(z.string()).optional()
        : z
            .array(z.string())
            .min(
              1,
              `${field.displayName || field.name} requires at least one item`
            );
      shape[field.name] = fieldSchema;
      continue;
    }

    // String→string maps
    if (KEYVALUE_TYPE_NAMES.includes(typeName)) {
      const record = z.record(z.string(), z.string());
      shape[field.name] = field.isOptional ? record.optional() : record;
      continue;
    }

    // Determine base schema type
    switch (typeName) {
      case 'u16':
      case 'u32':
      case 'i32':
      case 'number':
        fieldSchema = z.coerce.number();
        if (field.typeName === 'u16') {
          fieldSchema = (fieldSchema as z.ZodNumber)
            .min(0)
            .max(65535, 'Port must be between 0 and 65535');
        }
        break;
      case 'bool':
        fieldSchema = z.boolean();
        break;
      default:
        fieldSchema = z.string();
    }

    // Boolean fields don't need optional/required handling
    if (typeName === 'bool') {
      if (field.isOptional) {
        fieldSchema = fieldSchema.optional();
      }
    } else if (field.isOptional) {
      // For optional strings, allow empty string
      if (typeName !== 'u16') {
        fieldSchema = fieldSchema.optional().or(z.literal(''));
      } else {
        fieldSchema = fieldSchema.optional();
      }
    } else {
      // Required field
      if (typeof fieldSchema === 'object' && 'min' in fieldSchema) {
        fieldSchema = (fieldSchema as z.ZodString).min(
          1,
          `${field.displayName || field.name} is required`
        );
      }
    }

    shape[field.name] = fieldSchema;
  }

  return z.object(shape);
}

/**
 * Builds form field configs from ConnectionFieldDto array
 */
function buildFieldsConfig(fields: ConnectionFieldDto[]): FieldConfig[] {
  // Title field is always first
  const titleField: FieldConfig = {
    type: 'text',
    label: 'Title',
    name: 'title',
    initialValue: '',
    placeholder: 'Enter a descriptive name for this connection',
    description: 'A unique name to identify this connection',
    colSpan: 'full',
    isSecret: false,
    isOptional: false,
    fieldName: 'title',
  };

  const fieldConfigs: FieldConfig[] = fields.map((field) => {
    const fieldNameLower = field.name.toLowerCase();
    const typeName = field.typeName?.toLowerCase() ?? '';
    const fieldType = getFieldType(field);

    // Port fields should be smaller (1/3 width when paired with host)
    const isPortField = fieldNameLower === 'port';
    // Host fields should be wider (2/3 width when paired with port)
    const isHostField =
      fieldNameLower === 'host' || fieldNameLower === 'hostname';

    // Determine column span for 3-column grid
    let colSpan = 'full'; // default: span all 3 columns
    if (isPortField) {
      colSpan = '1'; // 1/3 width
    } else if (isHostField) {
      colSpan = '2'; // 2/3 width
    }

    // For numeric fields, use placeholder as default value if no defaultValue is set
    const isNumericField = ['u16', 'u32', 'i32', 'number'].includes(typeName);
    const isBoolField = typeName === 'bool';
    const isArrayField = ARRAY_TYPE_NAMES.includes(typeName);
    const isKeyValueField = KEYVALUE_TYPE_NAMES.includes(typeName);
    const hasEnumValues = !!field.enumValues && field.enumValues.length > 0;
    let initialValue: unknown = '';
    if (hasEnumValues) {
      // Default to the declared default if it's one of the allowed values,
      // otherwise fall back to the first enum value so the select isn't blank.
      const defaults = field.enumValues ?? [];
      initialValue =
        field.defaultValue && defaults.includes(field.defaultValue)
          ? field.defaultValue
          : defaults[0] ?? '';
    } else if (isBoolField) {
      initialValue = field.defaultValue === 'true' ? true : false;
    } else if (isArrayField) {
      // Parse default value as comma-separated for arrays
      initialValue = field.defaultValue
        ? field.defaultValue
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
        : [];
    } else if (isKeyValueField) {
      // HashMap defaults to an empty object; defaultValue strings from the
      // backend aren't parsed here — keyvalue defaults are always empty.
      initialValue = {};
    } else if (field.defaultValue) {
      initialValue = field.defaultValue;
    } else if (isNumericField && field.placeholder) {
      initialValue = field.placeholder;
    }

    return {
      type: fieldType,
      label: field.displayName || field.name,
      name: field.name,
      initialValue,
      placeholder: field.placeholder || undefined,
      description: field.description || undefined,
      colSpan,
      isSecret: field.isSecret || false,
      isOptional: field.isOptional || false,
      fieldName: field.name,
      options: hasEnumValues
        ? (field.enumValues ?? []).map((value) => ({
            value,
            label: enumValueToLabel(value),
          }))
        : undefined,
    };
  });

  return [titleField, ...fieldConfigs];
}

// Auth-related field names (username, password, etc.)
const AUTH_FIELD_NAMES = [
  'username',
  'user',
  'password',
  'pass',
  'secret',
  'token',
  'apikey',
  'api_key',
  'accesskey',
  'access_key',
  'credentials',
];

// Key-based auth field names (SSH keys, certificates)
const KEY_AUTH_FIELD_NAMES = [
  'privatekey',
  'private_key',
  'publickey',
  'public_key',
  'passphrase',
  'certificate',
  'cert',
  'sshkey',
  'ssh_key',
];

/**
 * Checks if a field name matches auth-related patterns
 */
function isAuthField(fieldName: string): boolean {
  const lower = fieldName.toLowerCase().replace(/[_-]/g, '');
  return AUTH_FIELD_NAMES.some((name) =>
    lower.includes(name.replace(/[_-]/g, ''))
  );
}

/**
 * Checks if a field name matches key-based auth patterns
 */
function isKeyAuthField(fieldName: string): boolean {
  const lower = fieldName.toLowerCase().replace(/[_-]/g, '');
  return KEY_AUTH_FIELD_NAMES.some((name) =>
    lower.includes(name.replace(/[_-]/g, ''))
  );
}

/**
 * Groups fields into sections based on their characteristics
 */
function groupFieldsIntoSections(fieldsConfig: FieldConfig[]): {
  serverFields: FieldConfig[];
  authFields: FieldConfig[];
  keyAuthFields: FieldConfig[];
} {
  const serverFields: FieldConfig[] = [];
  const authFields: FieldConfig[] = [];
  const keyAuthFields: FieldConfig[] = [];

  for (const field of fieldsConfig) {
    const fieldName = field.fieldName || field.name;

    // Key-based auth fields (optional secrets like private_key, passphrase)
    if (isKeyAuthField(fieldName) || (field.isSecret && field.isOptional)) {
      keyAuthFields.push(field);
    }
    // Auth fields (username, password, etc.)
    else if (field.isSecret || isAuthField(fieldName)) {
      authFields.push(field);
    }
    // Server/connection fields (title, host, port, etc.)
    else {
      serverFields.push(field);
    }
  }

  return { serverFields, authFields, keyAuthFields };
}

/**
 * Builds initial values from ConnectionFieldDto array
 */
function buildInitialValues(
  fields: ConnectionFieldDto[]
): Record<string, unknown> {
  const values: Record<string, unknown> = {
    title: '',
  };

  for (const field of fields) {
    // Use defaultValue if provided, otherwise fall back to placeholder for numeric fields
    // This ensures fields like "port" with placeholder "22" get the value set as default
    const typeName = field.typeName?.toLowerCase() ?? '';
    const isNumericField = ['u16', 'u32', 'i32', 'number'].includes(typeName);
    const isArrayField = ARRAY_TYPE_NAMES.includes(typeName);
    const isKeyValueField = KEYVALUE_TYPE_NAMES.includes(typeName);
    const hasEnumValues = !!field.enumValues && field.enumValues.length > 0;

    if (hasEnumValues) {
      const defaults = field.enumValues ?? [];
      values[field.name] =
        field.defaultValue && defaults.includes(field.defaultValue)
          ? field.defaultValue
          : defaults[0] ?? '';
    } else if (typeName === 'bool') {
      values[field.name] = field.defaultValue === 'true' ? true : false;
    } else if (isArrayField) {
      values[field.name] = field.defaultValue
        ? field.defaultValue
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
        : [];
    } else if (isKeyValueField) {
      values[field.name] = {};
    } else if (field.defaultValue) {
      values[field.name] = field.defaultValue;
    } else if (isNumericField && field.placeholder) {
      // For numeric fields, use placeholder as default value if no defaultValue is set
      values[field.name] = field.placeholder;
    } else {
      values[field.name] = '';
    }
  }

  return values;
}

type DynamicConnectionFormProps = {
  connectionType: ConnectionTypeDto;
  initValues?: Record<string, unknown>;
  isLoading?: boolean;
  onSubmit: (data: Record<string, unknown>) => void;
  mode?: 'create' | 'edit';
  onDelete?: () => void;
  isDeleting?: boolean;
  /** Live rate-limit status for this connection (edit mode), if loaded. */
  rateLimitStatus?: RateLimitStatusDto | null;
  /** Show the interactive-OAuth "Reconnect" affordance (edit mode, OAuth types). */
  showReconnect?: boolean;
  onReconnect?: () => void;
  isReconnecting?: boolean;
  /** When true the connection needs re-authorization. */
  needsReconnect?: boolean;
};

/**
 * Dynamic form component that generates fields based on ConnectionTypeDto.fields
 */
export function DynamicConnectionForm({
  connectionType,
  initValues,
  isLoading,
  onSubmit,
  mode = 'create',
  onDelete,
  isDeleting,
  rateLimitStatus,
  showReconnect,
  onReconnect,
  isReconnecting,
  needsReconnect,
}: DynamicConnectionFormProps) {
  const isFileStorage = FILE_STORAGE_CATEGORIES.has(
    connectionType.category ?? ''
  );

  const { schema, fieldsConfig, initialValues, groupedFields } = useMemo(() => {
    const fields = connectionType.fields || [];
    const config = buildFieldsConfig(fields);

    const baseSchema = buildSchema(fields);
    const rateLimitFields = z.object({
      rateLimitEnabled: z.boolean(),
      requestsPerSecond: z.coerce.number().min(0).optional(),
      burstSize: z.coerce.number().min(0).optional(),
      maxRetries: z.coerce.number().min(0).optional(),
      maxWaitMs: z.coerce.number().min(0).optional(),
      retryOnLimit: z.boolean(),
      defaultFor: z.array(z.string()).optional(),
    });
    const fileStorageFields = z.object({
      isDefaultFileStorage: z.boolean(),
    });

    const mergedSchema = isFileStorage
      ? baseSchema.merge(rateLimitFields).merge(fileStorageFields)
      : baseSchema.merge(rateLimitFields);

    // When the override is enabled the rate-limit numbers must be enforceable
    // (mirrors the backend validate_rate_limit_config): rps >= 1 (0 silently
    // disables enforcement), burst >= 1 and burst >= rps, plus sane caps. SYN-500.
    const refinedSchema = mergedSchema.superRefine((data, ctx) => {
      const d = data as Record<string, unknown>;
      if (!d.rateLimitEnabled) return;
      const rps = Number(d.requestsPerSecond);
      const burst = Number(d.burstSize);
      if (!Number.isFinite(rps) || rps < 1) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['requestsPerSecond'],
          message:
            'Must be at least 1 — 0 disables rate limiting. Turn off the override to leave this connection unlimited.',
        });
      }
      if (!Number.isFinite(burst) || burst < 1) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['burstSize'],
          message: 'Must be at least 1.',
        });
      }
      if (Number.isFinite(rps) && Number.isFinite(burst) && burst < rps) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['burstSize'],
          message: 'Burst size must be ≥ requests per second.',
        });
      }
      const maxRetries = Number(d.maxRetries);
      if (Number.isFinite(maxRetries) && maxRetries > 100) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['maxRetries'],
          message: 'Must be 100 or fewer.',
        });
      }
      const maxWaitMs = Number(d.maxWaitMs);
      if (Number.isFinite(maxWaitMs) && maxWaitMs > 3_600_000) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: ['maxWaitMs'],
          message: 'Must be 3600000 ms (1 hour) or less.',
        });
      }
    });

    return {
      schema: refinedSchema,
      fieldsConfig: config,
      initialValues: buildInitialValues(fields),
      groupedFields: groupFieldsIntoSections(config),
    };
  }, [connectionType.fields, isFileStorage]);

  const rateLimitDefaults = useMemo(() => {
    const defaultConfig = connectionType.defaultRateLimitConfig;
    const existingConfig = initValues?.rateLimitConfig as
      | RateLimitConfigDto
      | null
      | undefined;

    if (existingConfig) {
      return {
        rateLimitEnabled: true,
        requestsPerSecond: existingConfig.requestsPerSecond,
        burstSize: existingConfig.burstSize,
        maxRetries: existingConfig.maxRetries,
        maxWaitMs: existingConfig.maxWaitMs,
        retryOnLimit: existingConfig.retryOnLimit,
      };
    }

    return {
      rateLimitEnabled: false,
      requestsPerSecond: defaultConfig?.requestsPerSecond ?? '',
      burstSize: defaultConfig?.burstSize ?? '',
      maxRetries: defaultConfig?.maxRetries ?? '',
      maxWaitMs: defaultConfig?.maxWaitMs ?? '',
      retryOnLimit: defaultConfig?.retryOnLimit ?? true,
    };
  }, [connectionType.defaultRateLimitConfig, initValues]);

  const fileStorageDefaults = isFileStorage
    ? { isDefaultFileStorage: !!(initValues as any)?.isDefaultFileStorage }
    : {};
  const defaultForDefaults = {
    defaultFor: Array.isArray((initValues as any)?.defaultFor)
      ? ((initValues as any).defaultFor as string[])
      : [],
  };

  const form = useForm({
    resolver: zodResolver(schema),
    defaultValues: {
      ...initialValues,
      ...rateLimitDefaults,
      ...fileStorageDefaults,
      ...defaultForDefaults,
    },
    values: initValues
      ? {
          ...initValues,
          ...rateLimitDefaults,
          ...fileStorageDefaults,
          ...defaultForDefaults,
        }
      : undefined,
  });

  const handleSubmit = (values: Record<string, unknown>) => {
    onSubmit(values);
  };

  const editNotice =
    mode === 'edit'
      ? 'Stored secrets stay hidden. Enter new values to update them.'
      : undefined;

  const integrationIcon = (
    <ServiceIcon
      serviceId={connectionType.integrationId || undefined}
      category={connectionType.category || undefined}
    />
  );

  const renderContent = () => (
    <ConnectionFormLayout
      title={mode === 'edit' ? 'Edit connection' : 'Create connection'}
      isLoading={isLoading}
      submitLabel={mode === 'edit' ? 'Save changes' : 'Create connection'}
      loadingLabel={mode === 'edit' ? 'Saving...' : 'Creating...'}
      editNotice={editNotice}
      integrationIcon={integrationIcon}
      integrationName={connectionType.displayName || undefined}
      integrationCategory={connectionType.category || undefined}
      onDelete={onDelete}
      isDeleting={isDeleting}
      showReconnect={showReconnect}
      onReconnect={onReconnect}
      isReconnecting={isReconnecting}
      needsReconnect={needsReconnect}
    >
      <div className="space-y-6">
        {/* Server Details Section */}
        {groupedFields.serverFields.length > 0 && (
          <FormSection title="Server Details" icon={Server}>
            <FormContent
              fieldsConfig={groupedFields.serverFields}
              className="grid-cols-3 gap-4"
            />
          </FormSection>
        )}

        {/* Authentication Section */}
        {groupedFields.authFields.length > 0 && (
          <FormSection title="Authentication" icon={Key}>
            <FormContent
              fieldsConfig={groupedFields.authFields}
              className="grid-cols-1 gap-4"
            />
          </FormSection>
        )}

        {/* Key-based Authentication Section */}
        {groupedFields.keyAuthFields.length > 0 && (
          <FormSection title="Key-based Authentication" icon={Shield} optional>
            <FormContent
              fieldsConfig={groupedFields.keyAuthFields}
              className="grid-cols-1 gap-4"
            />
          </FormSection>
        )}

        {/* Default File Storage Section (S3-compatible only) */}
        {isFileStorage && <DefaultFileStorageSection />}

        <DefaultForSection connectionType={connectionType} />

        {/* Rate Limiting Section */}
        <RateLimitSection
          defaultConfig={connectionType.defaultRateLimitConfig}
          liveStatus={rateLimitStatus}
        />
      </div>
    </ConnectionFormLayout>
  );

  return (
    <NextForm
      form={form}
      fieldsConfig={fieldsConfig}
      renderContent={renderContent}
      renderActions={() => null}
      onSubmit={handleSubmit}
      className="w-full"
    />
  );
}
