export type FormFieldType =
  | 'string'
  | 'integer'
  | 'number'
  | 'boolean'
  | 'array'
  | 'object'
  | 'file';

export type FieldAccessMode = 'read_write' | 'read' | 'write';

export type FormControlKind =
  | 'text'
  | 'textarea'
  | 'secret_textarea'
  | 'password'
  | 'number'
  | 'toggle'
  | 'select'
  | 'multi_select'
  | 'radio'
  | 'date'
  | 'datetime'
  | 'date_range'
  | 'number_range'
  | 'tags'
  | 'key_value'
  | 'lookup'
  | 'file';

export interface FormOption {
  value: unknown;
  label: string;
}

export interface FormControl {
  kind: FormControlKind;
  options?: FormOption[];
}

export interface FormConditions {
  visible?: unknown;
  enabled?: unknown;
  required?: unknown;
}

export interface FormField {
  type: FormFieldType;
  description?: string;
  required?: boolean;
  default?: unknown;
  example?: unknown;
  items?: FormField;
  enum?: unknown[];
  label?: string;
  placeholder?: string;
  order?: number;
  format?: string;
  min?: number;
  max?: number;
  pattern?: string;
  properties?: Record<string, FormField>;
  nullable?: boolean;
  control?: FormControl;
  section?: string;
  conditions?: FormConditions;
  access?: FieldAccessMode;
  secret?: boolean;
}

export interface FormSectionDefinition {
  id: string;
  label: string;
  description?: string;
  order?: number;
  advanced?: boolean;
  conditions?: FormConditions;
}

export interface FormDefinition {
  schemaVersion?: number;
  fields: Record<string, FormField>;
  sections?: FormSectionDefinition[];
  allowUnknownFields?: boolean;
}

export interface FormIssue {
  code: string;
  path: string;
  message: string;
  severity: 'error' | 'warning';
}

export interface FormFieldState {
  visible: boolean;
  enabled: boolean;
  required: boolean;
}

export interface FormAnalysisResult {
  success: boolean;
  valid: boolean;
  status: 'valid' | 'invalid' | 'unavailable';
  fields: Record<string, FormFieldState>;
  issues: FormIssue[];
  message: string;
  wasmAvailable: boolean;
  unavailableReason?: string;
}
