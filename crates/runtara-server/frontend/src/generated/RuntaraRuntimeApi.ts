/* eslint-disable */
/* tslint:disable */
// @ts-nocheck
/*
 * ---------------------------------------------------------------
 * ## THIS FILE WAS GENERATED VIA SWAGGER-TYPESCRIPT-API        ##
 * ##                                                           ##
 * ## AUTHOR: acacode                                           ##
 * ## SOURCE: https://github.com/acacode/swagger-typescript-api ##
 * ---------------------------------------------------------------
 */

/**
 * Data types for variables.
 * Matches the operator field types for consistency.
 */
export enum VariableType {
  String = "string",
  Number = "number",
  Integer = "integer",
  Boolean = "boolean",
  Array = "array",
  Object = "object",
  File = "file",
}

/**
 * Type hints for reference values.
 * Used to interpret data from unknown sources (e.g., HTTP responses).
 *
 * Note: Type names are aligned with VariableType for consistency:
 * - `integer` for whole numbers
 * - `number` for floating point
 * - `boolean` for true/false
 * - `json` for pass-through JSON (distinct from `object`/`array` in VariableType)
 */
export enum ValueType {
  String = "string",
  Integer = "integer",
  Number = "number",
  Boolean = "boolean",
  Json = "json",
  File = "file",
}

/** Trigger type for invocation triggers */
export enum TriggerType {
  HTTP = "HTTP",
  CRON = "CRON",
  EMAIL = "EMAIL",
  APPLICATION = "APPLICATION",
  CHANNEL = "CHANNEL",
}

/**
 * Optional secondary text-index annotation for string-typed columns.
 *
 * `Trigram` causes a `gin_trgm_ops` GIN index to be created alongside the
 * table, which speeds up `SIMILARITY_GTE` and `similarity()` scoring.
 */
export enum TextIndexKind {
  None = "none",
  Trigram = "trigram",
}

/** Termination type providing context for why an execution terminated */
export enum TerminationType {
  NormalCompletion = "normal_completion",
  UserInitiated = "user_initiated",
  QueueTimeout = "queue_timeout",
  ExecutionTimeout = "execution_timeout",
  SystemError = "system_error",
}

/**
 * Match type for switch cases.
 * Supports all ConditionOperator values plus compound match types.
 */
export enum SwitchMatchType {
  GT = "GT",
  GTE = "GTE",
  LT = "LT",
  LTE = "LTE",
  EQ = "EQ",
  NE = "NE",
  STARTS_WITH = "STARTS_WITH",
  ENDS_WITH = "ENDS_WITH",
  CONTAINS = "CONTAINS",
  IN = "IN",
  NOT_IN = "NOT_IN",
  IS_DEFINED = "IS_DEFINED",
  IS_EMPTY = "IS_EMPTY",
  IS_NOT_EMPTY = "IS_NOT_EMPTY",
  BETWEEN = "BETWEEN",
  RANGE = "RANGE",
}

/** Sort direction. JSON encoding is UPPERCASE (`"ASC"` / `"DESC"`). */
export enum SortDirection {
  ASC = "ASC",
  DESC = "DESC",
}

/**
 * Data types for schema fields.
 * Used in input/output schema definitions.
 */
export enum SchemaFieldType {
  String = "string",
  Integer = "integer",
  Number = "number",
  Boolean = "boolean",
  Array = "array",
  Object = "object",
  File = "file",
}

/** Rate limit event types */
export enum RateLimitEventType {
  Request = "request",
  RateLimited = "rate_limited",
  Retry = "retry",
}

/** Memory allocation tier for workflow execution */
export enum MemoryTier {
  S = "S",
  M = "M",
  L = "L",
  XL = "XL",
}

/** Log level for Log steps */
export enum LogLevel {
  Debug = "debug",
  Info = "info",
  Warn = "warn",
  Error = "error",
}

/** Severity of a validation issue */
export enum IssueSeverity {
  Error = "error",
  Warning = "warning",
}

/** Category of validation issue */
export enum IssueCategory {
  MissingStep = "missing_step",
  UnknownFieldPath = "unknown_field_path",
  InvalidReferencePath = "invalid_reference_path",
  MissingConnection = "missing_connection",
}

/** Execution status representing the current state of a workflow execution */
export enum ExecutionStatus {
  Queued = "queued",
  Compiling = "compiling",
  Running = "running",
  Suspended = "suspended",
  Completed = "completed",
  Failed = "failed",
  Timeout = "timeout",
  Cancelled = "cancelled",
}

/** Error severity for logging and alerting. */
export enum ErrorSeverity {
  Info = "info",
  Warning = "warning",
  Error = "error",
  Critical = "critical",
}

/**
 * Error category for structured errors.
 * Determines retry behavior.
 *
 * Two categories:
 * - **Transient**: Auto-retry likely to succeed (network, timeout, rate limit)
 * - **Permanent**: Don't auto-retry (validation, not found, auth, business rules)
 *
 * To distinguish technical vs business errors within Permanent, use:
 * - `code`: e.g., `VALIDATION_*` vs `BUSINESS_*` or `CREDIT_LIMIT_EXCEEDED`
 * - `severity`: `error` for technical, `warning` for expected business outcomes
 */
export enum ErrorCategory {
  Transient = "transient",
  Permanent = "permanent",
}

export enum ConnectionStatus {
  UNKNOWN = "UNKNOWN",
  ACTIVE = "ACTIVE",
  REQUIRES_RECONNECTION = "REQUIRES_RECONNECTION",
  INVALID_CREDENTIALS = "INVALID_CREDENTIALS",
}

/**
 * Canonical list of connection categories.
 *
 * Used for grouping connection types in the UI and API responses.
 * When adding a new integration, pick the most specific category that fits.
 */
export enum ConnectionCategory {
  Ecommerce = "ecommerce",
  FileStorage = "file_storage",
  Llm = "llm",
  Crm = "crm",
  Erp = "erp",
  Database = "database",
  Email = "email",
  Messaging = "messaging",
  Payment = "payment",
  Cloud = "cloud",
  Api = "api",
}

/**
 * Canonical list of authentication / credential types for connections.
 *
 * Describes **what credentials** are used to authenticate, not how they are
 * transported (e.g. bearer header is a delivery mechanism, not a credential type).
 */
export enum ConnectionAuthType {
  ApiKey = "api_key",
  Oauth2AuthorizationCode = "oauth2_authorization_code",
  Oauth2ClientCredentials = "oauth2_client_credentials",
  UsernamePassword = "username_password",
  SshKey = "ssh_key",
  AccessKey = "access_key",
  ConnectionString = "connection_string",
  Custom = "custom",
}

/** Condition expression operators */
export enum ConditionOperator {
  AND = "AND",
  OR = "OR",
  NOT = "NOT",
  GT = "GT",
  GTE = "GTE",
  LT = "LT",
  LTE = "LTE",
  EQ = "EQ",
  NE = "NE",
  STARTS_WITH = "STARTS_WITH",
  ENDS_WITH = "ENDS_WITH",
  CONTAINS = "CONTAINS",
  IN = "IN",
  NOT_IN = "NOT_IN",
  LENGTH = "LENGTH",
  IS_DEFINED = "IS_DEFINED",
  IS_EMPTY = "IS_EMPTY",
  IS_NOT_EMPTY = "IS_NOT_EMPTY",
  SIMILARITY_GTE = "SIMILARITY_GTE",
  MATCH = "MATCH",
  COSINE_DISTANCE_LTE = "COSINE_DISTANCE_LTE",
  L2_DISTANCE_LTE = "L2_DISTANCE_LTE",
}

/** Strategy for compacting conversation memory. */
export enum CompactionStrategy {
  Summarize = "summarize",
  SlidingWindow = "slidingWindow",
}

/** Behavior on per-row validation failure for bulk-create. */
export enum BulkValidationMode {
  Stop = "stop",
  Skip = "skip",
}

/** Behavior on unique-key conflict for bulk-create. */
export enum BulkConflictMode {
  Error = "error",
  Skip = "skip",
  Upsert = "upsert",
}

/** LLM provider used by an AI Agent step. */
export enum AiAgentProvider {
  Openai = "openai",
  Bedrock = "bedrock",
}

/** Aggregate function. JSON encoding is SCREAMING_SNAKE_CASE. */
export enum AggregateFn {
  COUNT = "COUNT",
  SUM = "SUM",
  AVG = "AVG",
  MIN = "MIN",
  MAX = "MAX",
  FIRST_VALUE = "FIRST_VALUE",
  LAST_VALUE = "LAST_VALUE",
  PERCENTILE_CONT = "PERCENTILE_CONT",
  PERCENTILE_DISC = "PERCENTILE_DISC",
  STDDEV_SAMP = "STDDEV_SAMP",
  VAR_SAMP = "VAR_SAMP",
  EXPR = "EXPR",
}

/** API-compatible agent info */
export interface AgentInfo {
  capabilities: CapabilityInfo[];
  description: string;
  hasSideEffects: boolean;
  id: string;
  integrationIds: string[];
  name: string;
  supportsConnections: boolean;
}

/** Executes an agent capability */
export interface AgentStep {
  /** Agent name (e.g., "utils", "transform", "http", "sftp") */
  agentId: string;
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Capability name (e.g., "random-double", "group-by", "http-request") */
  capabilityId: string;
  /**
   * Compensation configuration for saga pattern support.
   * Defines what step to execute to rollback this step's effects on failure.
   */
  compensation?: null | CompensationConfig;
  /** Connection ID for agents requiring authentication */
  connectionId?: string | null;
  /**
   * Disable durability for this step when `Some(false)`. Skips checkpoint
   * read/write around the capability call. Ignored when the enclosing
   * workflow is already non-durable. Defaults to the workflow setting.
   */
  durable?: boolean | null;
  /** Unique step identifier */
  id: string;
  /** Maps data to agent capability inputs */
  inputMapping?: null | HashMap;
  /**
   * Maximum retry attempts (default: 3)
   * @format int32
   * @min 0
   */
  maxRetries?: number | null;
  /** Human-readable step name */
  name?: string | null;
  /**
   * Base delay between retries in milliseconds (default: 1000)
   * @format int64
   * @min 0
   */
  retryDelay?: number | null;
  /**
   * Step timeout in milliseconds. If exceeded, step fails.
   * @format int64
   * @min 0
   */
  timeout?: number | null;
}

/** Simplified agent info without capabilities (for list endpoint) */
export interface AgentSummary {
  description: string;
  id: string;
  name: string;
}

export interface AggregateOrderBy {
  column: string;
  /** Sort direction. JSON encoding is UPPERCASE (`"ASC"` / `"DESC"`). */
  direction?: SortDirection;
}

export interface AggregateRequest {
  aggregates: AggregateSpec[];
  condition?: null | Condition;
  groupBy?: string[];
  /** @format int64 */
  limit?: number | null;
  /** @format int64 */
  offset?: number | null;
  orderBy?: AggregateOrderBy[];
}

export interface AggregateResponse {
  columns: string[];
  error?: string | null;
  /** @format int64 */
  groupCount: number;
  rows: any[][];
  success: boolean;
}

export interface AggregateSpec {
  /** Output column name. Must match `[a-zA-Z_][a-zA-Z0-9_]*` and be unique. */
  alias: string;
  /**
   * Source column. Optional for COUNT (COUNT(*)); required otherwise.
   * Must be omitted for EXPR.
   */
  column?: string | null;
  /** Apply DISTINCT. Only valid with `fn = COUNT` and a non-null `column`. */
  distinct?: boolean;
  /**
   * Required for EXPR — an expression tree referencing prior aliases and
   * constants. Rejected for every other function.
   */
  expression?: any;
  /** Aggregate function. One of COUNT, SUM, MIN, MAX, FIRST_VALUE, LAST_VALUE, EXPR. */
  fn: AggregateFn;
  /** Required for FIRST_VALUE / LAST_VALUE; rejected for others. */
  orderBy?: AggregateOrderBy[];
  /**
   * Fraction in `[0.0, 1.0]` for `PERCENTILE_CONT` / `PERCENTILE_DISC`.
   * Required for those functions, rejected otherwise.
   * @format double
   */
  percentile?: number | null;
}

/** Configuration for the AI Agent step. */
export interface AiAgentConfig {
  /**
   * Maximum number of tool-call iterations before stopping (default: 10)
   * @format int32
   * @min 0
   */
  maxIterations?: number | null;
  /**
   * Maximum tokens per LLM call
   * @format int64
   * @min 0
   */
  maxTokens?: number | null;
  /**
   * Conversation memory configuration.
   * Requires a "memory" labeled edge pointing to a memory provider Agent step.
   */
  memory?: null | AiAgentMemory;
  /** LLM model identifier (e.g., "gpt-4o", "claude-sonnet-4-20250514") */
  model?: string | null;
  /**
   * Output schema for structured responses (DSL flat-map format).
   *
   * When set, the LLM is instructed to return JSON matching this schema
   * via the provider's structured output feature (e.g., OpenAI `response_format`,
   * Anthropic `response_format`).
   *
   * Uses the same `SchemaField` format as workflow `inputSchema`/`outputSchema`.
   *
   * Example:
   * ```json
   * {
   *   "sentiment": { "type": "string", "required": true, "enum": ["positive", "negative", "neutral"] },
   *   "confidence": { "type": "number", "required": true },
   *   "reasoning": { "type": "string", "required": false }
   * }
   * ```
   *
   * The step output `response` will be a parsed JSON object instead of a string.
   */
  outputSchema?: Partial<Record<string, SchemaField>> | null;
  /** LLM provider to use for the agent brain. */
  provider: AiAgentProvider;
  /** System prompt / instructions for the LLM */
  systemPrompt: MappingValue;
  /**
   * Temperature for LLM sampling (default: 0.7)
   * @format double
   */
  temperature?: number | null;
  /** User message / request to process */
  userPrompt: MappingValue;
}

/**
 * Conversation memory configuration for the AI Agent step.
 *
 * Memory allows conversation history to persist across:
 * - Multiple iterations within one execution
 * - Multiple AI Agent steps in the same execution (shared by conversation_id)
 * - Multiple executions (cross-execution memory via an external conversation key)
 *
 * The actual storage is delegated to a memory provider agent connected via
 * a "memory" labeled edge. The provider must implement `load_memory` and
 * `save_memory` capabilities.
 */
export interface AiAgentMemory {
  /**
   * Compaction configuration — controls how old messages are handled
   * when the conversation grows beyond a threshold.
   */
  compaction?: null | CompactionConfig;
  /**
   * Identifier for the conversation thread.
   * Can be a reference (e.g., `data.sessionId`) or an immediate value.
   * All AI Agent steps sharing the same conversation_id share memory.
   */
  conversationId: MappingValue;
}

/**
 * LLM-driven agent that selects and calls tools in a loop.
 *
 * The AI Agent step uses an LLM to autonomously decide which tools to call.
 * Tools are defined as labeled edges in the execution plan, each pointing to
 * a concrete step (Agent, EmbedWorkflow, WaitForSignal). The LLM picks which
 * tool/branch to execute, collects the result, and loops until it produces a
 * final text response or reaches max_iterations.
 *
 * Without tool edges, it acts as a simple LLM completion step.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "AiAgent",
 *   "id": "assistant",
 *   "name": "Inventory Assistant",
 *   "connectionId": "conn-openai",
 *   "config": {
 *     "systemPrompt": { "valueType": "immediate", "value": "You are an inventory manager" },
 *     "userPrompt": { "valueType": "reference", "value": "data.userRequest" },
 *     "model": "gpt-4o",
 *     "maxIterations": 10,
 *     "temperature": 0.7
 *   }
 * }
 * ```
 */
export interface AiAgentStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** AI Agent configuration */
  config?: null | AiAgentConfig;
  /** Connection ID for the LLM provider (e.g., OpenAI, Anthropic) */
  connectionId?: string | null;
  /**
   * Disable durability for this step when `Some(false)`. Skips checkpoint
   * on each tool call and LLM call inside this agent's loop. Ignored when
   * the enclosing workflow is already non-durable.
   */
  durable?: boolean | null;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/** API key record (key_hash is never exposed via serde skip) */
export interface ApiKey {
  created_at: string;
  created_by?: string | null;
  expires_at?: string | null;
  /** @format uuid */
  id: string;
  is_revoked: boolean;
  key_prefix: string;
  last_used_at?: string | null;
  name: string;
  org_id: string;
}

/** Generic API response wrapper */
export interface ApiResponseInvocationTrigger {
  /** Invocation trigger model */
  data: {
    /**
     * Whether the trigger is currently active
     * @example true
     */
    active: boolean;
    /** Trigger-specific configuration in JSON format */
    configuration?: object | null;
    /**
     * Timestamp when the trigger was created
     * @example "2025-01-15T10:30:00Z"
     */
    created_at: string;
    /**
     * Unique identifier for the invocation trigger (auto-generated)
     * @example "550e8400-e29b-41d4-a716-446655440000"
     */
    id: string;
    /**
     * Timestamp of the last trigger execution (system-managed)
     * @example "2025-01-15T12:00:00Z"
     */
    last_run?: string | null;
    /**
     * Remote tenant identifier for external system triggers
     * @example "remote-tenant-789"
     */
    remote_tenant_id?: string | null;
    /**
     * Whether only a single instance of this trigger should run at a time
     * @example false
     */
    single_instance: boolean;
    /**
     * Tenant identifier for multi-tenancy support
     * @example "tenant-123"
     */
    tenant_id?: string | null;
    /** Type of trigger */
    trigger_type: TriggerType;
    /**
     * Timestamp when the trigger was last updated
     * @example "2025-01-15T10:30:00Z"
     */
    updated_at: string;
    /**
     * Reference to the workflow to be invoked
     * @example "workflow-456"
     */
    workflow_id: string;
  };
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponseMoveWorkflowResponse {
  /** Response for move workflow operation */
  data: {
    path: string;
    success: boolean;
    workflowId: string;
  };
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponsePageWorkflowDto {
  /** Paginated response for workflow listings (matches Spring Boot Page format) */
  data: {
    content: WorkflowDto[];
    first: boolean;
    last: boolean;
    /**
     * Current page number (0-based)
     * @format int32
     */
    number: number;
    /** @format int32 */
    numberOfElements: number;
    /** @format int32 */
    size: number;
    /** @format int64 */
    totalElements: number;
    /** @format int32 */
    totalPages: number;
  };
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponseRenameFolderResponse {
  /** Response for rename folder operation */
  data: {
    newPath: string;
    oldPath: string;
    success: boolean;
    /**
     * Number of workflows updated
     * @format int64
     * @min 0
     */
    workflowsUpdated: number;
  };
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponseVecInvocationTrigger {
  data: {
    /**
     * Whether the trigger is currently active
     * @example true
     */
    active: boolean;
    /** Trigger-specific configuration in JSON format */
    configuration?: object | null;
    /**
     * Timestamp when the trigger was created
     * @example "2025-01-15T10:30:00Z"
     */
    created_at: string;
    /**
     * Unique identifier for the invocation trigger (auto-generated)
     * @example "550e8400-e29b-41d4-a716-446655440000"
     */
    id: string;
    /**
     * Timestamp of the last trigger execution (system-managed)
     * @example "2025-01-15T12:00:00Z"
     */
    last_run?: string | null;
    /**
     * Remote tenant identifier for external system triggers
     * @example "remote-tenant-789"
     */
    remote_tenant_id?: string | null;
    /**
     * Whether only a single instance of this trigger should run at a time
     * @example false
     */
    single_instance: boolean;
    /**
     * Tenant identifier for multi-tenancy support
     * @example "tenant-123"
     */
    tenant_id?: string | null;
    /** Type of trigger */
    trigger_type: TriggerType;
    /**
     * Timestamp when the trigger was last updated
     * @example "2025-01-15T10:30:00Z"
     */
    updated_at: string;
    /**
     * Reference to the workflow to be invoked
     * @example "workflow-456"
     */
    workflow_id: string;
  }[];
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponseVecWorkflowVersionInfoDto {
  data: {
    /** Whether this version has been compiled */
    compiled: boolean;
    /** Timestamp when this version was compiled (RFC3339 format, null if not compiled) */
    compiledAt?: string | null;
    createdAt: string;
    /** Whether this is the current/active version used for execution */
    isActive: boolean;
    /** Whether step-event tracking is enabled for this version */
    trackEvents: boolean;
    updatedAt: string;
    versionId: string;
    /** @format int32 */
    versionNumber: number;
    workflowId: string;
  }[];
  message: string;
  success: boolean;
}

/** Generic API response wrapper */
export interface ApiResponseWorkflowDto {
  data: {
    created: string;
    /**
     * The active/current version that will be used when executing this workflow
     * Can be set explicitly via the set-current-version endpoint, otherwise defaults to latest_version
     * @format int32
     */
    currentVersionNumber: number;
    description: string;
    executionGraph: any;
    /** @format int64 */
    executionTime?: number | null;
    /** @format int64 */
    executionTimeout?: number | null;
    finished?: string | null;
    id: string;
    inputSchema: any;
    /**
     * The highest version number that exists for this workflow
     * @format int32
     */
    lastVersionNumber: number;
    /** Memory allocation tier for workflow execution */
    memoryTier?: MemoryTier;
    name: string;
    /** Visual notes/annotations for the workflow canvas */
    notes?: Note[];
    outputSchema: any;
    /**
     * Folder path for organization (e.g., "/Sales/Shopify/")
     * Defaults to "/" (root folder)
     */
    path?: string;
    started?: string | null;
    /** Whether this version is compiled with step-event tracking instrumentation */
    trackEvents?: boolean;
    updated: string;
    /** Default variable values (can be overridden at execution time) */
    variables?: any;
  };
  message: string;
  success: boolean;
}

export interface BucketDto {
  /** Creation date (ISO 8601) */
  creationDate: string;
  /** Bucket name */
  name: string;
}

/**
 * Bulk create request supporting two input shapes.
 *
 * **Object form** — each record as a JSON object:
 * ```jsonc
 * { "instances": [ { "sku": "A", "qty": 1 }, ... ] }
 * ```
 *
 * **Columnar form** — column names once, rows as arrays of values. Optional
 * `constants` are merged into every row (row values win on overlap). Use for
 * large, uniform payloads (snapshots, CSV-style writes) to avoid repeating
 * column keys.
 * ```jsonc
 * {
 *   "columns": ["sku", "qty"],
 *   "rows":    [["A", 1], ["B", 2]],
 *   "constants":          { "snapshot_date": "2026-04-18" },
 *   "nullifyEmptyStrings": true
 * }
 * ```
 *
 * Exactly one of (`instances`) or (`columns` + `rows`) must be provided.
 */
export interface BulkCreateRequest {
  /** Columnar form — column names (length N). Must be paired with `rows`. */
  columns?: string[] | null;
  /** Columns used to detect conflicts. Required when `onConflict` is `skip` or `upsert`. */
  conflictColumns?: string[];
  /**
   * Columnar form — fields merged into every row. Row cell values take
   * precedence over constants when both provide the same column.
   */
  constants?: Partial<Record<string, any>>;
  /** Object form — array of JSON objects, one per record. */
  instances?: any[] | null;
  /**
   * Columnar form — when true, empty strings in non-string columns are
   * converted to `null` before validation. Useful when ingesting from
   * sources (CSV, SFTP) where missing values come through as "".
   */
  nullifyEmptyStrings?: boolean;
  /** How to handle unique-key conflicts (default `error`). */
  onConflict?: BulkConflictMode;
  /** How to handle per-row validation failures (default `stop`). */
  onError?: BulkValidationMode;
  /** Columnar form — each row is an array of values aligned to `columns`. */
  rows?: any[][] | null;
}

export interface BulkCreateResponse {
  /** @format int64 */
  createdCount: number;
  errors?: BulkRowError[];
  message: string;
  /** @format int64 */
  skippedCount?: number;
  success: boolean;
}

export interface BulkDeleteRequest {
  instanceIds: string[];
}

export interface BulkDeleteResponse {
  /** @min 0 */
  deletedCount: number;
  message: string;
  success: boolean;
}

export interface BulkRowError {
  /** @min 0 */
  index: number;
  reason: string;
}

export interface BulkUpdateByIdEntry {
  id: string;
  properties: any;
}

/**
 * Bulk update request. The `mode` field selects between two semantics:
 * - `byCondition` — apply the same `properties` to every row matching `condition`.
 * - `byIds` — apply per-row `properties` to each listed `id`.
 */
export type BulkUpdateRequest =
  | {
      condition: Condition;
      mode: "byCondition";
      properties: any;
    }
  | {
      mode: "byIds";
      updates: BulkUpdateByIdEntry[];
    };

export interface BulkUpdateResponse {
  message: string;
  success: boolean;
  /** @format int64 */
  updatedCount: number;
}

/**
 * API-compatible capability field info.
 * Used for agent inputs and workflow input/output schemas.
 */
export interface CapabilityField {
  default?: any;
  description?: string | null;
  displayName?: string | null;
  enum?: string[] | null;
  example?: any;
  format?: string | null;
  items?: null | FieldTypeInfo;
  name: string;
  required: boolean;
  /**
   * JSON Schema for complex object types (e.g., ConditionExpression)
   * Provides detailed structure hints for strongly-typed objects.
   */
  schema?: any;
  type: string;
}

/** API-compatible capability info */
export interface CapabilityInfo {
  /**
   * Optional compensation hint - suggests how to undo this capability.
   * This is metadata only; the system never auto-compensates.
   */
  compensationHint?: null | CompensationHintInfo;
  description?: string | null;
  displayName?: string | null;
  hasSideEffects: boolean;
  id: string;
  inputType: string;
  inputs: CapabilityField[];
  isIdempotent: boolean;
  /**
   * Known errors this capability can return.
   * Used for tooling hints and documentation.
   */
  knownErrors?: KnownErrorInfo[];
  name: string;
  /**
   * API-compatible field type info.
   * Describes the type of a field, including nested structures.
   */
  output: FieldTypeInfo;
  rateLimited: boolean;
  /**
   * Semantic tags for capability classification and filtering.
   * Well-known tags: "memory:read", "memory:write".
   */
  tags?: string[];
}

/** Request body for starting a chat session with an initial message */
export interface ChatRequest {
  /** Input data for the workflow (merged with message) */
  data?: any;
  /** User message to send to the AI agent */
  message: string;
  /** Variables for the workflow */
  variables?: any;
  /**
   * Workflow version to execute (defaults to current)
   * @format int32
   */
  version?: number | null;
}

/** Request body for starting a chat session without an initial message */
export interface ChatStartRequest {
  /** Input data for the workflow */
  data?: any;
  /** Variables for the workflow */
  variables?: any;
  /**
   * Workflow version to execute (defaults to current)
   * @format int32
   */
  version?: number | null;
}

export interface CheckpointMetadataDto {
  operation: string;
  /**
   * @format int64
   * @min 0
   */
  resultSize: number;
  resultType: string;
  /**
   * @format int64
   * @min 0
   */
  seq: number;
  stepId?: string | null;
}

/** Child workflow version specification */
export type ChildVersion = string | number;

export interface CloneWorkflowRequest {
  name: string;
}

/** Column definition for dynamic schema */
export type ColumnDefinition = ColumnType & {
  /** Default value (SQL expression, e.g., "0", "NOW()", "'active'") */
  default?: string | null;
  /** Column name (must be valid PostgreSQL identifier) */
  name: string;
  /** Whether the column allows NULL values (default: true) */
  nullable?: boolean;
  /**
   * Optional secondary text index. Only valid for `string` / `enum`
   * columns; rejected at validation otherwise.
   */
  textIndex?: TextIndexKind;
  /** Whether the column has a UNIQUE constraint (default: false) */
  unique?: boolean;
};

/** Column type definition with validation and SQL mapping */
export type ColumnType =
  | {
      type: "string";
    }
  | {
      type: "integer";
    }
  | {
      /**
       * Total number of digits (default: 19)
       * @format int32
       * @min 0
       */
      precision?: number;
      /**
       * Number of digits after decimal point (default: 4)
       * @format int32
       * @min 0
       */
      scale?: number;
      type: "decimal";
    }
  | {
      type: "boolean";
    }
  | {
      type: "timestamp";
    }
  | {
      type: "json";
    }
  | {
      type: "enum";
      /** List of allowed string values */
      values: string[];
    }
  | {
      /** Postgres text-search configuration. Defaults to `"english"`. */
      language?: string;
      /**
       * Name of the text column to derive the tsvector from. Must be a
       * `String` or `Enum` column declared in the same schema.
       */
      sourceColumn: string;
      type: "tsvector";
    }
  | {
      /**
       * Number of dimensions. Range: 1..=16000.
       * @format int32
       * @min 0
       */
      dimension: number;
      /** Optional approximate-nearest-neighbor index. None ⇒ no index. */
      indexMethod?: null | VectorIndexMethod;
      type: "vector";
    };

/** Controls how conversation memory is compacted when it grows too large. */
export interface CompactionConfig {
  /**
   * Maximum number of messages before compaction triggers.
   * Default: 50
   * @format int32
   * @min 0
   */
  maxMessages?: number | null;
  /**
   * Strategy for compacting old messages.
   * Default: SlidingWindow
   */
  strategy?: null | CompactionStrategy;
}

/**
 * Compensation configuration for saga pattern support.
 *
 * Defines what compensation step to execute if a downstream step fails,
 * enabling distributed transaction rollback.
 */
export interface CompensationConfig {
  /** Data to pass to compensation step (maps from current step's context) */
  compensationData?: null | HashMap;
  /** Step ID to execute for compensation (rollback) */
  compensationStep: string;
  /**
   * Compensation order (higher = compensate first, default = step execution order reversed)
   * @format int32
   */
  order?: number | null;
  /** When to trigger compensation: "on_downstream_error" (default), "on_any_error", "manual" */
  trigger?: string | null;
}

/** API-compatible compensation hint info */
export interface CompensationHintInfo {
  /** Capability ID that reverses this capability's effects */
  capabilityId: string;
  /** Human-readable description */
  description?: string | null;
}

export interface CompileWorkflowResponse {
  binaryChecksum: string;
  /** @min 0 */
  binarySize: number;
  message: string;
  success: boolean;
  timestamp: string;
  translatedPath: string;
  version: string;
  workflowId: string;
}

export interface Condition {
  arguments?: any[] | null;
  op: string;
}

/**
 * An argument to a condition operation.
 * Can be a nested expression or a mapping value (reference or immediate).
 *
 * Uses untagged serialization to avoid duplicate "type" fields when nesting
 * expressions (since both ConditionExpression and MappingValue use internally-tagged enums).
 * The deserializer distinguishes variants by structure:
 * - Expression: has "op" and "arguments" fields (from ConditionExpression::Operation)
 *   or has "valueType" field (from ConditionExpression::Value -> MappingValue)
 * - Value: has "valueType" field (from MappingValue)
 */
export type ConditionArgument = ConditionExpression | MappingValue;

/**
 * A condition expression for conditional branching.
 * Can be either an operation (with operator and arguments) or a simple value check.
 */
export type ConditionExpression =
  | (ConditionOperation & {
      type: "operation";
    })
  | (MappingValue & {
      type: "value";
    });

/** An operation in a condition expression */
export interface ConditionOperation {
  /**
   * The arguments to the operator (1+ depending on operator).
   * Each argument can be a nested expression or a value (reference/immediate).
   */
  arguments: ConditionArgument[];
  /** The operator (AND, OR, GT, EQ, STARTS_WITH, etc.) */
  op: ConditionOperator;
}

/** Evaluates conditions and branches execution */
export interface ConditionalStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** The condition expression to evaluate */
  condition: ConditionExpression;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/** DTO for returning auth type metadata to the frontend */
export interface ConnectionAuthTypeDto {
  /** Short description of this authentication type */
  description: string;
  /** Human-readable display name */
  displayName: string;
  /** Auth type identifier (snake_case) */
  id: string;
}

/** DTO for returning category metadata to the frontend */
export interface ConnectionCategoryDto {
  /** Short description of what this category covers */
  description: string;
  /** Human-readable display name */
  displayName: string;
  /** Category identifier (snake_case) */
  id: string;
}

/**
 * Connection DTO - Used for GET/LIST responses
 * SECURITY: Does NOT include connection_parameters field
 */
export interface ConnectionDto {
  connectionSubtype?: string | null;
  createdAt: string;
  id: string;
  /** Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp) */
  integrationId?: string | null;
  /** When true, this connection is the default S3 storage for webhook attachments */
  isDefaultFileStorage: boolean;
  rateLimitConfig?: null | RateLimitConfigDto;
  /** Rate limit statistics for the requested time period (only included when requested) */
  rateLimitStats?: null | PeriodStatsDto;
  status: ConnectionStatus;
  tenantId: string;
  title: string;
  updatedAt: string;
  validUntil?: string | null;
}

/** A field in a connection type's parameter schema */
export interface ConnectionFieldDto {
  /** Default value */
  defaultValue?: string | null;
  /** Description of the field */
  description?: string | null;
  /** Display name for UI */
  displayName?: string | null;
  /** Whether this field is optional */
  isOptional: boolean;
  /** Whether this is a secret field (password, API key, etc.) */
  isSecret: boolean;
  /** Field name (used in JSON) */
  name: string;
  /** Placeholder text for the input */
  placeholder?: string | null;
  /** Type name (String, u16, bool, etc.) */
  typeName: string;
}

/** Response for single connection operations */
export interface ConnectionResponse {
  /**
   * Connection DTO - Used for GET/LIST responses
   * SECURITY: Does NOT include connection_parameters field
   */
  connection: ConnectionDto;
  success: boolean;
}

/** A connection type with its parameter schema */
export interface ConnectionTypeDto {
  /** Category for grouping (e.g., "ecommerce", "file_storage", "llm") */
  category?: string | null;
  /** Default rate limit configuration for this connection type (if applicable) */
  defaultRateLimitConfig?: null | RateLimitConfigDto;
  /** Description of this connection type */
  description?: string | null;
  /** Display name for UI */
  displayName: string;
  /** Fields required for this connection type */
  fields: ConnectionFieldDto[];
  /** Unique identifier for this connection type */
  integrationId: string;
  /** OAuth2 configuration (only for auth_type = oauth2_authorization_code) */
  oauthConfig?: null | OAuthConfigDto;
}

/** Response for getting a single connection type */
export interface ConnectionTypeResponse {
  /** A connection type with its parameter schema */
  connectionType: ConnectionTypeDto;
  success: boolean;
}

/** CPU information for the runtime system */
export interface CpuInfo {
  /** CPU architecture (e.g., "x86_64", "aarch64") */
  architecture: string;
  /**
   * Number of logical CPU cores (including hyperthreading)
   * @min 0
   */
  logicalCores: number;
  /**
   * Number of physical CPU cores
   * @min 0
   */
  physicalCores: number;
}

export interface CreateApiKeyRequest {
  /** Optional expiration time */
  expires_at?: string | null;
  /** Human-readable name for the key */
  name: string;
}

export type CreateApiKeyResponse = ApiKey & {
  /** The plaintext API key — shown only once, store it securely */
  key: string;
};

export interface CreateBucketRequest {
  /** Bucket name to create */
  name: string;
}

export interface CreateBucketResponse {
  success: boolean;
}

/** Create connection request */
export interface CreateConnectionRequest {
  connectionParameters?: any;
  connectionSubtype?: string | null;
  /** Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp) */
  integrationId?: string | null;
  isDefaultFileStorage?: boolean | null;
  rateLimitConfig?: null | RateLimitConfigDto;
  status?: null | ConnectionStatus;
  title: string;
  validUntil?: string | null;
}

/** Response for create operation */
export interface CreateConnectionResponse {
  connectionId: string;
  message: string;
  success: boolean;
}

export interface CreateInstanceRequest {
  properties: any;
  /** Schema ID (UUID) - use this OR schemaName */
  schemaId?: string | null;
  /** Schema name - use this OR schemaId (more convenient) */
  schemaName?: string | null;
}

export interface CreateInstanceResponse {
  instanceId: string;
  message: string;
  success: boolean;
}

/** Request payload for creating a new invocation trigger */
export interface CreateInvocationTriggerRequest {
  /**
   * Whether the trigger should be active upon creation
   * @example true
   */
  active?: boolean;
  /** Trigger-specific configuration in JSON format */
  configuration?: object | null;
  /**
   * Remote tenant identifier for external system triggers
   * @example "remote-tenant-789"
   */
  remote_tenant_id?: string | null;
  /**
   * Whether only a single instance of this trigger should run at a time
   * @example false
   */
  single_instance?: boolean;
  /** Type of trigger */
  trigger_type: TriggerType;
  /**
   * Reference to the workflow to be invoked
   * @example "workflow-456"
   */
  workflow_id: string;
}

export interface CreateSchemaRequest {
  columns: ColumnDefinition[];
  description?: string | null;
  indexes?: IndexDefinition[] | null;
  name: string;
  tableName: string;
}

export interface CreateSchemaResponse {
  message: string;
  schemaId: string;
  success: boolean;
}

export interface CreateWorkflowRequest {
  description: string;
  memoryTier?: null | MemoryTier;
  name: string;
  /** Enable step-event tracking for this workflow version (default: true) */
  trackEvents?: boolean | null;
}

/** Request body for CSV export */
export interface CsvExportRequest {
  /** Columns to include in export (default: all schema columns) */
  columns?: string[] | null;
  /** Filter condition (reuses existing Condition type) */
  condition?: null | Condition;
  /** Include system columns (id, created_at, updated_at). Default: true */
  includeSystemColumns?: boolean;
  /** Sort fields */
  sortBy?: string[] | null;
  /** Sort order for each field ("asc" or "desc") */
  sortOrder?: string[] | null;
}

/** JSON request body for CSV import (base64-encoded CSV with mapping) */
export interface CsvImportJsonRequest {
  /** Column mapping: CSV header → schema column name */
  columnMapping: Partial<Record<string, string>>;
  /** Conflict columns for upsert mode */
  conflictColumns?: string[] | null;
  /** Base64-encoded CSV data */
  data: string;
  /** Import mode: "create" or "upsert" */
  mode?: string;
  /** Error handling: "abort" (reject all on any error) or "skip" (import valid rows, skip invalid) */
  onError?: string;
}

/** Successful import response */
export interface CsvImportResponse {
  /** @format int64 */
  affectedRows: number;
  message: string;
  mode: string;
  /**
   * Number of rows skipped due to validation errors (only present in "skip" mode)
   * @format int64
   */
  skippedRows?: number | null;
  success: boolean;
  /** Validation errors for skipped rows (only present in "skip" mode) */
  validationErrors?: CsvValidationError[] | null;
}

/** Validation failure response (HTTP 400) */
export interface CsvImportValidationErrorResponse {
  error: string;
  success: boolean;
  validationErrors: CsvValidationError[];
}

/** JSON request body for import preview (base64-encoded CSV) */
export interface CsvPreviewJsonRequest {
  /** Base64-encoded CSV data */
  data: string;
}

/** Per-row validation error */
export interface CsvValidationError {
  column: string;
  error: string;
  /**
   * 1-indexed row number (excluding header)
   * @min 0
   */
  row: number;
}

/**
 * Delay step - pause workflow execution for a specified duration.
 *
 * This is a **durable** delay: if the workflow crashes during the delay,
 * it will resume from where it left off rather than restarting the delay.
 *
 * For native platforms, this uses `sdk.durable_sleep()` which stores
 * the wake time in the database. For WASI/embedded, it uses blocking sleep.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "Delay",
 *   "id": "wait-for-cooldown",
 *   "name": "Wait 5 seconds",
 *   "duration_ms": { "valueType": "immediate", "value": 5000 }
 * }
 * ```
 *
 * Duration can also be dynamic:
 * ```json
 * {
 *   "stepType": "Delay",
 *   "id": "dynamic-wait",
 *   "duration_ms": { "valueType": "reference", "value": "data.waitTimeMs" }
 * }
 * ```
 */
export interface DelayStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /**
   * Disable durability for this step when `Some(false)`. Uses
   * `std::thread::sleep` instead of `sdk.durable_sleep` — the delay is
   * not suspendable or resumable across crashes.
   */
  durable?: boolean | null;
  /**
   * Duration to delay in milliseconds.
   * Can be an immediate value or a reference to data/variables.
   */
  durationMs: MappingValue;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/** Response for delete operation */
export interface DeleteConnectionResponse {
  message: string;
  success: boolean;
}

export interface DeleteResponse {
  success: boolean;
}

/** Disk space information for the data directory */
export interface DiskInfo {
  /**
   * Available disk space in bytes
   * @format int64
   * @min 0
   */
  availableBytes: number;
  /** Path to the data directory being measured */
  path: string;
  /**
   * Total disk space in bytes
   * @format int64
   * @min 0
   */
  totalBytes: number;
}

/** Executes a nested child workflow */
export interface EmbedWorkflowStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Version of child workflow ("latest" or specific version number) */
  childVersion: ChildVersion;
  /** ID of the child workflow to execute */
  childWorkflowId: string;
  /**
   * Disable durability for this step when `Some(false)`. Skips checkpoint
   * on the child workflow's final result at this call site. The child
   * workflow's internal steps still run according to the enclosing
   * workflow setting (step-level flag does not leak into the child).
   */
  durable?: boolean | null;
  /** Unique step identifier */
  id: string;
  /** Maps parent data to child workflow inputs */
  inputMapping?: null | HashMap;
  /**
   * Maximum retry attempts (default: 3)
   * @format int32
   * @min 0
   */
  maxRetries?: number | null;
  /** Human-readable step name */
  name?: string | null;
  /**
   * Base delay between retries in milliseconds (default: 1000)
   * @format int64
   * @min 0
   */
  retryDelay?: number | null;
  /**
   * Step timeout in milliseconds. If exceeded, step fails.
   * @format int64
   * @min 0
   */
  timeout?: number | null;
}

/**
 * Standard error payload returned by connection endpoints.
 *
 * Runtime handlers return arbitrary `serde_json::Value` errors; this struct
 * exists to give OpenAPI consumers a documented shape.
 */
export interface ErrorResponse {
  error: string;
  message?: string | null;
  success: boolean;
}

/**
 * Emit a structured error and terminate the workflow.
 *
 * The Error step allows workflows to explicitly emit categorized errors
 * with structured metadata. This is the primary mechanism for business
 * logic errors that should be distinguishable from technical errors.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "Error",
 *   "id": "credit_limit_error",
 *   "category": "business",
 *   "code": "CREDIT_LIMIT_EXCEEDED",
 *   "message": "Order total ${data.total} exceeds credit limit ${data.limit}"
 * }
 * ```
 */
export interface ErrorStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /**
   * Error category determines retry behavior:
   * - "transient": Retry is likely to succeed (network, timeout, rate limit)
   * - "permanent": Don't retry (validation, not found, authorization, business rules)
   *
   * Use `code` and `severity` to distinguish technical vs business errors.
   */
  category?: ErrorCategory;
  /** Machine-readable error code (e.g., "CREDIT_LIMIT_EXCEEDED", "INVALID_ACCOUNT") */
  code: string;
  /**
   * Additional context data to include with the error.
   * Keys are field names, values specify how to obtain the data.
   */
  context?: null | HashMap;
  /** Unique step identifier */
  id: string;
  /**
   * Human-readable error message (static string).
   * For dynamic data, use the `context` field with mappings.
   */
  message: string;
  /** Human-readable step name */
  name?: string | null;
  /**
   * Error severity for logging/alerting:
   * - "info": Informational (expected errors)
   * - "warning": Warning (degraded but functional)
   * - "error": Error (operation failed) - default
   * - "critical": Critical (system-level failure)
   */
  severity?: null | ErrorSeverity;
}

/** Error response from agent execution */
export interface ExecuteAgentErrorResponse {
  error: string;
  message?: string | null;
  success: boolean;
}

/**
 * Request body for executing an agent capability on the host
 * @example {"connectionId":"conn_shopify_main","inputs":{"method":"GET","url":"https://api.example.com/data"},"instanceId":"inst-abc-123","tenantId":"org_example_tenant"}
 */
export interface ExecuteAgentRequest {
  /**
   * Optional connection ID for agents that require credentials.
   * The host resolves the connection and injects it as `_connection` in agent input.
   */
  connectionId?: string | null;
  /** Agent-specific input data (structure depends on the agent/capability). */
  inputs: any;
  /** Instance ID of the calling workflow (for tracing/logging). */
  instanceId?: string | null;
  /**
   * Tenant ID of the calling workflow.
   * Used as fallback if not available from auth context.
   */
  tenantId?: string | null;
}

/** Successful response from agent execution */
export interface ExecuteAgentResponse {
  /** Error message (present on failure) */
  error?: string | null;
  /**
   * Execution time in milliseconds
   * @format double
   */
  executionTimeMs: number;
  /** Agent output (present on success) */
  output?: any;
  /** Whether the agent executed successfully */
  success: boolean;
}

export interface ExecuteWorkflowRequest {
  /**
   * When true, enables debug mode: execution pauses at steps with breakpoints.
   * Use the resume endpoint to continue execution to the next breakpoint.
   */
  debug?: boolean | null;
  inputs: any;
}

export interface ExecuteWorkflowResponse {
  instanceId: string;
  status: string;
}

/** The execution graph containing all steps and control flow */
export interface ExecutionGraph {
  /** Detailed description of what the workflow does */
  description?: string | null;
  /**
   * Disable durability for this workflow when `Some(false)`. Mirrors
   * `Workflow.durable`; `parse_workflow` copies the top-level flag here when
   * this field is `None`. Codegen reads `ctx.durable` from this value at
   * the root, then inherits it unconditionally into all nested subgraphs
   * and embedded children. `None` → durable (default).
   */
  durable?: boolean | null;
  /**
   * UI edge positions for the visual workflow editor.
   * This is opaque data managed by the UI - the runtime does not interpret this field.
   * Typically contains an array of edge objects connecting nodes.
   * Not used in compilation or execution.
   */
  edges?: any;
  /** ID of the entry point step (step with no incoming edges) */
  entryPoint: string;
  /** Ordered list of step transitions defining control flow */
  executionPlan?: ExecutionPlanEdge[];
  /**
   * Schema defining expected input data structure for this workflow.
   * Keys are field names, values define the field type and constraints.
   */
  inputSchema?: Partial<Record<string, SchemaField>>;
  /** Human-readable name for the workflow */
  name?: string | null;
  /**
   * UI node positions for the visual workflow editor.
   * This is opaque data managed by the UI - the runtime does not interpret this field.
   * Typically contains an array of node objects with position coordinates.
   * Not used in compilation or execution.
   */
  nodes?: any;
  /** Visual annotations for UI (not used in compilation) */
  notes?: Note[] | null;
  /**
   * Schema defining output data structure for this workflow.
   * Keys are field names, values define the field type and constraints.
   */
  outputSchema?: Partial<Record<string, SchemaField>>;
  /**
   * Maximum cumulative time (in milliseconds) that rate-limited retries may
   * durable-sleep before giving up.  Applies to all steps in this workflow.
   * Default: 60 000 (1 minute).  Set higher for workflows that make many
   * calls through a slow rate limit (e.g. 3 600 000 for 1 hour).
   * @format int64
   * @min 0
   */
  rateLimitBudgetMs?: number;
  /** Map of step IDs to step definitions */
  steps: Partial<Record<string, Step>>;
  /**
   * Constant variables available as `variables.<name>` during execution.
   * These are static values defined at design time, not overridable at runtime.
   * Keys are variable names, values contain type and value.
   */
  variables?: Partial<Record<string, Variable>>;
}

/**
 * An edge in the execution plan defining control flow between steps.
 *
 * # Edge Selection Semantics
 *
 * When multiple edges originate from the same step with the same label:
 *
 * 1. **Conditional edges** (with `condition`): Evaluated in priority order (highest first).
 *    The first condition that evaluates to true wins.
 *
 * 2. **Default edge** (without `condition`): At most one is allowed per (from_step, label) pair.
 *    Only taken if no conditional edge matches.
 *
 * 3. **Parallel edges** (without conditions OR labels): Multiple unlabeled, condition-less
 *    edges can exist - they execute in parallel (e.g., fan-out patterns).
 *
 * 4. **Conditional step exception**: `true`/`false` labeled edges from a Conditional step
 *    are mutually exclusive based on the condition result, not evaluated via edge conditions.
 *
 * # Validation Rules
 *
 * - Multiple conditional edges from the same step with the same label must have unique priorities
 * - At most one default (condition-less) edge per (from_step, label) pair
 * - If no condition matches and no default exists, the workflow fails (for onError) or continues normally
 */
export interface ExecutionPlanEdge {
  /**
   * Optional condition expression for conditional transitions.
   *
   * Uses the same format as `Conditional` step conditions, supporting
   * operators like EQ, AND, OR, STARTS_WITH, CONTAINS, etc.
   *
   * Available context for conditions:
   * - `data.*` - Input data
   * - `steps.<stepId>.outputs.*` - Previous step outputs
   * - `variables.*` - Workflow variables
   * - `__error.*` - Error details (for `onError` edges): code, message, category, severity, attributes
   *
   * Example for onError routing:
   * ```json
   * {
   *   "condition": {
   *     "type": "operation",
   *     "op": "EQ",
   *     "arguments": [
   *       { "valueType": "reference", "value": "__error.category" },
   *       { "valueType": "immediate", "value": "transient" }
   *     ]
   *   }
   * }
   * ```
   */
  condition?: null | ConditionExpression;
  /** Source step ID */
  fromStep: string;
  /**
   * Edge label for control flow:
   * - `"true"`/`"false"` for Conditional step branches
   * - `"onError"` for error handling transition (step failed after retries)
   * - `None` or empty for normal sequential flow
   */
  label?: string | null;
  /**
   * Priority for conditional edge selection (higher = checked first, default = 0).
   *
   * When multiple edges with conditions exist for the same (from_step, label) pair:
   * - Edges are evaluated in descending priority order
   * - The first condition that evaluates to true wins
   * - Priorities must be unique among conditional edges from the same step/label
   * - Edges without conditions (default fallback) are always checked last
   * @format int32
   */
  priority?: number | null;
  /** Target step ID */
  toStep: string;
}

/**
 * API-compatible field type info.
 * Describes the type of a field, including nested structures.
 */
export interface FieldTypeInfo {
  description?: string | null;
  displayName?: string | null;
  format?: string | null;
  /** Whether this field can be null */
  nullable?: boolean;
  type: string;
}

/**
 * Base64-encoded file data structure.
 * Used for file inputs/outputs in workflows and operators.
 */
export interface FileData {
  /** Base64-encoded file content */
  content: string;
  /** Original filename (optional) */
  filename?: string | null;
  /** MIME type, e.g., "text/csv", "application/pdf" (optional) */
  mimeType?: string | null;
}

export interface FileMetadataResponse {
  /**
   * File size in bytes
   * @format int64
   * @min 0
   */
  contentLength: number;
  /** MIME content type */
  contentType: string;
  /** ETag (content hash) */
  etag: string;
  /** Last modified timestamp */
  lastModified: string;
}

export interface FileObjectDto {
  /** ETag (content hash) */
  etag: string;
  /** Object key (file path) */
  key: string;
  /** Last modified timestamp */
  lastModified: string;
  /**
   * File size in bytes
   * @format int64
   * @min 0
   */
  size: number;
}

/** Configuration for a Filter step */
export interface FilterConfig {
  /**
   * Condition expression evaluated for each item.
   * Within the condition, `item.*` references resolve to the current element.
   */
  condition: ConditionExpression;
  /**
   * Array to filter (MappingValue resolving to array).
   * If null or non-array, treated as empty array.
   */
  value: MappingValue;
}

export interface FilterInstancesResponse {
  instances: Instance[];
  /** @format int64 */
  limit: number;
  /** @format int64 */
  offset: number;
  success: boolean;
  /** @format int64 */
  totalCount: number;
}

export interface FilterRequest {
  condition?: null | Condition;
  /** @format int64 */
  limit?: number;
  /** @format int64 */
  offset?: number;
  /** Optional ORDER BY entries. When set, supersedes `sortBy` / `sortOrder`. */
  orderBy?: OrderByEntry[] | null;
  /**
   * Optional computed score column. Adds `<expression> AS <alias>` to
   * the SELECT and surfaces under `instance.computed[alias]`.
   */
  scoreExpression?: null | ScoreExpression;
  /** Fields to sort by (e.g., ["createdAt", "name"]). Supports system fields (id, createdAt, updatedAt) and schema-defined columns. */
  sortBy?: string[] | null;
  /** Sort order for each field (e.g., ["desc", "asc"]). Defaults to "asc" for unspecified fields. */
  sortOrder?: string[] | null;
}

/**
 * Filter step - filters an array using a condition expression
 *
 * The condition is evaluated for each item in the array, with `item.*`
 * references resolving to the current element being evaluated.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "Filter",
 *   "id": "filter-active",
 *   "config": {
 *     "value": { "valueType": "reference", "path": "steps.get-users.outputs.items" },
 *     "condition": {
 *       "type": "operation",
 *       "op": "eq",
 *       "arguments": [
 *         { "valueType": "reference", "path": "item.status" },
 *         { "valueType": "immediate", "value": "active" }
 *       ]
 *     }
 *   }
 * }
 * ```
 */
export interface FilterStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Filter configuration: array to filter and condition */
  config: FilterConfig;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/**
 * Exit point step - defines workflow outputs.
 *
 * The Finish step's `inputMapping` IS the workflow's (or, when nested in a
 * Split subgraph, the iteration's) output: each map key becomes a field on
 * the resulting object, and the values reference workflow data via the
 * standard mapping system (`data.*`, `steps.<id>.outputs.*`, `variables.*`).
 *
 * There is no `outputMapping` field — Finish only takes `inputMapping`.
 */
export interface FinishStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Unique step identifier */
  id: string;
  /**
   * Defines the workflow's final output (or a Split iteration's per-item
   * result when nested in a Split subgraph). Each map key becomes a field
   * on the resulting object; values reference workflow data via the
   * standard mapping system (`data.*`, `steps.<id>.outputs.*`,
   * `variables.*`). Finish has no `outputMapping` — `inputMapping` *is*
   * the output.
   */
  inputMapping?: null | HashMap;
  /** Human-readable step name */
  name?: string | null;
}

/** Response for listing folders */
export interface FoldersResponse {
  /** List of distinct folder paths */
  folders: string[];
}

export interface GetInstanceResponse {
  instance: Instance;
  success: boolean;
}

/** Response for single connection rate limit status */
export interface GetRateLimitStatusResponse {
  /** Complete rate limit status for a connection */
  data: RateLimitStatusDto;
  success: boolean;
}

export interface GetSchemaResponse {
  schema: Schema;
  success: boolean;
}

/** Response for get step events endpoint */
export interface GetStepEventsResponse {
  /** Step events data container */
  data: StepEventsData;
  message: string;
  success: boolean;
}

/** Configuration for a GroupBy step */
export interface GroupByConfig {
  /**
   * Optional list of expected key values.
   * These keys are pre-initialized with count=0 and groups=[]
   * before grouping, ensuring they always exist in output.
   */
  expectedKeys?: string[] | null;
  /**
   * Property path to group by (e.g., "status", "user.role", "data.category").
   * Supports nested paths with dot notation.
   */
  key: string;
  /**
   * Array to group (MappingValue resolving to array).
   * If null or non-array, treated as empty array.
   */
  value: MappingValue;
}

/**
 * GroupBy step - groups array items by a key property
 *
 * Groups items in an array based on the value at a specified property path.
 * Returns grouped items as a map, counts per group, and total number of groups.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "GroupBy",
 *   "id": "group-by-status",
 *   "config": {
 *     "value": { "valueType": "reference", "value": "steps.get-orders.outputs.items" },
 *     "key": "status"
 *   }
 * }
 * ```
 */
export interface GroupByStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** GroupBy configuration: array to group and key path */
  config: GroupByConfig;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

export type HashMap = Partial<
  Record<
    string,
    | (ReferenceValue & {
        valueType: "reference";
      })
    | (ImmediateValue & {
        valueType: "immediate";
      })
    | {
        valueType: "composite";
      }
    | (TemplateValue & {
        valueType: "template";
      })
  >
>;

/**
 * An immediate (literal) value.
 *
 * For non-string types (number, boolean, object, array), the type is unambiguous.
 * For strings, this is always treated as a literal string, never as a reference.
 *
 * Example: `{ "valueType": "immediate", "value": "Hello World" }`
 */
export interface ImmediateValue {
  /** The literal value (string, number, boolean, object, or array) */
  value: any;
}

/** Response from the import preview endpoint */
export interface ImportPreviewResponse {
  csvHeaders: string[];
  sampleRows: string[][];
  schemaColumns: SchemaColumnInfo[];
  success: boolean;
  suggestedMappings: Partial<Record<string, string | null>>;
  /** @min 0 */
  totalRows: number;
  /**
   * Columns that can be used as conflict columns for upsert mode
   * (columns with UNIQUE constraints or belonging to unique indexes)
   */
  uniqueColumns: string[];
}

/** Index definition for dynamic schema */
export interface IndexDefinition {
  /** Columns included in the index */
  columns: string[];
  /** Index name */
  name: string;
  /** Whether this is a UNIQUE index (default: false) */
  unique?: boolean;
}

export interface Instance {
  /**
   * Computed columns (e.g. `score_expression` output). Absent when
   * no score expression was requested.
   */
  computed?: Partial<Record<string, any>> | null;
  createdAt: string;
  id: string;
  properties: any;
  schemaId?: string | null;
  schemaName?: string | null;
  tenantId: string;
  updatedAt: string;
}

export interface InstanceInputs {
  data?: any;
  variables?: any;
}

/** Invocation trigger model */
export interface InvocationTrigger {
  /**
   * Whether the trigger is currently active
   * @example true
   */
  active: boolean;
  /** Trigger-specific configuration in JSON format */
  configuration?: object | null;
  /**
   * Timestamp when the trigger was created
   * @example "2025-01-15T10:30:00Z"
   */
  created_at: string;
  /**
   * Unique identifier for the invocation trigger (auto-generated)
   * @example "550e8400-e29b-41d4-a716-446655440000"
   */
  id: string;
  /**
   * Timestamp of the last trigger execution (system-managed)
   * @example "2025-01-15T12:00:00Z"
   */
  last_run?: string | null;
  /**
   * Remote tenant identifier for external system triggers
   * @example "remote-tenant-789"
   */
  remote_tenant_id?: string | null;
  /**
   * Whether only a single instance of this trigger should run at a time
   * @example false
   */
  single_instance: boolean;
  /**
   * Tenant identifier for multi-tenancy support
   * @example "tenant-123"
   */
  tenant_id?: string | null;
  /** Type of trigger */
  trigger_type: TriggerType;
  /**
   * Timestamp when the trigger was last updated
   * @example "2025-01-15T10:30:00Z"
   */
  updated_at: string;
  /**
   * Reference to the workflow to be invoked
   * @example "workflow-456"
   */
  workflow_id: string;
}

/** API-compatible known error info */
export interface KnownErrorInfo {
  /** Context attributes included with this error */
  attributes?: string[];
  /** Machine-readable error code (e.g., "HTTP_TIMEOUT") */
  code: string;
  /** Human-readable description of when this error occurs */
  description: string;
  /** Error kind: "transient" or "permanent" */
  kind: string;
}

/** Response for listing all agents */
export interface ListAgentsResponse {
  agents: AgentSummary[];
}

/** Response for listing all executions */
export interface ListAllExecutionsResponse {
  data: PageWorkflowInstanceHistoryDto;
  success: boolean;
}

export interface ListBucketsResponse {
  buckets: BucketDto[];
}

export interface ListCheckpointsQuery {
  /** @format int32 */
  page?: number | null;
  /** @format int32 */
  size?: number | null;
}

export interface ListCheckpointsResponse {
  checkpoints: CheckpointMetadataDto[];
  instanceId: string;
  /** @format int32 */
  page: number;
  /** @format int32 */
  size: number;
  success: boolean;
  /** @min 0 */
  totalCount: number;
  /** @format int32 */
  totalPages: number;
}

/** Response for listing all connection auth types */
export interface ListConnectionAuthTypesResponse {
  authTypes: ConnectionAuthTypeDto[];
  /** @min 0 */
  count: number;
  success: boolean;
}

/** Response for listing all connection categories */
export interface ListConnectionCategoriesResponse {
  categories: ConnectionCategoryDto[];
  /** @min 0 */
  count: number;
  success: boolean;
}

/** Response for listing all connection types */
export interface ListConnectionTypesResponse {
  connectionTypes: ConnectionTypeDto[];
  /** @min 0 */
  count: number;
  success: boolean;
}

/** Response for listing connections */
export interface ListConnectionsResponse {
  connections: ConnectionDto[];
  /** @min 0 */
  count: number;
  success: boolean;
}

export interface ListInstancesResponse {
  instances: Instance[];
  /** @format int64 */
  limit: number;
  /** @format int64 */
  offset: number;
  success: boolean;
  /** @format int64 */
  totalCount: number;
}

export interface ListObjectsResponse {
  /**
   * @format int32
   * @min 0
   */
  count: number;
  files: FileObjectDto[];
  /** Token for fetching next page (null if no more results) */
  nextContinuationToken?: string | null;
}

/** Response for listing all connections' rate limit status */
export interface ListRateLimitsResponse {
  /** @min 0 */
  count: number;
  data: RateLimitStatusDto[];
  success: boolean;
}

export interface ListSchemasResponse {
  /** @format int64 */
  limit: number;
  /** @format int64 */
  offset: number;
  schemas: Schema[];
  success: boolean;
  /** @format int64 */
  totalCount: number;
}

/** Response for listing all step types */
export interface ListStepTypesResponse {
  step_types: StepTypeInfo[];
}

/** Emit custom log/debug events during workflow execution */
export interface LogStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /**
   * Additional context data to include in the log event.
   * Keys are field names, values specify how to obtain the data.
   */
  context?: null | HashMap;
  /** Unique step identifier */
  id: string;
  /** Log level */
  level?: LogLevel;
  /** Log message */
  message: string;
  /** Human-readable step name */
  name?: string | null;
}

/**
 * A mapping value specifies how to obtain data for a field.
 *
 * Uses explicit `valueType` discriminator:
 * - `reference`: Value is a path to data (e.g., "data.name", "steps.step1.outputs.result")
 * - `immediate`: Value is a literal (string, number, boolean, object, array)
 * - `composite`: Value is a structured object or array with nested MappingValues
 *
 * Example reference: `{ "valueType": "reference", "value": "data.user.name" }`
 * Example immediate: `{ "valueType": "immediate", "value": "Hello World" }`
 * Example composite: `{ "valueType": "composite", "value": { "name": {...}, "id": {...} } }`
 */
export type MappingValue =
  | (ReferenceValue & {
      valueType: "reference";
    })
  | (ImmediateValue & {
      valueType: "immediate";
    })
  | {
      valueType: "composite";
    }
  | (TemplateValue & {
      valueType: "template";
    });

/** Memory information for the runtime system */
export interface MemoryInfo {
  /**
   * Currently available memory in bytes
   * @format int64
   * @min 0
   */
  availableBytes: number;
  /**
   * Memory available for workflows (80% of available, 20% reserved for runtime)
   * @format int64
   * @min 0
   */
  availableForWorkflowsBytes: number;
  /**
   * Total system memory in bytes
   * @format int64
   * @min 0
   */
  totalBytes: number;
}

export interface MetricsQuery {
  /** @format date-time */
  endTime?: string | null;
  granularity?: string | null;
  /** @format date-time */
  startTime?: string | null;
  /** @format int32 */
  version?: number | null;
}

export interface MetricsResponse {
  data: any;
  message: string;
  success: boolean;
}

/** Request to move a workflow to a different folder */
export interface MoveWorkflowRequest {
  /**
   * Target folder path (e.g., "/Sales/Shopify/")
   * Must start and end with "/"
   */
  path: string;
}

/** Response for move workflow operation */
export interface MoveWorkflowResponse {
  path: string;
  success: boolean;
  workflowId: string;
}

export interface NotImplementedResponse {
  endpoint: string;
  message: string;
  /**
   * @format int32
   * @min 0
   */
  status: number;
  success: boolean;
}

/** Visual note/annotation for workflow canvas */
export interface Note {
  /** Note text content */
  content: string;
  /** Unique identifier (UUID, client-generated or server-generated if missing) */
  id: string;
  /** Additional flexible metadata (color, fontSize, etc.) */
  metadata?: any;
  /** Optional user ID who created the note */
  userId?: string | null;
  /**
   * X coordinate for visual positioning
   * @format double
   */
  x: number;
  /**
   * Y coordinate for visual positioning
   * @format double
   */
  y: number;
}

export interface OAuthAuthorizeResponse {
  authorizationUrl: string;
  success: boolean;
}

/** OAuth2 configuration for a connection type (authorization code flow) */
export interface OAuthConfigDto {
  /** Provider's authorization endpoint */
  authUrl: string;
  /** Space-separated default scopes */
  defaultScopes: string;
  /** Provider's token endpoint */
  tokenUrl: string;
}

export interface OrderByEntry {
  /** Sort direction. JSON encoding is UPPERCASE (`"ASC"` / `"DESC"`). */
  direction?: SortDirection;
  expression: OrderByTarget;
}

export type OrderByTarget =
  | {
      kind: "column";
      name: string;
    }
  | {
      kind: "alias";
      name: string;
    };

/**
 * API-compatible output field info.
 * Describes an output field with type information.
 */
export interface OutputField {
  description?: string | null;
  displayName?: string | null;
  example?: any;
  format?: string | null;
  name: string;
  /** Whether this field can be null */
  nullable?: boolean;
  type: string;
}

/** Paginated response for workflow listings (matches Spring Boot Page format) */
export interface PageWorkflowDto {
  content: WorkflowDto[];
  first: boolean;
  last: boolean;
  /**
   * Current page number (0-based)
   * @format int32
   */
  number: number;
  /** @format int32 */
  numberOfElements: number;
  /** @format int32 */
  size: number;
  /** @format int64 */
  totalElements: number;
  /** @format int32 */
  totalPages: number;
}

export interface PageWorkflowInstanceHistoryDto {
  content: WorkflowInstanceDto[];
  first: boolean;
  last: boolean;
  /** @format int32 */
  number: number;
  /** @format int32 */
  numberOfElements: number;
  /** @format int32 */
  size: number;
  /** @format int64 */
  totalElements: number;
  /** @format int32 */
  totalPages: number;
}

/** Aggregated rate limit stats for a time period */
export interface PeriodStatsDto {
  /** The interval used for aggregation */
  interval: string;
  /**
   * Number of rate-limited events
   * @format int64
   */
  rateLimitedCount: number;
  /**
   * Percentage of requests that were rate-limited
   * @format double
   */
  rateLimitedPercent: number;
  /**
   * Number of retry events
   * @format int64
   */
  retryCount: number;
  /**
   * Total requests in the period
   * @format int64
   */
  totalRequests: number;
}

/** Position coordinates for UI elements */
export interface Position {
  /** @format double */
  x: number;
  /** @format double */
  y: number;
}

/** Rate limit configuration stored in PostgreSQL */
export interface RateLimitConfigDto {
  /**
   * Maximum token capacity (burst size)
   * @format int32
   * @min 0
   */
  burstSize: number;
  /**
   * Maximum retry attempts
   * @format int32
   * @min 0
   */
  maxRetries: number;
  /**
   * Maximum cumulative wait time in milliseconds
   * @format int64
   * @min 0
   */
  maxWaitMs: number;
  /**
   * Requests allowed per second (refill rate)
   * @format int32
   * @min 0
   */
  requestsPerSecond: number;
  /** Whether to automatically retry when rate limited */
  retryOnLimit: boolean;
}

/** A single rate limit event in the timeline */
export interface RateLimitEventDto {
  /** Connection ID */
  connectionId: string;
  /**
   * When the event occurred
   * @format date-time
   */
  createdAt: string;
  /** Type of event */
  eventType: string;
  /**
   * Event ID
   * @format int64
   */
  id: number;
  /** Additional event metadata */
  metadata?: any;
}

/** Response for rate limit history endpoint */
export interface RateLimitHistoryResponse {
  data: RateLimitEventDto[];
  /** @format int64 */
  limit: number;
  /** @format int64 */
  offset: number;
  success: boolean;
  /** @format int64 */
  totalCount: number;
}

/** Computed rate limit metrics */
export interface RateLimitMetricsDto {
  /**
   * Current capacity as percentage (tokens / burst_size * 100)
   * @format double
   */
  capacityPercent?: number | null;
  /** Whether the connection is currently rate limited (tokens < 1) */
  isRateLimited: boolean;
  /**
   * Milliseconds until next token is available (if rate limited)
   * @format int64
   * @min 0
   */
  retryAfterMs?: number | null;
  /**
   * Current utilization as percentage (100 - capacity_percent)
   * @format double
   */
  utilizationPercent?: number | null;
}

/** Real-time rate limit state from Redis */
export interface RateLimitStateDto {
  /** Whether Redis state is available */
  available: boolean;
  /**
   * Number of calls made in the current window (since last refill)
   * @format int32
   * @min 0
   */
  callsInWindow?: number | null;
  /**
   * Current token count in the bucket
   * @format double
   */
  currentTokens?: number | null;
  /**
   * Last refill timestamp in milliseconds
   * @format int64
   */
  lastRefillMs?: number | null;
  /**
   * Learned rate limit from API response headers
   * @format int32
   * @min 0
   */
  learnedLimit?: number | null;
  /**
   * Total lifetime calls made through this connection
   * @format int64
   * @min 0
   */
  totalCalls?: number | null;
  /**
   * Timestamp when the current window started (milliseconds)
   * @format int64
   */
  windowStartMs?: number | null;
}

/** Complete rate limit status for a connection */
export interface RateLimitStatusDto {
  /** Rate limit configuration (from PostgreSQL) */
  config?: null | RateLimitConfigDto;
  /** Connection ID */
  connectionId: string;
  /** Connection title */
  connectionTitle: string;
  /** Integration ID (connection type) */
  integrationId?: string | null;
  /** Computed metrics */
  metrics: RateLimitMetricsDto;
  /** Aggregated stats for the requested time period */
  periodStats?: null | PeriodStatsDto;
  /** Real-time state (from Redis) */
  state: RateLimitStateDto;
}

/** A single time bucket in the timeline */
export interface RateLimitTimelineBucket {
  /**
   * Start of the time bucket
   * @format date-time
   */
  bucket: string;
  /**
   * Number of rate_limited events in this bucket
   * @format int64
   */
  rateLimitedCount: number;
  /**
   * Number of request events in this bucket
   * @format int64
   */
  requestCount: number;
  /**
   * Number of retry events in this bucket
   * @format int64
   */
  retryCount: number;
}

/** Response data for the timeline endpoint */
export interface RateLimitTimelineData {
  buckets: RateLimitTimelineBucket[];
  connectionId: string;
  /** @format date-time */
  endTime: string;
  granularity: string;
  /** @format date-time */
  startTime: string;
}

/** Response for the timeline endpoint */
export interface RateLimitTimelineResponse {
  /** @min 0 */
  bucketCount: number;
  /** Response data for the timeline endpoint */
  data: RateLimitTimelineData;
  success: boolean;
}

/**
 * A reference to data at a specific path.
 *
 * Paths use dot notation: "data.user.name", "steps.step1.outputs.items", "variables.counter"
 *
 * Available root contexts:
 * - `data` - Current iteration data (in Split) or workflow input data
 * - `variables` - Workflow variables (user-defined + built-in)
 * - `steps.<stepId>.outputs` - Outputs from a previous step
 * - `workflow.inputs` - Original workflow inputs
 *
 * Built-in variables (available in all steps, including subgraphs):
 * - `variables._workflow_id` - Unique per execution: "{workflow_id}::{instance_id}"
 * - `variables._instance_id` - Execution instance UUID
 * - `variables._tenant_id` - Tenant identifier
 *
 * Example: `{ "valueType": "reference", "value": "data.user.name" }`
 * With type hint: `{ "valueType": "reference", "value": "steps.http.outputs.body.count", "type": "int" }`
 */
export interface ReferenceValue {
  /**
   * Default value to use when the reference path returns null or doesn't exist.
   * This allows graceful handling of optional fields while providing fallback values.
   */
  default?: any;
  /**
   * Expected type hint for the referenced value.
   * Used when the source type is unknown (e.g., HTTP response body).
   * If omitted, the value is passed through as-is (typically as JSON).
   */
  type?: null | ValueType;
  /** Path to the data using dot notation (e.g., "data.user.name") */
  value: string;
}

/** Request to rename a folder (updates all workflows in that folder and subfolders) */
export interface RenameFolderRequest {
  /** New folder path (e.g., "/Revenue/") */
  newPath: string;
  /** Current folder path (e.g., "/Sales/") */
  oldPath: string;
}

/** Response for rename folder operation */
export interface RenameFolderResponse {
  newPath: string;
  oldPath: string;
  success: boolean;
  /**
   * Number of workflows updated
   * @format int64
   * @min 0
   */
  workflowsUpdated: number;
}

export interface Schema {
  columns: ColumnDefinition[];
  createdAt: string;
  description?: string | null;
  id: string;
  indexes?: IndexDefinition[] | null;
  name: string;
  tableName: string;
  tenantId: string;
  updatedAt: string;
}

/** Column info from the schema, returned in preview */
export interface SchemaColumnInfo {
  name: string;
  nullable: boolean;
  type: string;
  /** Whether the column has a UNIQUE constraint */
  unique?: boolean;
}

/**
 * A field definition for input/output schemas.
 *
 * Used to define the structure of workflow inputs and outputs.
 * The field name is the key in the HashMap.
 *
 * ## Form rendering extensions
 *
 * The optional fields `label`, `placeholder`, `order`, `format`, `min`, `max`,
 * `pattern`, `properties`, and `visible_when` enable clients to render rich
 * forms from WaitForSignal response schemas. All are backward-compatible —
 * existing schemas without these fields continue to work unchanged.
 */
export interface SchemaField {
  /** Default value if not provided */
  default?: any;
  /** Human-readable description */
  description?: string | null;
  /** Allowed values (enum) */
  enum?: any[] | null;
  /** Example value for documentation */
  example?: any;
  /**
   * Display format hint for the field type.
   *
   * For `string` type: `textarea`, `date`, `datetime`, `email`, `url`,
   * `tel`, `color`, `password`, `markdown`.
   * Unknown formats fall back to the default input for the type.
   */
  format?: string | null;
  /** For array types, the type of items in the array */
  items?: null | SchemaField;
  /**
   * Short display label for form rendering.
   * Falls back to the humanized field key name if not provided.
   */
  label?: string | null;
  /**
   * Maximum value (for numbers) or maximum length (for strings).
   * @format double
   */
  max?: number | null;
  /**
   * Minimum value (for numbers) or minimum length (for strings).
   * @format double
   */
  min?: number | null;
  /**
   * Sort order for rendering fields in forms.
   * Lower values appear first. Falls back to alphabetical order if not set.
   * @format int32
   */
  order?: number | null;
  /** Regex validation pattern (for string fields). */
  pattern?: string | null;
  /** Placeholder text shown in empty inputs. */
  placeholder?: string | null;
  /**
   * Sub-fields for `type: "object"`.
   * Uses the same flat-map format recursively.
   */
  properties?: Partial<Record<string, SchemaField>> | null;
  /** Whether this field is required */
  required?: boolean;
  /** Field type (string, integer, number, boolean, array, object) */
  type: SchemaFieldType;
  /**
   * Conditional visibility — show this field only when a sibling field
   * matches a specific value.
   */
  visibleWhen?: null | VisibleWhen;
}

export interface ScoreExpression {
  /** Output alias. Must be `[a-zA-Z_][a-zA-Z0-9_]*`. */
  alias: string;
  /**
   * Expression tree. Same shape as aggregate `EXPR`, plus the
   * whitelisted function-call form `{fn: "SIMILARITY"|"GREATEST"|"LEAST",
   * arguments: [...]}`.
   */
  expression: any;
}

/**
 * Configuration for a Split step.
 * Defines the array to iterate over and execution options.
 */
export interface SplitConfig {
  /**
   * Allow null values as input (default: false).
   * When true, null input is treated as an empty array (zero iterations).
   * When false, null input raises an error.
   */
  allowNull?: boolean | null;
  /**
   * Batch size for grouping array elements into sub-arrays before iteration.
   *
   * When 0 or unset (the default), the array is split element-by-element —
   * `[1,2,3,4,5]` yields five iterations with items `1, 2, 3, 4, 5`.
   *
   * When > 0, elements are grouped into chunks of `batch_size` (last chunk
   * may be shorter). For example with `batch_size: 2`, `[1,2,3,4,5]` yields
   * three iterations with items `[1,2]`, `[3,4]`, `[5]`. Each iteration's
   * subgraph receives an array value instead of an individual element.
   * @format int32
   * @min 0
   */
  batchSize?: number | null;
  /**
   * Convert single values to a single-element array (default: false).
   * When true, non-array values are wrapped in an array.
   * When false, non-array values raise an error.
   * Use `transform/ensure-array` agent for explicit conversion.
   */
  convertSingleValue?: boolean | null;
  /** Continue execution even if some iterations fail */
  dontStopOnFailed?: boolean | null;
  /**
   * Maximum retry attempts for the split operation (default: 0 - no retries)
   * @format int32
   * @min 0
   */
  maxRetries?: number | null;
  /**
   * Maximum concurrent iterations (0 = unlimited)
   * @format int32
   * @min 0
   */
  parallelism?: number | null;
  /**
   * Base delay between retries in milliseconds (default: 1000)
   * @format int64
   * @min 0
   */
  retryDelay?: number | null;
  /** Execute iterations sequentially instead of in parallel */
  sequential?: boolean | null;
  /**
   * Step timeout in milliseconds. If exceeded, step fails.
   * @format int64
   * @min 0
   */
  timeout?: number | null;
  /** The array to iterate over */
  value: MappingValue;
  /** Additional variables to pass to each iteration's subgraph */
  variables?: null | HashMap;
}

/**
 * Iterates over an array, executing subgraph for each item.
 *
 * Each iteration's outer-array entry is whatever the subgraph's reachable
 * `Finish` step returns (via its `inputMapping`). If `output_schema` is
 * non-empty, the per-iteration result is checked for required fields before
 * being collected — extra fields are allowed, missing required fields fail
 * the iteration. Likewise `input_schema` validates each iteration's `data`
 * (the array element) before the subgraph runs.
 */
export interface SplitStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Split configuration: array to iterate, parallelism settings, error handling */
  config?: null | SplitConfig;
  /**
   * Disable durability for this step when `Some(false)`. Skips checkpoint
   * on the split's final result; iteration subgraph steps remain durable
   * according to the enclosing workflow setting (step-level flag does not
   * leak into the subgraph).
   */
  durable?: boolean | null;
  /** Unique step identifier */
  id: string;
  /**
   * Schema defining the expected shape of each item in the array.
   * Keys are field names, values define the field type and constraints.
   *
   * Validation is permissive: required fields must be present and
   * type-compatible; extra fields are allowed. A missing required field
   * causes the iteration to fail (see `SplitConfig.dontStopOnFailed`).
   */
  inputSchema?: Partial<Record<string, SchemaField>>;
  /** Human-readable step name */
  name?: string | null;
  /**
   * Schema defining the expected output from each iteration.
   * Keys are field names, values define the field type and constraints.
   *
   * Validation is permissive: required fields must be present and
   * type-compatible in the iteration's result; extra fields are allowed.
   * The result is whatever the subgraph's reachable Finish step returned.
   */
  outputSchema?: Partial<Record<string, SchemaField>>;
  /** Nested execution graph for each iteration */
  subgraph: ExecutionGraph;
}

/** Union of all step types, discriminated by stepType field */
export type Step =
  | (FinishStep & {
      stepType: "Finish";
    })
  | (AgentStep & {
      stepType: "Agent";
    })
  | (ConditionalStep & {
      stepType: "Conditional";
    })
  | (SplitStep & {
      stepType: "Split";
    })
  | (SwitchStep & {
      stepType: "Switch";
    })
  | (EmbedWorkflowStep & {
      stepType: "EmbedWorkflow";
    })
  | (WhileStep & {
      stepType: "While";
    })
  | (LogStep & {
      stepType: "Log";
    })
  | (ErrorStep & {
      stepType: "Error";
    })
  | (FilterStep & {
      stepType: "Filter";
    })
  | (GroupByStep & {
      stepType: "GroupBy";
    })
  | (DelayStep & {
      stepType: "Delay";
    })
  | (WaitForSignalStep & {
      stepType: "WaitForSignal";
    })
  | (AiAgentStep & {
      stepType: "AiAgent";
    });

/** Common fields shared by all step types */
export interface StepCommon {
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/** Individual step execution event */
export interface StepEvent {
  /**
   * Execution duration in milliseconds
   * @format int64
   */
  durationMs?: number | null;
  /** Error message (only present if status is "failed") */
  error?: string | null;
  /** Step inputs (JSON string, truncated at 100KB) */
  inputs: string;
  /** Step outputs (JSON string, truncated at 100KB) */
  outputs: string;
  /**
   * Execution sequence number (0-indexed)
   * @format int64
   */
  sequence: number;
  /** Step status: "running", "completed", "failed" */
  status: string;
  /** Step identifier from workflow definition */
  stepId: string;
  /** Step type (Agent, Conditional, Split, etc.) */
  stepType: string;
  /**
   * Start time (Unix milliseconds)
   * @format int64
   */
  timestampMs: number;
}

/** Execution event for step subinstances */
export interface StepEventDto {
  eventData: any;
  /** @format int64 */
  eventId: number;
  eventType: string;
  timestamp: string;
}

/** Individual step event in the response */
export interface StepEventResponse {
  /** Associated checkpoint ID if any */
  checkpointId?: string | null;
  /**
   * When the event was created
   * @format date-time
   */
  createdAt: string;
  /** Event type (e.g., "custom") */
  eventType: string;
  /**
   * Event ID from the database
   * @format int64
   */
  id: number;
  /** Event payload (parsed JSON) */
  payload?: any;
  /** Event subtype (e.g., "step_debug_start", "step_debug_end") */
  subtype?: string | null;
}

/** Step events data container */
export interface StepEventsData {
  /** @min 0 */
  count: number;
  events: StepEvent[];
  instanceId: string;
  workflowId: string;
}

/** Response wrapper for step events with total count */
export interface StepEventsResponse {
  /** Step events response data with pagination info */
  data: StepEventsResponseData;
  message: string;
  success: boolean;
}

/** Step events response data with pagination info */
export interface StepEventsResponseData {
  /** @min 0 */
  count: number;
  events: StepEventResponse[];
  instanceId: string;
  /**
   * @format int32
   * @min 0
   */
  limit: number;
  /**
   * @format int32
   * @min 0
   */
  offset: number;
  /**
   * @format int32
   * @min 0
   */
  totalCount: number;
  workflowId: string;
}

/** Response for step subinstances query */
export interface StepSubinstancesResponse {
  /** @min 0 */
  count: number;
  instanceId: string;
  stepId: string;
  subinstances: StepEventDto[];
  success: boolean;
  timestamp: string;
}

/** Response wrapper for step summaries (used for OpenAPI documentation) */
export interface StepSummariesResponse {
  /** Step summaries response data with pagination info (used for OpenAPI documentation) */
  data: StepSummariesResponseData;
  message: string;
  success: boolean;
}

/** Step summaries response data with pagination info (used for OpenAPI documentation) */
export interface StepSummariesResponseData {
  /** @min 0 */
  count: number;
  instanceId: string;
  /**
   * @format int32
   * @min 0
   */
  limit: number;
  /**
   * @format int32
   * @min 0
   */
  offset: number;
  steps: StepSummaryResponse[];
  /**
   * @format int32
   * @min 0
   */
  totalCount: number;
  workflowId: string;
}

/** Individual step summary in the response */
export interface StepSummaryResponse {
  /**
   * When the step completed (null if still running)
   * @format date-time
   */
  completedAt?: string | null;
  /**
   * Execution duration in milliseconds
   * @format int64
   */
  durationMs?: number | null;
  /** Error details (if failed) */
  error?: any;
  /** Step input data */
  inputs?: any;
  /** Step output data (if completed) */
  outputs?: any;
  /** Parent scope ID for nesting */
  parentScopeId?: string | null;
  /** Step's scope ID for hierarchy */
  scopeId?: string | null;
  /**
   * When the step started
   * @format date-time
   */
  startedAt: string;
  /** Step execution status */
  status: string;
  /** Unique step identifier */
  stepId: string;
  /** Human-readable step name */
  stepName?: string | null;
  /** Step type (e.g., "Http", "Transform", "Agent") */
  stepType: string;
}

/** Step type information */
export interface StepTypeInfo {
  category: string;
  description: string;
  id: string;
  name: string;
}

/**
 * A single case in a Switch step.
 * Defines a match condition and the output to produce if matched.
 */
export interface SwitchCase {
  /** The value to match against (interpretation depends on match_type) */
  match: any;
  /** The type of match to perform */
  matchType: SwitchMatchType;
  /** The output to produce if this case matches */
  output: any;
  /**
   * Route label for routing switches. When present, the switch acts as a
   * branching control flow step. The label corresponds to edge labels in
   * the execution plan.
   */
  route?: string | null;
}

/**
 * Configuration for a Switch step.
 * Defines the value to switch on, the cases to match, and the default output.
 */
export interface SwitchConfig {
  /** Array of cases to match against the value */
  cases?: SwitchCase[];
  /** Default output if no case matches */
  default?: any;
  /** The value to switch on (evaluated at runtime) */
  value: MappingValue;
}

/** Multi-way branch based on value matching */
export interface SwitchStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Switch configuration: value to switch on, cases, and default */
  config?: null | SwitchConfig;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
}

/** System analytics data containing memory, disk, and CPU information */
export interface SystemAnalyticsData {
  /** CPU information */
  cpu: CpuInfo;
  /** Disk space information for the data directory */
  disk: DiskInfo;
  /** Memory information */
  memory: MemoryInfo;
}

/** Response for system analytics endpoint */
export interface SystemAnalyticsResponse {
  /** System analytics data containing memory, disk, and CPU information */
  data: SystemAnalyticsData;
  message: string;
  success: boolean;
}

/**
 * A template value rendered with minijinja using the full execution context.
 *
 * Templates support full minijinja syntax: variable interpolation, filters, conditionals, loops.
 *
 * Available context variables (same as reference resolution):
 * - `data.*` — workflow input data
 * - `variables.*` — workflow variables
 * - `steps.<id>.outputs.*` — previous step outputs
 * - `workflow.inputs.*` — original workflow inputs
 *
 * Example: `{ "valueType": "template", "value": "Bearer {{ steps.my_conn.outputs.parameters.api_key }}" }`
 * With filter: `{ "valueType": "template", "value": "{{ data.name | upper }}" }`
 */
export interface TemplateValue {
  /** Minijinja template string */
  value: string;
}

/** Tenant metrics response data */
export interface TenantMetricsData {
  /** @format date-time */
  endTime: string;
  metrics: TenantMetricsDataPoint[];
  /** @format date-time */
  startTime: string;
  tenantId: string;
}

/** Tenant metrics data point */
export interface TenantMetricsDataPoint {
  /** @format double */
  avgDurationSeconds?: number | null;
  /** @format double */
  avgMemoryMb?: number | null;
  /** @format date-time */
  dayBucket?: string | null;
  /** @format int64 */
  failureCount?: number | null;
  /** @format int64 */
  invocationCount?: number | null;
  /** @format int64 */
  successCount?: number | null;
  /** @format double */
  successRatePercent?: number | null;
  /** @format int64 */
  timeoutCount?: number | null;
}

/** Response for tenant metrics */
export interface TenantMetricsResponse {
  /** Tenant metrics response data */
  data: TenantMetricsData;
  message: string;
  success: boolean;
}

/** Error response for agent testing */
export interface TestAgentErrorResponse {
  error: string;
  message?: string | null;
  success: boolean;
}

/**
 * Request body for testing an agent
 * @example {"connectionId":"e9af2f09-0666-43b2-9173-b1ce6ac0c739","input":{}}
 */
export interface TestAgentRequest {
  /**
   * Optional connection ID for agents that require connections (e.g., HTTP, Shopify).
   * If provided, the connection will be looked up and passed to the agent.
   * The connection must belong to the authenticated tenant and be in ACTIVE status.
   */
  connectionId?: string | null;
  /**
   * Input data for the agent (structure depends on the specific agent).
   * Most agents expect an object with specific fields, or an empty object {}.
   * If omitted, defaults to an empty object {}.
   * Example for random-double: {"input": {}}
   * Example for calculate: {"input": {"expression": "2 + 2", "variables": {}}}
   */
  input?: object;
}

/** Response from testing an agent */
export interface TestAgentResponse {
  error?: string | null;
  /** @format double */
  executionTimeMs: number;
  /** @format double */
  maxMemoryMb?: number | null;
  output?: any;
  success: boolean;
}

/** Update connection request - all fields optional */
export interface UpdateConnectionRequest {
  connectionParameters?: any;
  connectionSubtype?: string | null;
  /** Connection type identifier that maps to a connection schema (e.g., shopify_access_token, bearer, sftp) */
  integrationId?: string | null;
  isDefaultFileStorage?: boolean | null;
  rateLimitConfig?: null | RateLimitConfigDto;
  status?: null | ConnectionStatus;
  title?: string | null;
  validUntil?: string | null;
}

export interface UpdateInstanceRequest {
  properties: any;
}

export interface UpdateInstanceResponse {
  message: string;
  success: boolean;
}

/** Request payload for updating an invocation trigger */
export interface UpdateInvocationTriggerRequest {
  /**
   * Whether the trigger is currently active
   * @example true
   */
  active: boolean;
  /** Trigger-specific configuration in JSON format */
  configuration?: object | null;
  /**
   * Remote tenant identifier for external system triggers
   * @example "remote-tenant-789"
   */
  remote_tenant_id?: string | null;
  /**
   * Whether only a single instance of this trigger should run at a time
   * @example false
   */
  single_instance: boolean;
  /** Type of trigger */
  trigger_type: TriggerType;
  /**
   * Reference to the workflow to be invoked
   * @example "workflow-456"
   */
  workflow_id: string;
}

export interface UpdateSchemaRequest {
  columns?: ColumnDefinition[] | null;
  description?: string | null;
  indexes?: IndexDefinition[] | null;
  name?: string | null;
}

export interface UpdateSchemaResponse {
  message: string;
  success: boolean;
}

export interface UpdateTrackEventsRequest {
  /** Enable or disable step-event tracking for this workflow version */
  trackEvents: boolean;
}

export interface UpdateWorkflowRequest {
  /**
   * The execution graph containing workflow definition.
   * Must include 'name' and optionally 'description' fields.
   */
  executionGraph: any;
  memoryTier?: null | MemoryTier;
  /** Enable step-event tracking for this workflow version (optional, keeps existing if not provided) */
  trackEvents?: boolean | null;
}

export interface UploadResponse {
  key: string;
  /**
   * Size of uploaded file in bytes
   * @format int64
   * @min 0
   */
  size: number;
  success: boolean;
}

/** Response for validate-mappings endpoint */
export interface ValidateMappingsResponse {
  /** @min 0 */
  errorCount: number;
  issues: ValidationIssue[];
  success: boolean;
  /** @format int32 */
  version?: number | null;
  /** @min 0 */
  warningCount: number;
  workflowId: string;
}

/** Structured validation error with step context for frontend highlighting */
export interface ValidationErrorDto {
  /** Error code (e.g., "E023") */
  code: string;
  /** Field name with the error (if applicable) */
  fieldName?: string | null;
  /** Human-readable error message */
  message: string;
  /** Additional step IDs involved (for errors spanning multiple steps) */
  relatedStepIds?: string[] | null;
  /** Step ID where the error occurred (if applicable) */
  stepId?: string | null;
}

/** A validation issue with structured information */
export interface ValidationIssue {
  /** Category of the issue */
  category: IssueCategory;
  /** Field name in input_mapping (if applicable) */
  fieldName?: string | null;
  /** Human-readable message */
  message: string;
  /** The problematic reference path (if applicable) */
  referencePath?: string | null;
  /** Severity: error (blocking) or warning (non-blocking) */
  severity: IssueSeverity;
  /** Step ID where the issue was found */
  stepId: string;
}

/**
 * A typed variable definition with its value.
 *
 * Variables are static values available during workflow execution
 * via the `variables.*` path in mappings.
 */
export interface Variable {
  /** Human-readable description */
  description?: string | null;
  /** Variable type */
  type: VariableType;
  /** The actual value (must match the declared type) */
  value: any;
}

/** Mirror of `runtara_object_store::VectorIndexMethod` for the HTTP DTO. */
export type VectorIndexMethod =
  | {
      type: "hnsw";
    }
  | {
      /**
       * @format int32
       * @min 0
       */
      lists: number;
      type: "ivfflat";
    };

/** Response containing schemas from a specific workflow version's execution graph */
export interface VersionSchemasResponse {
  /** Input schema definition from the execution graph */
  inputSchema: any;
  /** Output schema definition from the execution graph */
  outputSchema: any;
  /** Variables defined in the execution graph */
  variables: any;
}

/**
 * Conditional visibility rule for a schema field.
 *
 * When attached to a field, the field is only shown in forms if the
 * referenced sibling field matches the condition. Only single-level
 * comparisons are supported — no complex boolean logic.
 *
 * Example:
 * ```json
 * { "field": "approved", "equals": false }
 * ```
 */
export interface VisibleWhen {
  /** Show this field when the sibling equals this value. */
  equals?: any;
  /** The sibling field name to check. */
  field: string;
  /** Show this field when the sibling does NOT equal this value. */
  notEquals?: any;
}

/**
 * Wait for an external signal before continuing execution.
 *
 * This step pauses workflow execution until an external system sends a signal
 * with the matching signal_id. The signal_id is auto-generated based on the
 * step's position in the workflow (instance_id + workflow context + step_id + loop indices).
 *
 * The `on_wait` subgraph executes immediately when the step starts waiting,
 * allowing the workflow to notify external systems of the signal_id they should
 * use to resume execution.
 *
 * Example:
 * ```json
 * {
 *   "stepType": "WaitForSignal",
 *   "id": "approval",
 *   "name": "Wait for manager approval",
 *   "onWait": {
 *     "name": "Notify approver",
 *     "entryPoint": "send_notification",
 *     "steps": {
 *       "send_notification": {
 *         "stepType": "Agent",
 *         "id": "send_notification",
 *         "agentId": "http",
 *         "capabilityId": "http-request",
 *         "inputMapping": {
 *           "url": { "valueType": "immediate", "value": "https://approval-system/request" },
 *           "body": {
 *             "valueType": "composite",
 *             "value": {
 *               "signal_id": { "valueType": "reference", "value": "variables._signal_id" },
 *               "instance_id": { "valueType": "reference", "value": "variables._instance_id" }
 *             }
 *           }
 *         }
 *       },
 *       "finish": { "stepType": "Finish", "id": "finish" }
 *     },
 *     "executionPlan": [{ "fromStep": "send_notification", "toStep": "finish" }]
 *   },
 *   "timeoutMs": 86400000
 * }
 * ```
 */
export interface WaitForSignalStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
  /**
   * Subgraph to execute when starting to wait.
   * This runs before suspending and is typically used to notify
   * external systems of the signal_id they should use.
   * The subgraph has access to `variables._signal_id` and `variables._instance_id`.
   */
  onWait?: null | ExecutionGraph;
  /**
   * Polling interval in milliseconds for checking signal (default: 1000).
   * Lower values mean faster response but more server load.
   * @format int64
   * @min 0
   */
  pollIntervalMs?: number | null;
  /**
   * Schema describing the expected response from the human/external system.
   * Uses the same flat-map format as workflow `inputSchema`.
   *
   * Examples:
   * - Confirm: `{"approved": {"type": "boolean", "required": true}}`
   * - Choice: `{"decision": {"type": "string", "required": true, "enum": ["approve", "reject"]}}`
   * - Text: `{"response": {"type": "string", "required": true}}`
   *
   * When used as an AI Agent tool, this schema is exposed to the LLM as tool
   * parameters and included in debug events so the frontend can render the
   * appropriate input widget.
   */
  responseSchema?: Partial<Record<string, SchemaField>> | null;
  /**
   * Optional timeout in milliseconds.
   * If the signal is not received within this duration, the step fails.
   */
  timeoutMs?: null | MappingValue;
}

/** Configuration for a While step. */
export interface WhileConfig {
  /**
   * Maximum number of iterations (default: 10).
   * Prevents infinite loops.
   * @format int32
   * @min 0
   */
  maxIterations?: number | null;
  /**
   * Step timeout in milliseconds. If exceeded, step fails.
   * @format int64
   * @min 0
   */
  timeout?: number | null;
}

/** Conditional loop - repeat subgraph until condition is false */
export interface WhileStep {
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean | null;
  /**
   * The condition expression to evaluate before each iteration.
   * Loop continues while condition is true.
   */
  condition: ConditionExpression;
  /** While loop configuration */
  config?: null | WhileConfig;
  /** Unique step identifier */
  id: string;
  /** Human-readable step name */
  name?: string | null;
  /** Nested execution graph to execute on each iteration */
  subgraph: ExecutionGraph;
}

/** Complete workflow definition */
export interface Workflow {
  /**
   * Disable durability for this workflow when `false`. Compiled code contains
   * no checkpoint reads/writes, no `sdk.durable_sleep`, and no breakpoint
   * checkpoints. When this field is `Some(false)`, the setting propagates
   * into `ExecutionGraph.durable` (via `parse_workflow`) and then to every
   * nested subgraph and embedded child workflow at codegen time. Default: durable.
   */
  durable?: boolean | null;
  /** The execution graph containing all steps */
  executionGraph: ExecutionGraph;
  /** Memory allocation tier for workflow execution */
  memoryTier?: null | MemoryTier;
  /** Enable step-level debug instrumentation */
  trackEvents?: boolean | null;
}

export interface WorkflowDto {
  created: string;
  /**
   * The active/current version that will be used when executing this workflow
   * Can be set explicitly via the set-current-version endpoint, otherwise defaults to latest_version
   * @format int32
   */
  currentVersionNumber: number;
  description: string;
  executionGraph: any;
  /** @format int64 */
  executionTime?: number | null;
  /** @format int64 */
  executionTimeout?: number | null;
  finished?: string | null;
  id: string;
  inputSchema: any;
  /**
   * The highest version number that exists for this workflow
   * @format int32
   */
  lastVersionNumber: number;
  /** Memory allocation tier for workflow execution */
  memoryTier?: MemoryTier;
  name: string;
  /** Visual notes/annotations for the workflow canvas */
  notes?: Note[];
  outputSchema: any;
  /**
   * Folder path for organization (e.g., "/Sales/Shopify/")
   * Defaults to "/" (root folder)
   */
  path?: string;
  started?: string | null;
  /** Whether this version is compiled with step-event tracking instrumentation */
  trackEvents?: boolean;
  updated: string;
  /** Default variable values (can be overridden at execution time) */
  variables?: any;
}

export interface WorkflowInstanceDto {
  created: string;
  /** @format double */
  executionDurationSeconds?: number | null;
  /** Whether this execution has pending human input requests (AI Agent waiting for signal) */
  hasPendingInput?: boolean;
  id: string;
  inputs: InstanceInputs;
  /** @format double */
  maxMemoryMb?: number | null;
  outputs?: any;
  /** @format double */
  processingOverheadSeconds?: number | null;
  /** @format double */
  queueDurationSeconds?: number | null;
  /** Current execution status */
  status: ExecutionStatus;
  steps?: WorkflowStepDto[];
  tags?: string[];
  /** Reason for termination (set for all terminal states including successful completion) */
  terminationType?: null | TerminationType;
  updated: string;
  /** @format int32 */
  usedVersion: number;
  workflowId: string;
  /** Workflow name (populated when listing all executions) */
  workflowName?: string | null;
}

/** Daily aggregated metrics */
export interface WorkflowMetricsDaily {
  /** @format double */
  avgDurationSeconds?: number | null;
  /** @format double */
  avgMemoryMb?: number | null;
  /** @format double */
  avgProcessingOverheadSeconds?: number | null;
  /** @format double */
  avgQueueDurationSeconds?: number | null;
  /** @format date-time */
  dayBucket?: string | null;
  /** @format int64 */
  failureCount?: number | null;
  /** @format int64 */
  invocationCount?: number | null;
  /** @format double */
  maxDurationSeconds?: number | null;
  /** @format double */
  maxMemoryMb?: number | null;
  /** @format double */
  maxProcessingOverheadSeconds?: number | null;
  /** @format double */
  maxQueueDurationSeconds?: number | null;
  /** @format double */
  minDurationSeconds?: number | null;
  /** @format double */
  minMemoryMb?: number | null;
  /** @format double */
  minProcessingOverheadSeconds?: number | null;
  /** @format double */
  minQueueDurationSeconds?: number | null;
  /** @format int64 */
  successCount?: number | null;
  /** @format double */
  successRatePercent?: number | null;
  tenantId: string;
  /** @format int64 */
  timeoutCount?: number | null;
  /** @format int32 */
  version: number;
  workflowId: string;
}

/** Response for workflow metrics (daily) */
export interface WorkflowMetricsDailyResponse {
  /** Response data for workflow metrics endpoint */
  data: WorkflowMetricsData;
  message: string;
  success: boolean;
}

/** Response data for workflow metrics endpoint */
export interface WorkflowMetricsData {
  /** @format date-time */
  endTime: string;
  granularity: string;
  metrics: WorkflowMetricsDaily[];
  /** @format date-time */
  startTime: string;
  /** @format int32 */
  version?: number | null;
  workflowId: string;
}

/** Hourly metrics for a workflow */
export interface WorkflowMetricsHourly {
  /** @format date-time */
  created_at: string;
  /** @format int32 */
  failure_count: number;
  /** @format date-time */
  hour_bucket: string;
  /** @format int64 */
  id: number;
  /** @format int32 */
  invocation_count: number;
  /** @format double */
  max_duration_seconds?: number | null;
  /** @format double */
  max_memory_mb?: number | null;
  /** @format double */
  max_processing_overhead_seconds?: number | null;
  /** @format double */
  max_queue_duration_seconds?: number | null;
  /** @format double */
  min_duration_seconds?: number | null;
  /** @format double */
  min_memory_mb?: number | null;
  /** @format double */
  min_processing_overhead_seconds?: number | null;
  /** @format double */
  min_queue_duration_seconds?: number | null;
  side_effect_counts: any;
  /** @format int32 */
  success_count: number;
  tenant_id: string;
  /** @format int32 */
  timeout_count: number;
  /** @format double */
  total_duration_seconds?: number | null;
  /** @format double */
  total_memory_mb?: number | null;
  /** @format double */
  total_processing_overhead_seconds?: number | null;
  /** @format double */
  total_queue_duration_seconds?: number | null;
  /** @format date-time */
  updated_at: string;
  /** @format int32 */
  version: number;
  workflow_id: string;
}

/** Response data for workflow metrics hourly endpoint */
export interface WorkflowMetricsHourlyData {
  /** @format date-time */
  endTime: string;
  granularity: string;
  metrics: WorkflowMetricsHourly[];
  /** @format date-time */
  startTime: string;
  /** @format int32 */
  version?: number | null;
  workflowId: string;
}

/** Response for workflow metrics (hourly) */
export interface WorkflowMetricsHourlyResponse {
  /** Response data for workflow metrics hourly endpoint */
  data: WorkflowMetricsHourlyData;
  message: string;
  success: boolean;
}

/** Overall workflow statistics */
export interface WorkflowStats {
  /** @format double */
  avgDurationSeconds?: number | null;
  /** @format double */
  avgMemoryMb?: number | null;
  /** @format double */
  avgProcessingOverheadSeconds?: number | null;
  /** @format double */
  avgQueueDurationSeconds?: number | null;
  /** @format double */
  maxDurationSeconds?: number | null;
  /** @format double */
  maxMemoryMb?: number | null;
  /** @format double */
  maxProcessingOverheadSeconds?: number | null;
  /** @format double */
  maxQueueDurationSeconds?: number | null;
  /** @format double */
  minDurationSeconds?: number | null;
  /** @format double */
  minMemoryMb?: number | null;
  /** @format double */
  minProcessingOverheadSeconds?: number | null;
  /** @format double */
  minQueueDurationSeconds?: number | null;
  /** @format double */
  p95DurationSeconds?: number | null;
  /** @format double */
  p95QueueDurationSeconds?: number | null;
  /** @format double */
  p99DurationSeconds?: number | null;
  /** @format double */
  p99QueueDurationSeconds?: number | null;
  /** @format double */
  successRatePercent?: number | null;
  /** @format int64 */
  totalFailures?: number | null;
  /** @format int64 */
  totalInvocations?: number | null;
  /** @format int64 */
  totalSuccesses?: number | null;
  /** @format int64 */
  totalTimeouts?: number | null;
}

/** Statistics data */
export interface WorkflowStatsData {
  /** Overall workflow statistics */
  stats: WorkflowStats;
  /** @format int32 */
  version?: number | null;
  workflowId: string;
}

/** Response for workflow statistics */
export interface WorkflowStatsResponse {
  /** Statistics data */
  data: WorkflowStatsData;
  message: string;
  success: boolean;
}

export interface WorkflowStepDto {
  connectionDataId?: string | null;
  created: string;
  /** @format int64 */
  executionTime?: number | null;
  /** @format int64 */
  executionTimeout?: number | null;
  finished?: string | null;
  id: string;
  inputMapping?: any;
  inputs?: any;
  /** @format int32 */
  maxDepth?: number | null;
  nextStepId?: string | null;
  outputs?: any;
  started?: string | null;
  stepLabel?: string | null;
  stepName?: string | null;
  stepType?: string | null;
  subInstances?: string[];
  updated: string;
  workflowInstanceId?: string | null;
}

/** Response returned when workflow validation fails */
export interface WorkflowValidationErrorResponse {
  /** Summary message describing the validation failure */
  message: string;
  /** Always false for error responses */
  success: boolean;
  /** Detailed validation errors with step context */
  validationErrors: ValidationErrorDto[];
}

export interface WorkflowVersionInfoDto {
  /** Whether this version has been compiled */
  compiled: boolean;
  /** Timestamp when this version was compiled (RFC3339 format, null if not compiled) */
  compiledAt?: string | null;
  createdAt: string;
  /** Whether this is the current/active version used for execution */
  isActive: boolean;
  /** Whether step-event tracking is enabled for this version */
  trackEvents: boolean;
  updatedAt: string;
  versionId: string;
  /** @format int32 */
  versionNumber: number;
  workflowId: string;
}

import type {
  AxiosInstance,
  AxiosRequestConfig,
  AxiosResponse,
  HeadersDefaults,
  ResponseType,
} from "axios";
import axios from "axios";

export type QueryParamsType = Record<string | number, any>;

export interface FullRequestParams
  extends Omit<AxiosRequestConfig, "data" | "params" | "url" | "responseType"> {
  /** set parameter to `true` for call `securityWorker` for this request */
  secure?: boolean;
  /** request path */
  path: string;
  /** content type of request body */
  type?: ContentType;
  /** query params */
  query?: QueryParamsType;
  /** format of response (i.e. response.json() -> format: "json") */
  format?: ResponseType;
  /** request body */
  body?: unknown;
}

export type RequestParams = Omit<
  FullRequestParams,
  "body" | "method" | "query" | "path"
>;

export interface ApiConfig<SecurityDataType = unknown>
  extends Omit<AxiosRequestConfig, "data" | "cancelToken"> {
  securityWorker?: (
    securityData: SecurityDataType | null,
  ) => Promise<AxiosRequestConfig | void> | AxiosRequestConfig | void;
  secure?: boolean;
  format?: ResponseType;
}

export enum ContentType {
  Json = "application/json",
  JsonApi = "application/vnd.api+json",
  FormData = "multipart/form-data",
  UrlEncoded = "application/x-www-form-urlencoded",
  Text = "text/plain",
}

export class HttpClient<SecurityDataType = unknown> {
  public instance: AxiosInstance;
  private securityData: SecurityDataType | null = null;
  private securityWorker?: ApiConfig<SecurityDataType>["securityWorker"];
  private secure?: boolean;
  private format?: ResponseType;

  constructor({
    securityWorker,
    secure,
    format,
    ...axiosConfig
  }: ApiConfig<SecurityDataType> = {}) {
    this.instance = axios.create({
      ...axiosConfig,
      baseURL: axiosConfig.baseURL || "",
    });
    this.secure = secure;
    this.format = format;
    this.securityWorker = securityWorker;
  }

  public setSecurityData = (data: SecurityDataType | null) => {
    this.securityData = data;
  };

  protected mergeRequestParams(
    params1: AxiosRequestConfig,
    params2?: AxiosRequestConfig,
  ): AxiosRequestConfig {
    const method = params1.method || (params2 && params2.method);

    return {
      ...this.instance.defaults,
      ...params1,
      ...(params2 || {}),
      headers: {
        ...((method &&
          this.instance.defaults.headers[
            method.toLowerCase() as keyof HeadersDefaults
          ]) ||
          {}),
        ...(params1.headers || {}),
        ...((params2 && params2.headers) || {}),
      },
    };
  }

  protected stringifyFormItem(formItem: unknown) {
    if (typeof formItem === "object" && formItem !== null) {
      return JSON.stringify(formItem);
    } else {
      return `${formItem}`;
    }
  }

  protected createFormData(input: Record<string, unknown>): FormData {
    if (input instanceof FormData) {
      return input;
    }
    return Object.keys(input || {}).reduce((formData, key) => {
      const property = input[key];
      const propertyContent: any[] =
        property instanceof Array ? property : [property];

      for (const formItem of propertyContent) {
        const isFileType = formItem instanceof Blob || formItem instanceof File;
        formData.append(
          key,
          isFileType ? formItem : this.stringifyFormItem(formItem),
        );
      }

      return formData;
    }, new FormData());
  }

  public request = async <T = any, _E = any>({
    secure,
    path,
    type,
    query,
    format,
    body,
    ...params
  }: FullRequestParams): Promise<AxiosResponse<T>> => {
    const secureParams =
      ((typeof secure === "boolean" ? secure : this.secure) &&
        this.securityWorker &&
        (await this.securityWorker(this.securityData))) ||
      {};
    const requestParams = this.mergeRequestParams(params, secureParams);
    const responseFormat = format || this.format || undefined;

    if (
      type === ContentType.FormData &&
      body &&
      body !== null &&
      typeof body === "object"
    ) {
      body = this.createFormData(body as Record<string, unknown>);
    }

    if (
      type === ContentType.Text &&
      body &&
      body !== null &&
      typeof body !== "string"
    ) {
      body = JSON.stringify(body);
    }

    return this.instance.request({
      ...requestParams,
      headers: {
        ...(requestParams.headers || {}),
        ...(type ? { "Content-Type": type } : {}),
      },
      params: query,
      responseType: responseFormat,
      data: body,
      url: path,
    });
  };
}

/**
 * @title Runtara API
 * @version 1
 * @license AGPL-3.0-or-later
 *
 * API for managing workflow definitions with versioning support
 */
export class Api<
  SecurityDataType extends unknown,
> extends HttpClient<SecurityDataType> {
  api = {
    /**
     * @description This is a PUBLIC endpoint (no JWT required) — called by the OAuth provider redirecting the user's browser after consent.
     *
     * @tags oauth-callback
     * @name CallbackHandler
     * @summary Handle the OAuth2 provider callback.
     * @request GET:/api/oauth/{tenant_id}/callback
     */
    callbackHandler: (
      tenantId: string,
      query?: {
        /** Authorization code returned by the provider */
        code?: string;
        /** Opaque state value used for CSRF protection and connection lookup */
        state?: string;
        /** Error code if the provider reports a failure */
        error?: string;
        /** Human-readable error description from the provider */
        error_description?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<void, any>({
        path: `/api/oauth/${tenantId}/callback`,
        method: "GET",
        query: query,
        ...params,
      }),

    /**
     * No description
     *
     * @tags agents-controller
     * @name ListAgentsHandler
     * @summary Get all available agents (without capabilities details)
     * @request GET:/api/runtime/agents
     */
    listAgentsHandler: (params: RequestParams = {}) =>
      this.request<ListAgentsResponse, any>({
        path: `/api/runtime/agents`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Workflow instances call this endpoint to delegate I/O-heavy agent work (HTTP requests, database queries, SFTP operations, etc.) to the host process. The host resolves connections, executes the agent, and returns the result. Pure computation agents (transform, csv, xml, utils, text) can still run in-process in the workflow binary — this endpoint is for agents that require network access or platform-specific dependencies.
     *
     * @tags agents-controller
     * @name ExecuteAgentHandler
     * @summary Execute an agent capability on the host
     * @request POST:/api/runtime/agents/{agent_id}/capabilities/{capability_id}/execute
     */
    executeAgentHandler: (
      agentId: string,
      capabilityId: string,
      data: ExecuteAgentRequest,
      params: RequestParams = {},
    ) =>
      this.request<ExecuteAgentResponse, ExecuteAgentErrorResponse>({
        path: `/api/runtime/agents/${agentId}/capabilities/${capabilityId}/execute`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags agents-controller
     * @name GetAgentHandler
     * @summary Get a specific agent by name
     * @request GET:/api/runtime/agents/{name}
     */
    getAgentHandler: (name: string, params: RequestParams = {}) =>
      this.request<any, void>({
        path: `/api/runtime/agents/${name}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags agents-controller
     * @name GetCapabilityHandler
     * @summary Get a specific capability within an agent
     * @request GET:/api/runtime/agents/{name}/capabilities/{capability_id}
     */
    getCapabilityHandler: (
      name: string,
      capabilityId: string,
      params: RequestParams = {},
    ) =>
      this.request<any, void>({
        path: `/api/runtime/agents/${name}/capabilities/${capabilityId}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description This endpoint allows testing agents in isolation using sandboxed container execution. Agent testing must be enabled via ENABLE_OPERATOR_TESTING=true environment variable.
     *
     * @tags agents-controller
     * @name TestAgentHandler
     * @summary Test an agent capability with given input
     * @request POST:/api/runtime/agents/{name}/capabilities/{capability_id}/test
     */
    testAgentHandler: (
      name: string,
      capabilityId: string,
      data: TestAgentRequest,
      params: RequestParams = {},
    ) =>
      this.request<TestAgentResponse, TestAgentErrorResponse>({
        path: `/api/runtime/agents/${name}/capabilities/${capabilityId}/test`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags agents-controller
     * @name GetAgentConnectionSchemaHandler
     * @summary Get connection schema for an agent (STUB)
     * @request GET:/api/runtime/agents/{name}/connection-schema
     */
    getAgentConnectionSchemaHandler: (
      name: string,
      params: RequestParams = {},
    ) =>
      this.request<any, void>({
        path: `/api/runtime/agents/${name}/connection-schema`,
        method: "GET",
        ...params,
      }),

    /**
     * No description
     *
     * @tags analytics-controller
     * @name GetSystemAnalyticsHandler
     * @summary Get system analytics including memory, disk space, and CPU information
     * @request GET:/api/runtime/analytics/system
     */
    getSystemAnalyticsHandler: (params: RequestParams = {}) =>
      this.request<SystemAnalyticsResponse, any>({
        path: `/api/runtime/analytics/system`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags api-keys-controller
     * @name ListApiKeys
     * @summary List all API keys for the authenticated tenant. Key hashes are never exposed.
     * @request GET:/api/runtime/api-keys
     * @secure
     */
    listApiKeys: (params: RequestParams = {}) =>
      this.request<ApiKey[], void>({
        path: `/api/runtime/api-keys`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags api-keys-controller
     * @name CreateApiKey
     * @summary Create a new API key for the authenticated tenant. The plaintext key is returned ONCE in the response — store it securely.
     * @request POST:/api/runtime/api-keys
     * @secure
     */
    createApiKey: (data: CreateApiKeyRequest, params: RequestParams = {}) =>
      this.request<CreateApiKeyResponse, void>({
        path: `/api/runtime/api-keys`,
        method: "POST",
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags api-keys-controller
     * @name RevokeApiKey
     * @summary Revoke an API key. The key can no longer be used for authentication.
     * @request DELETE:/api/runtime/api-keys/{id}
     * @secure
     */
    revokeApiKey: (id: string, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/api-keys/${id}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),

    /**
     * No description
     *
     * @tags connections-controller
     * @name ListConnectionsHandler
     * @summary List all connections for a tenant SECURITY: Does NOT return connection_parameters field
     * @request GET:/api/runtime/connections
     */
    listConnectionsHandler: (
      query?: {
        /** Filter by integration ID (connection type identifier) */
        integrationId?: string;
        /** Filter by status (UNKNOWN, ACTIVE, REQUIRES_RECONNECTION, INVALID_CREDENTIALS) */
        status?: string;
        /** Include rate limit statistics for each connection */
        includeRateLimitStats?: boolean;
        /** Time interval for rate limit stats: 1h, 24h, 7d, 30d (default: 24h) */
        interval?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListConnectionsResponse, ErrorResponse>({
        path: `/api/runtime/connections`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags connections-controller
     * @name CreateConnectionHandler
     * @summary Create a new connection
     * @request POST:/api/runtime/connections
     */
    createConnectionHandler: (
      data: CreateConnectionRequest,
      params: RequestParams = {},
    ) =>
      this.request<CreateConnectionResponse, ErrorResponse>({
        path: `/api/runtime/connections`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the canonical list of authentication / credential types. Used by the frontend to populate auth type selectors when creating connections.
     *
     * @tags connections-controller
     * @name ListConnectionAuthTypesHandler
     * @summary List all connection auth types
     * @request GET:/api/runtime/connections/auth-types
     */
    listConnectionAuthTypesHandler: (params: RequestParams = {}) =>
      this.request<ListConnectionAuthTypesResponse, any>({
        path: `/api/runtime/connections/auth-types`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the canonical list of connection categories with display names and descriptions. Used by the frontend to populate category filters and grouping UI.
     *
     * @tags connections-controller
     * @name ListConnectionCategoriesHandler
     * @summary List all connection categories
     * @request GET:/api/runtime/connections/categories
     */
    listConnectionCategoriesHandler: (params: RequestParams = {}) =>
      this.request<ListConnectionCategoriesResponse, any>({
        path: `/api/runtime/connections/categories`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Automatically searches for connections that match the operator using: - Direct match: connection_type = operatorName (case-insensitive) - Integration match: integration_id IN operator.integrationIds For example, "Shopify" operator finds connections with: - connection_type = "shopify" (direct Shopify connections) - integration_id = "shopify_access_token" (HTTP connections for Shopify) The operator's supported integration_ids are automatically looked up from the operator registry. SECURITY: Does NOT return connection_parameters field
     *
     * @tags connections-controller
     * @name GetConnectionsByOperatorHandler
     * @summary Get connections by operator name
     * @request GET:/api/runtime/connections/operator/{operatorName}
     */
    getConnectionsByOperatorHandler: (
      operatorName: string,
      query?: {
        /** Filter by status (UNKNOWN, ACTIVE, REQUIRES_RECONNECTION, INVALID_CREDENTIALS) */
        status?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListConnectionsResponse, ErrorResponse>({
        path: `/api/runtime/connections/operator/${operatorName}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Returns all registered connection types with their parameter schemas. This endpoint is used by the frontend to dynamically generate connection forms.
     *
     * @tags connections-controller
     * @name ListConnectionTypesHandler
     * @summary List all available connection types
     * @request GET:/api/runtime/connections/types
     */
    listConnectionTypesHandler: (params: RequestParams = {}) =>
      this.request<ListConnectionTypesResponse, any>({
        path: `/api/runtime/connections/types`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the connection type schema for the given integration_id. This endpoint is used by the frontend to get the form schema for a specific connection type.
     *
     * @tags connections-controller
     * @name GetConnectionTypeHandler
     * @summary Get a specific connection type by integration_id
     * @request GET:/api/runtime/connections/types/{integration_id}
     */
    getConnectionTypeHandler: (
      integrationId: string,
      params: RequestParams = {},
    ) =>
      this.request<ConnectionTypeResponse, ErrorResponse>({
        path: `/api/runtime/connections/types/${integrationId}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags connections-controller
     * @name GetConnectionHandler
     * @summary Get a single connection by ID SECURITY: Does NOT return connection_parameters field
     * @request GET:/api/runtime/connections/{id}
     */
    getConnectionHandler: (id: string, params: RequestParams = {}) =>
      this.request<ConnectionResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags connections-controller
     * @name UpdateConnectionHandler
     * @summary Update a connection
     * @request PUT:/api/runtime/connections/{id}
     */
    updateConnectionHandler: (
      id: string,
      data: UpdateConnectionRequest,
      params: RequestParams = {},
    ) =>
      this.request<ConnectionResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}`,
        method: "PUT",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags connections-controller
     * @name DeleteConnectionHandler
     * @summary Delete a connection
     * @request DELETE:/api/runtime/connections/{id}
     */
    deleteConnectionHandler: (id: string, params: RequestParams = {}) =>
      this.request<DeleteConnectionResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}`,
        method: "DELETE",
        format: "json",
        ...params,
      }),

    /**
     * @description The frontend should open this URL in a popup window. After user consent, the provider redirects to /api/oauth/{tenant_id}/callback.
     *
     * @tags connections-controller
     * @name AuthorizeHandler
     * @summary Generate an OAuth2 authorization URL for a connection.
     * @request GET:/api/runtime/connections/{id}/oauth/authorize
     */
    authorizeHandler: (id: string, params: RequestParams = {}) =>
      this.request<OAuthAuthorizeResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}/oauth/authorize`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns historical rate limit events including requests, rate limited events, and retries for the specified connection. Data is retained for 30 days.
     *
     * @tags rate-limits-controller
     * @name GetConnectionRateLimitHistoryHandler
     * @summary Get rate limit history (timeline) for a connection
     * @request GET:/api/runtime/connections/{id}/rate-limit-history
     */
    getConnectionRateLimitHistoryHandler: (
      id: string,
      query?: {
        /**
         * Maximum events to return (default: 100, max: 1000)
         * @format int64
         */
        limit?: number;
        /**
         * Number of events to skip for pagination
         * @format int64
         */
        offset?: number;
        /** Filter by event type: request, rate_limited, retry */
        event_type?: string;
        /** Filter events after this ISO 8601 timestamp */
        from?: string;
        /** Filter events before this ISO 8601 timestamp */
        to?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<RateLimitHistoryResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}/rate-limit-history`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Returns real-time rate limit state from Redis combined with configuration from PostgreSQL for the specified connection.
     *
     * @tags rate-limits-controller
     * @name GetConnectionRateLimitStatusHandler
     * @summary Get rate limit status for a single connection
     * @request GET:/api/runtime/connections/{id}/rate-limit-status
     */
    getConnectionRateLimitStatusHandler: (
      id: string,
      params: RequestParams = {},
    ) =>
      this.request<GetRateLimitStatusResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}/rate-limit-status`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns aggregated event counts in time buckets (per-minute, hourly, or daily). Supports filtering by tag to see which agent/step generated the requests. Data is retained for 30 days.
     *
     * @tags rate-limits-controller
     * @name GetConnectionRateLimitTimelineHandler
     * @summary Get time-bucketed rate limit timeline for a connection
     * @request GET:/api/runtime/connections/{id}/rate-limit-timeline
     */
    getConnectionRateLimitTimelineHandler: (
      id: string,
      query?: {
        /** Start time (ISO 8601), defaults to 1 hour ago */
        startTime?: string;
        /** End time (ISO 8601), defaults to now */
        endTime?: string;
        /** Time granularity: minute, hourly, daily (default: minute) */
        granularity?: string;
        /** Filter by tag (e.g. agent name like 'shopify_graphql') */
        tag?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<RateLimitTimelineResponse, ErrorResponse>({
        path: `/api/runtime/connections/${id}/rate-limit-timeline`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description This endpoint provides immediate execution results without creating database records or checkpoints. Ideal for low-latency use cases. Accepts ANY HTTP method (GET, POST, PUT, DELETE, PATCH, etc.) Request data (method, URI, headers, body) is forwarded to the workflow as inputs. Always executes the latest version of the workflow. # Performance - First execution: ~50-100ms overhead + execution time - Cached executions: ~5-10ms overhead + execution time - Hard timeout: 30 seconds # Limitations - No execution history in database - No checkpoint/replay support - Not suitable for long-running workflows
     *
     * @tags Event Capture
     * @name CaptureHttpEventSync
     * @summary Execute a workflow synchronously with minimal latency
     * @request POST:/api/runtime/events/http-sync/{workflow_id}
     */
    captureHttpEventSync: (
      workflowId: string,
      data: string,
      params: RequestParams = {},
    ) =>
      this.request<any, any>({
        path: `/api/runtime/events/http-sync/${workflowId}`,
        method: "POST",
        body: data,
        format: "json",
        ...params,
      }),

    /**
     * @description When a trigger is found for the given trigger_id: 1. Looks up the trigger in invocation_trigger table 2. Validates trigger is active 3. Publishes a TriggerEvent to the trigger stream for async execution 4. Returns instance_id for tracking Returns 404 if trigger is not found. Accepts ANY HTTP method (GET, POST, PUT, DELETE, PATCH, etc.) Body is optional and can be any content type including multipart/form-data
     *
     * @tags Event Capture
     * @name CaptureHttpEvent
     * @summary HTTP trigger execution endpoint
     * @request POST:/api/runtime/events/http/{trigger_id}/{action}
     */
    captureHttpEvent: (
      triggerId: string,
      action: string,
      data: string,
      params: RequestParams = {},
    ) =>
      this.request<void, void>({
        path: `/api/runtime/events/http/${triggerId}/${action}`,
        method: "POST",
        body: data,
        ...params,
      }),

    /**
     * No description
     *
     * @tags executions-controller
     * @name ListAllExecutionsHandler
     * @summary List all executions across all workflows with filtering, sorting, and pagination
     * @request GET:/api/runtime/executions
     */
    listAllExecutionsHandler: (
      query?: {
        /**
         * Page number (0-based, default: 0)
         * @format int32
         */
        page?: number;
        /**
         * Page size (default: 20, max: 100)
         * @format int32
         */
        size?: number;
        /** Filter by workflow ID */
        workflowId?: string;
        /** Filter by status (comma-separated, lowercase: queued,completed,failed,running,compiling,timeout,cancelled) */
        status?: string;
        /**
         * Filter by created date - from (inclusive, ISO 8601)
         * @format date-time
         */
        createdFrom?: string;
        /**
         * Filter by created date - to (inclusive, ISO 8601)
         * @format date-time
         */
        createdTo?: string;
        /**
         * Filter by completed date - from (inclusive, ISO 8601)
         * @format date-time
         */
        completedFrom?: string;
        /**
         * Filter by completed date - to (inclusive, ISO 8601)
         * @format date-time
         */
        completedTo?: string;
        /** Sort by field (default: completedAt). Options: createdAt, completedAt, status, workflowId */
        sortBy?: string;
        /** Sort order (default: desc). Options: asc, desc */
        sortOrder?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListAllExecutionsResponse, any>({
        path: `/api/runtime/executions`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name ListBuckets
     * @summary List all buckets
     * @request GET:/api/runtime/files/buckets
     * @secure
     */
    listBuckets: (
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListBucketsResponse, any>({
        path: `/api/runtime/files/buckets`,
        method: "GET",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name CreateBucket
     * @summary Create a bucket
     * @request POST:/api/runtime/files/buckets
     * @secure
     */
    createBucket: (
      data: CreateBucketRequest,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<CreateBucketResponse, any>({
        path: `/api/runtime/files/buckets`,
        method: "POST",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name DeleteBucket
     * @summary Delete a bucket (must be empty)
     * @request DELETE:/api/runtime/files/buckets/{bucket}
     * @secure
     */
    deleteBucket: (
      bucket: string,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<DeleteResponse, any>({
        path: `/api/runtime/files/buckets/${bucket}`,
        method: "DELETE",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name ListObjects
     * @summary List files in a bucket
     * @request GET:/api/runtime/files/{bucket}
     * @secure
     */
    listObjects: (
      bucket: string,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
        /** Filter by key prefix */
        prefix?: string;
        /**
         * Max results (default: 1000)
         * @format int32
         * @min 0
         */
        maxKeys?: number;
        /** Pagination token */
        continuationToken?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListObjectsResponse, any>({
        path: `/api/runtime/files/${bucket}`,
        method: "GET",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Upload a file to the specified bucket. Send as multipart/form-data with a `file` field. Optionally include a `key` field to specify the object key; defaults to the filename.
     *
     * @tags file-storage
     * @name UploadObject
     * @summary Upload a file (multipart/form-data)
     * @request POST:/api/runtime/files/{bucket}
     * @secure
     */
    uploadObject: (
      bucket: string,
      data: any,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<UploadResponse, void>({
        path: `/api/runtime/files/${bucket}`,
        method: "POST",
        query: query,
        body: data,
        secure: true,
        type: ContentType.FormData,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name DownloadObject
     * @summary Download a file
     * @request GET:/api/runtime/files/{bucket}/{key}
     * @secure
     */
    downloadObject: (
      bucket: string,
      key: string,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<void, void>({
        path: `/api/runtime/files/${bucket}/${key}`,
        method: "GET",
        query: query,
        secure: true,
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name DeleteObject
     * @summary Delete a file
     * @request DELETE:/api/runtime/files/{bucket}/{key}
     * @secure
     */
    deleteObject: (
      bucket: string,
      key: string,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<DeleteResponse, any>({
        path: `/api/runtime/files/${bucket}/${key}`,
        method: "DELETE",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags file-storage
     * @name GetObjectInfo
     * @summary Get file metadata (HEAD)
     * @request GET:/api/runtime/files/{bucket}/{key}/info
     * @secure
     */
    getObjectInfo: (
      bucket: string,
      key: string,
      query?: {
        /** Optional s3_compatible connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<FileMetadataResponse, void>({
        path: `/api/runtime/files/${bucket}/${key}/info`,
        method: "GET",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-step-type-api
     * @name GetWorkflowStepTypesHandler
     * @summary Get all available workflow step types
     * @request GET:/api/runtime/metadata/workflow/step-types
     */
    getWorkflowStepTypesHandler: (params: RequestParams = {}) =>
      this.request<void, any>({
        path: `/api/runtime/metadata/workflow/step-types`,
        method: "GET",
        ...params,
      }),

    /**
     * No description
     *
     * @tags metrics-controller
     * @name GetTenantMetrics
     * @summary Get tenant-level metrics aggregated across all workflows (hourly)
     * @request GET:/api/runtime/metrics/tenant
     */
    getTenantMetrics: (
      query?: {
        /** Start time (ISO 8601), defaults to 24 hours ago */
        startTime?: string;
        /** End time (ISO 8601), defaults to now */
        endTime?: string;
        /** Time granularity: 'hourly' or 'daily' (default: hourly) */
        granularity?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<TenantMetricsResponse, MetricsResponse>({
        path: `/api/runtime/metrics/tenant`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags metrics-controller
     * @name GetWorkflowMetrics
     * @summary Get metrics for a specific workflow
     * @request GET:/api/runtime/metrics/workflows/{workflow_id}
     */
    getWorkflowMetrics: (
      workflowId: string,
      query?: {
        /** Start time (ISO 8601), defaults to 24h ago */
        startTime?: string;
        /** End time (ISO 8601), defaults to now */
        endTime?: string;
        /**
         * Specific version, or all versions if not specified
         * @format int32
         */
        version?: number;
        /** Time granularity: 'hourly' or 'daily' (default: daily) */
        granularity?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<WorkflowMetricsHourlyResponse, MetricsResponse>({
        path: `/api/runtime/metrics/workflows/${workflowId}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags metrics-controller
     * @name GetWorkflowStats
     * @summary Get overall statistics for a workflow (all time)
     * @request GET:/api/runtime/metrics/workflows/{workflow_id}/stats
     */
    getWorkflowStats: (
      workflowId: string,
      query?: {
        /**
         * Specific version, or all versions if not specified
         * @format int32
         */
        version?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<WorkflowStatsResponse, MetricsResponse>({
        path: `/api/runtime/metrics/workflows/${workflowId}/stats`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates a new instance of an object with type-validated properties. All values are validated against the schema definition including type checking, nullable constraints, and enum values. **Type Requirements:** - `string` - Provide string value - `integer` - Provide integer value (JavaScript number without decimals) - `decimal` - Provide number value (JavaScript number with decimals) - `boolean` - Provide boolean value (true/false) - `timestamp` - Provide ISO 8601 string (e.g., "2025-01-15T10:00:00Z") - `json` - Provide any JSON value (object, array, string, number, boolean, null) - `enum` - Provide string matching one of the allowed values
     *
     * @tags object-model
     * @name CreateInstance
     * @summary Create a new instance
     * @request POST:/api/runtime/object-model/instances
     * @secure
     */
    createInstance: (
      data: CreateInstanceRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<CreateInstanceResponse, any>({
        path: `/api/runtime/object-model/instances`,
        method: "POST",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name GetInstancesBySchemaName
     * @summary Get instances by schema name
     * @request GET:/api/runtime/object-model/instances/schema/name/{schema_name}
     */
    getInstancesBySchemaName: (
      schemaName: string,
      query?: {
        /**
         * Pagination offset (default: 0)
         * @format int64
         */
        offset?: number;
        /**
         * Pagination limit (default: 100)
         * @format int64
         */
        limit?: number;
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListInstancesResponse, any>({
        path: `/api/runtime/object-model/instances/schema/name/${schemaName}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name AggregateInstances
     * @summary Aggregate instances with GROUP BY for a specific schema.
     * @request POST:/api/runtime/object-model/instances/schema/{name}/aggregate
     */
    aggregateInstances: (
      name: string,
      data: AggregateRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<AggregateResponse, any>({
        path: `/api/runtime/object-model/instances/schema/${name}/aggregate`,
        method: "POST",
        query: query,
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Exports filtered and sorted instances from a schema as a CSV file. Supports column selection and all existing filter/sort capabilities.
     *
     * @tags api::handlers::csv_import_export
     * @name ExportCsv
     * @summary Export instances as CSV
     * @request POST:/api/runtime/object-model/instances/schema/{name}/export-csv
     */
    exportCsv: (
      name: string,
      data: CsvExportRequest,
      query?: {
        /** Optional connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<void, void>({
        path: `/api/runtime/object-model/instances/schema/${name}/export-csv`,
        method: "POST",
        query: query,
        body: data,
        type: ContentType.Json,
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name FilterInstances
     * @summary Filter instances with condition-based queries for a specific schema
     * @request POST:/api/runtime/object-model/instances/schema/{name}/filter
     */
    filterInstances: (
      name: string,
      data: FilterRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<FilterInstancesResponse, any>({
        path: `/api/runtime/object-model/instances/schema/${name}/filter`,
        method: "POST",
        query: query,
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Imports CSV data into a schema with column mapping. Supports create (insert) and upsert modes. Accepts multipart/form-data or JSON with base64. Atomic: all rows validated first, none imported if any fail.
     *
     * @tags api::handlers::csv_import_export
     * @name ImportCsv
     * @summary Import CSV data
     * @request POST:/api/runtime/object-model/instances/schema/{name}/import-csv
     */
    importCsv: (
      name: string,
      query?: {
        /** Optional connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<CsvImportResponse, void>({
        path: `/api/runtime/object-model/instances/schema/${name}/import-csv`,
        method: "POST",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Parses CSV headers and sample rows, returns schema columns and auto-suggested column mappings. Accepts multipart/form-data or JSON with base64.
     *
     * @tags api::handlers::csv_import_export
     * @name ImportCsvPreview
     * @summary Preview CSV import
     * @request POST:/api/runtime/object-model/instances/schema/{name}/import-csv/preview
     */
    importCsvPreview: (
      name: string,
      query?: {
        /** Optional connection ID */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ImportPreviewResponse, void>({
        path: `/api/runtime/object-model/instances/schema/${name}/import-csv/preview`,
        method: "POST",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name GetInstancesBySchema
     * @summary Get instances by schema ID
     * @request GET:/api/runtime/object-model/instances/schema/{schema_id}
     */
    getInstancesBySchema: (
      schemaId: string,
      query?: {
        /**
         * Pagination offset (default: 0)
         * @format int64
         */
        offset?: number;
        /**
         * Pagination limit (default: 100)
         * @format int64
         */
        limit?: number;
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListInstancesResponse, any>({
        path: `/api/runtime/object-model/instances/schema/${schemaId}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates multiple instances in a single transaction. If any validation fails, no rows are inserted.
     *
     * @tags object-model
     * @name BulkCreateInstances
     * @summary Bulk create instances
     * @request POST:/api/runtime/object-model/instances/{schema_id}/bulk
     * @secure
     */
    bulkCreateInstances: (
      schemaId: string,
      data: BulkCreateRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<BulkCreateResponse, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/bulk`,
        method: "POST",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Soft deletes multiple instances in a single operation.
     *
     * @tags object-model
     * @name BulkDeleteInstances
     * @summary Bulk delete instances
     * @request DELETE:/api/runtime/object-model/instances/{schema_id}/bulk
     * @secure
     */
    bulkDeleteInstances: (
      schemaId: string,
      data: BulkDeleteRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<BulkDeleteResponse, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/bulk`,
        method: "DELETE",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Updates multiple instances in a single transaction. Supports two modes: `byCondition` applies the same properties to every row matching the condition; `byIds` applies per-row properties to each listed id.
     *
     * @tags object-model
     * @name BulkUpdateInstances
     * @summary Bulk update instances
     * @request PATCH:/api/runtime/object-model/instances/{schema_id}/bulk
     * @secure
     */
    bulkUpdateInstances: (
      schemaId: string,
      data: BulkUpdateRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<BulkUpdateResponse, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/bulk`,
        method: "PATCH",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Retrieves a specific instance by its ID. Requires the schema ID to locate the correct table.
     *
     * @tags object-model
     * @name GetInstanceById
     * @summary Get a single instance by ID
     * @request GET:/api/runtime/object-model/instances/{schema_id}/{instance_id}
     * @secure
     */
    getInstanceById: (
      schemaId: string,
      instanceId: string,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<GetInstanceResponse, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/${instanceId}`,
        method: "GET",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * @description Updates an existing instance with type-validated properties. Only provided fields are updated.
     *
     * @tags object-model
     * @name UpdateInstance
     * @summary Update an instance
     * @request PUT:/api/runtime/object-model/instances/{schema_id}/{instance_id}
     * @secure
     */
    updateInstance: (
      schemaId: string,
      instanceId: string,
      data: UpdateInstanceRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<UpdateInstanceResponse, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/${instanceId}`,
        method: "PUT",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Soft deletes an instance (sets deleted flag to true). The instance can be recovered.
     *
     * @tags object-model
     * @name DeleteInstance
     * @summary Delete an instance
     * @request DELETE:/api/runtime/object-model/instances/{schema_id}/{instance_id}
     * @secure
     */
    deleteInstance: (
      schemaId: string,
      instanceId: string,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<any, any>({
        path: `/api/runtime/object-model/instances/${schemaId}/${instanceId}`,
        method: "DELETE",
        query: query,
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name ListSchemas
     * @summary List all schemas with pagination
     * @request GET:/api/runtime/object-model/schemas
     */
    listSchemas: (
      query?: {
        /**
         * Pagination offset (default: 0)
         * @format int64
         */
        offset?: number;
        /**
         * Pagination limit (default: 100)
         * @format int64
         */
        limit?: number;
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListSchemasResponse, any>({
        path: `/api/runtime/object-model/schemas`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Creates a new object schema with typed columns and indexes. Each schema generates a dedicated PostgreSQL table in the object model database with automatic tenant isolation. **Supported Column Types:** - `string` - Unlimited text (TEXT) - `integer` - 64-bit integer (BIGINT) - `decimal` - Fixed-point decimal with precision/scale (NUMERIC) - `boolean` - True/false (BOOLEAN) - `timestamp` - UTC timestamp (TIMESTAMP WITH TIME ZONE) - `json` - Binary JSON (JSONB) - `enum` - String with allowed values (TEXT with CHECK constraint) **Auto-managed Columns:** Every table automatically includes: id, created_at, updated_at, deleted
     *
     * @tags object-model
     * @name CreateSchema
     * @summary Create a new schema
     * @request POST:/api/runtime/object-model/schemas
     * @secure
     */
    createSchema: (
      data: CreateSchemaRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<CreateSchemaResponse, any>({
        path: `/api/runtime/object-model/schemas`,
        method: "POST",
        query: query,
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name GetSchemaByName
     * @summary Get a schema by name
     * @request GET:/api/runtime/object-model/schemas/name/{name}
     */
    getSchemaByName: (
      name: string,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<GetSchemaResponse, any>({
        path: `/api/runtime/object-model/schemas/name/${name}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name GetSchemaById
     * @summary Get a schema by ID
     * @request GET:/api/runtime/object-model/schemas/{id}
     */
    getSchemaById: (
      id: string,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<GetSchemaResponse, any>({
        path: `/api/runtime/object-model/schemas/${id}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name UpdateSchema
     * @summary Update a schema
     * @request PUT:/api/runtime/object-model/schemas/{id}
     */
    updateSchema: (
      id: string,
      data: UpdateSchemaRequest,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<UpdateSchemaResponse, any>({
        path: `/api/runtime/object-model/schemas/${id}`,
        method: "PUT",
        query: query,
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags object-model
     * @name DeleteSchema
     * @summary Delete a schema
     * @request DELETE:/api/runtime/object-model/schemas/{id}
     */
    deleteSchema: (
      id: string,
      query?: {
        /** Optional connection ID for database selection */
        connectionId?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<any, any>({
        path: `/api/runtime/object-model/schemas/${id}`,
        method: "DELETE",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Returns real-time rate limit state from Redis combined with configuration from PostgreSQL for all connections. Optionally includes aggregated period stats based on the interval parameter.
     *
     * @tags rate-limits-controller
     * @name ListRateLimitsHandler
     * @summary List rate limit status for all tenant connections
     * @request GET:/api/runtime/rate-limits
     */
    listRateLimitsHandler: (
      query?: {
        /** Time interval for aggregated stats: 1h, 24h, 7d, 30d (default: 24h) */
        interval?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListRateLimitsResponse, ErrorResponse>({
        path: `/api/runtime/rate-limits`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the OpenAPI 3.1 specification for all agents, matching the exact format returned by the agent API endpoints.
     *
     * @tags Specifications
     * @name GetAgentsSpec
     * @summary Get the agent OpenAPI specification
     * @request GET:/api/runtime/specs/agents
     */
    getAgentsSpec: (params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/agents`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Specifications
     * @name GetAgentsChangelog
     * @summary Get the agent changelog
     * @request GET:/api/runtime/specs/agents/changelog
     */
    getAgentsChangelog: (params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/agents/changelog`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Currently only the embedded version is available.
     *
     * @tags Specifications
     * @name GetAgentsSpecVersion
     * @summary Get a specific version of the agent spec
     * @request GET:/api/runtime/specs/agents/{version}
     */
    getAgentsSpecVersion: (version: string, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/agents/${version}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the JSON Schema for the core DSL structure including: - Step types (7 types after GroupBy removal) - Execution graph format - Data mapping DSL
     *
     * @tags Specifications
     * @name GetDslSpec
     * @summary Get the current DSL specification
     * @request GET:/api/runtime/specs/dsl
     */
    getDslSpec: (params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/dsl`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Specifications
     * @name GetDslChangelog
     * @summary Get the DSL changelog
     * @request GET:/api/runtime/specs/dsl/changelog
     */
    getDslChangelog: (params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/dsl/changelog`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns a list of all available step types with full JSON Schema for each. This is generated dynamically from the inventory-registered step metadata.
     *
     * @tags Specifications
     * @name ListStepTypes
     * @summary List all step types with their schemas
     * @request GET:/api/runtime/specs/dsl/steps
     */
    listStepTypes: (params: RequestParams = {}) =>
      this.request<void, any>({
        path: `/api/runtime/specs/dsl/steps`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the full JSON Schema for the specified step type.
     *
     * @tags Specifications
     * @name GetStepTypeSchema
     * @summary Get schema for a specific step type
     * @request GET:/api/runtime/specs/dsl/steps/{stepType}
     */
    getStepTypeSchema: (stepType: string, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/dsl/steps/${stepType}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Currently only the embedded version is available.
     *
     * @tags Specifications
     * @name GetDslSpecVersion
     * @summary Get a specific version of the DSL spec
     * @request GET:/api/runtime/specs/dsl/{version}
     */
    getDslSpecVersion: (version: string, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/specs/dsl/${version}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Specifications
     * @name GetSpecVersions
     * @summary Get all available spec versions
     * @request GET:/api/runtime/specs/versions
     */
    getSpecVersions: (params: RequestParams = {}) =>
      this.request<void, any>({
        path: `/api/runtime/specs/versions`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns hardcoded metadata about available step types. No database or external dependencies - just static data.
     *
     * @tags workflow-controller
     * @name ListStepTypesHandler
     * @summary List all supported step types
     * @request GET:/api/runtime/steps
     */
    listStepTypesHandler: (params: RequestParams = {}) =>
      this.request<ListStepTypesResponse, void>({
        path: `/api/runtime/steps`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Invocation Triggers
     * @name ListInvocationTriggers
     * @summary List all invocation triggers
     * @request GET:/api/runtime/triggers
     */
    listInvocationTriggers: (params: RequestParams = {}) =>
      this.request<ApiResponseVecInvocationTrigger, void>({
        path: `/api/runtime/triggers`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Invocation Triggers
     * @name CreateInvocationTrigger
     * @summary Create a new invocation trigger
     * @request POST:/api/runtime/triggers
     */
    createInvocationTrigger: (
      data: CreateInvocationTriggerRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseInvocationTrigger, void>({
        path: `/api/runtime/triggers`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Invocation Triggers
     * @name GetInvocationTrigger
     * @summary Get a single invocation trigger by ID
     * @request GET:/api/runtime/triggers/{id}
     */
    getInvocationTrigger: (id: string, params: RequestParams = {}) =>
      this.request<ApiResponseInvocationTrigger, void>({
        path: `/api/runtime/triggers/${id}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Invocation Triggers
     * @name UpdateInvocationTrigger
     * @summary Update an invocation trigger by ID
     * @request PUT:/api/runtime/triggers/{id}
     */
    updateInvocationTrigger: (
      id: string,
      data: UpdateInvocationTriggerRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseInvocationTrigger, void>({
        path: `/api/runtime/triggers/${id}`,
        method: "PUT",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Invocation Triggers
     * @name DeleteInvocationTrigger
     * @summary Delete an invocation trigger by ID
     * @request DELETE:/api/runtime/triggers/{id}
     */
    deleteInvocationTrigger: (id: string, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/triggers/${id}`,
        method: "DELETE",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ListWorkflowsHandler
     * @summary List all workflows for a tenant with pagination and optional folder filtering
     * @request GET:/api/runtime/workflows
     */
    listWorkflowsHandler: (
      query: {
        /**
         * Page number (1-based, default: 1)
         * @format int32
         */
        page?: number;
        /**
         * Page size (default: 20, max: 100)
         * @format int32
         */
        pageSize?: number;
        /** Filter by folder path (e.g., '/Sales/'). If not provided, returns all workflows. */
        path?: string;
        /** If true and path is provided, includes workflows in subfolders (default: false) */
        recursive: boolean;
        /** Search workflows by name (case-insensitive substring match) */
        search?: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<ApiResponsePageWorkflowDto, any>({
        path: `/api/runtime/workflows`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name CreateWorkflowHandler
     * @summary Create a new workflow with auto-generated ID
     * @request POST:/api/runtime/workflows/create
     */
    createWorkflowHandler: (
      data: CreateWorkflowRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseWorkflowDto, any>({
        path: `/api/runtime/workflows/create`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ListFoldersHandler
     * @summary List all folders (distinct paths) for a tenant
     * @request GET:/api/runtime/workflows/folders
     */
    listFoldersHandler: (params: RequestParams = {}) =>
      this.request<FoldersResponse, any>({
        path: `/api/runtime/workflows/folders`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name RenameFolderHandler
     * @summary Rename a folder (updates all workflows with matching path prefix)
     * @request PUT:/api/runtime/workflows/folders/rename
     */
    renameFolderHandler: (
      data: RenameFolderRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseRenameFolderResponse, any>({
        path: `/api/runtime/workflows/folders/rename`,
        method: "PUT",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description Pure validation handler - no database or external dependencies. Validates the execution graph using runtara-workflows validation.
     *
     * @tags workflow-controller
     * @name ValidateGraphHandler
     * @summary Validate graph structure
     * @request POST:/api/runtime/workflows/graph/validate
     */
    validateGraphHandler: (data: any, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/workflows/graph/validate`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name GetExecutionMetricsHandler
     * @summary Get execution results for a workflow instance
     * @request GET:/api/runtime/workflows/instances/{instance_id}
     */
    getExecutionMetricsHandler: (
      instanceId: string,
      params: RequestParams = {},
    ) =>
      this.request<WorkflowInstanceDto, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * @description Sends a pause signal to the instance. The instance will checkpoint its state and suspend execution until resumed.
     *
     * @tags workflow-controller
     * @name PauseInstanceHandler
     * @summary Pause a running workflow instance
     * @request POST:/api/runtime/workflows/instances/{instance_id}/pause
     */
    pauseInstanceHandler: (instanceId: string, params: RequestParams = {}) =>
      this.request<any, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}/pause`,
        method: "POST",
        format: "json",
        ...params,
      }),

    /**
     * @tags workflow-controller
     * @name ReplayInstanceHandler
     * @summary Replay a workflow instance with the same inputs
     * @request POST:/api/runtime/workflows/instances/{instance_id}/replay
     */
    replayInstanceHandler: (instanceId: string, params: RequestParams = {}) =>
      this.request<any, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}/replay`,
        method: "POST",
        ...params,
      }),

    /**
     * @description Sends a resume signal to the instance. The instance will resume execution from its last checkpoint.
     *
     * @tags workflow-controller
     * @name ResumeInstanceHandler
     * @summary Resume a paused workflow instance
     * @request POST:/api/runtime/workflows/instances/{instance_id}/resume
     */
    resumeInstanceHandler: (instanceId: string, params: RequestParams = {}) =>
      this.request<any, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}/resume`,
        method: "POST",
        format: "json",
        ...params,
      }),

    /**
     * @description Note: This endpoint is currently not implemented as execution event data is stored in runtara-environment and requires querying the environment.
     *
     * @tags workflow-controller
     * @name GetStepSubinstancesHandler
     * @summary Get step subinstances (execution events) for a specific step
     * @request GET:/api/runtime/workflows/instances/{instance_id}/steps/{step_id}/subinstances
     */
    getStepSubinstancesHandler: (
      instanceId: string,
      stepId: string,
      params: RequestParams = {},
    ) =>
      this.request<any, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}/steps/${stepId}/subinstances`,
        method: "GET",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name StopInstanceHandler
     * @summary Stop a running workflow instance
     * @request POST:/api/runtime/workflows/instances/{instance_id}/stop
     */
    stopInstanceHandler: (instanceId: string, params: RequestParams = {}) =>
      this.request<any, ErrorResponse>({
        path: `/api/runtime/workflows/instances/${instanceId}/stop`,
        method: "POST",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name GetWorkflowHandler
     * @summary Get a specific workflow by ID and optional version
     * @request GET:/api/runtime/workflows/{id}
     */
    getWorkflowHandler: (
      id: string,
      query?: {
        /**
         * Version number (defaults to latest)
         * @format int32
         */
        versionNumber?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseWorkflowDto, any>({
        path: `/api/runtime/workflows/${id}`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description The workflow executes asynchronously while this endpoint streams execution events (tool calls, LLM responses, memory operations, pending input requests) as Server-Sent Events. For workflows with WaitForSignal steps (human-in-the-loop), the stream emits a `waiting_for_input` event with a `signal_id`. Use `POST /api/runtime/signals/{instanceId}` to submit the response and resume execution.
     *
     * @tags Chat
     * @name ChatHandler
     * @summary Start a chat session with an initial message and stream events via SSE.
     * @request POST:/api/runtime/workflows/{id}/chat
     */
    chatHandler: (id: string, data: ChatRequest, params: RequestParams = {}) =>
      this.request<void, void>({
        path: `/api/runtime/workflows/${id}/chat`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        ...params,
      }),

    /**
     * @description Use this when the workflow doesn't require an initial user message to begin (e.g., the AI agent starts the conversation proactively).
     *
     * @tags Chat
     * @name ChatStartHandler
     * @summary Start a chat session without an initial message and stream events via SSE.
     * @request POST:/api/runtime/workflows/{id}/chat/start
     */
    chatStartHandler: (
      id: string,
      data: ChatStartRequest,
      params: RequestParams = {},
    ) =>
      this.request<void, void>({
        path: `/api/runtime/workflows/${id}/chat/start`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name CloneWorkflowHandler
     * @summary Clone a workflow with all its versions
     * @request POST:/api/runtime/workflows/{id}/clone
     */
    cloneWorkflowHandler: (
      id: string,
      data: CloneWorkflowRequest,
      params: RequestParams = {},
    ) =>
      this.request<any, any>({
        path: `/api/runtime/workflows/${id}/clone`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name DeleteWorkflowHandler
     * @summary Delete a workflow and all its versions (soft delete)
     * @request POST:/api/runtime/workflows/{id}/delete
     */
    deleteWorkflowHandler: (id: string, params: RequestParams = {}) =>
      this.request<any, any>({
        path: `/api/runtime/workflows/${id}/delete`,
        method: "POST",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ExecuteWorkflowHandler
     * @summary Execute a workflow by scheduling it with inputs (defaults to active version)
     * @request POST:/api/runtime/workflows/{id}/execute
     */
    executeWorkflowHandler: (
      id: string,
      data: ExecuteWorkflowRequest,
      query?: {
        /**
         * Specific version to execute (defaults to current)
         * @format int32
         */
        version?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<ExecuteWorkflowResponse, ErrorResponse>({
        path: `/api/runtime/workflows/${id}/execute`,
        method: "POST",
        query: query,
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name MoveWorkflowHandler
     * @summary Move a workflow to a different folder
     * @request PUT:/api/runtime/workflows/{id}/move
     */
    moveWorkflowHandler: (
      id: string,
      data: MoveWorkflowRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseMoveWorkflowResponse, any>({
        path: `/api/runtime/workflows/${id}/move`,
        method: "PUT",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ScheduleWorkflowHandler
     * @summary Schedule a workflow execution (placeholder - not implemented)
     * @request POST:/api/runtime/workflows/{id}/schedule
     */
    scheduleWorkflowHandler: (
      id: string,
      data: any,
      params: RequestParams = {},
    ) =>
      this.request<any, any>({
        path: `/api/runtime/workflows/${id}/schedule`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name UpdateWorkflowHandler
     * @summary Update a workflow by creating a new version
     * @request POST:/api/runtime/workflows/{id}/update
     */
    updateWorkflowHandler: (
      id: string,
      data: UpdateWorkflowRequest,
      params: RequestParams = {},
    ) =>
      this.request<any, WorkflowValidationErrorResponse>({
        path: `/api/runtime/workflows/${id}/update`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ValidateMappingsHandler
     * @summary Validate workflow mappings without full compilation Returns validation issues (errors and warnings) for reference paths, types, and connections
     * @request POST:/api/runtime/workflows/{id}/validate-mappings
     */
    validateMappingsHandler: (
      id: string,
      query?: {
        /**
         * Version number (defaults to latest)
         * @format int32
         */
        versionNumber?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<ValidateMappingsResponse, ErrorResponse>({
        path: `/api/runtime/workflows/${id}/validate-mappings`,
        method: "POST",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ListWorkflowVersionsHandler
     * @summary Get all versions of a specific workflow
     * @request GET:/api/runtime/workflows/{id}/versions
     */
    listWorkflowVersionsHandler: (id: string, params: RequestParams = {}) =>
      this.request<ApiResponseVecWorkflowVersionInfoDto, any>({
        path: `/api/runtime/workflows/${id}/versions`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name CompileWorkflowHandler
     * @summary Compile a specific workflow by tenant ID, workflow ID, and version
     * @request POST:/api/runtime/workflows/{id}/versions/{version}/compile
     */
    compileWorkflowHandler: (
      workflowId: string,
      version: string,
      id: string,
      params: RequestParams = {},
    ) =>
      this.request<CompileWorkflowResponse, ErrorResponse>({
        path: `/api/runtime/workflows/${id}/versions/${version}/compile`,
        method: "POST",
        format: "json",
        ...params,
      }),

    /**
     * @description Returns the input schema, output schema, and variables from the execution graph of a specific workflow version.
     *
     * @tags workflow-controller
     * @name GetVersionSchemasHandler
     * @summary Get schemas for a specific workflow version
     * @request GET:/api/runtime/workflows/{id}/versions/{version}/schemas
     */
    getVersionSchemasHandler: (
      id: string,
      version: number,
      params: RequestParams = {},
    ) =>
      this.request<VersionSchemasResponse, void>({
        path: `/api/runtime/workflows/${id}/versions/${version}/schemas`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ToggleTrackEventsHandler
     * @summary Toggle step-event tracking for a specific workflow version
     * @request PUT:/api/runtime/workflows/{id}/versions/{version}/track-events
     */
    toggleTrackEventsHandler: (
      id: string,
      version: number,
      data: UpdateTrackEventsRequest,
      params: RequestParams = {},
    ) =>
      this.request<ApiResponseWorkflowDto, any>({
        path: `/api/runtime/workflows/${id}/versions/${version}/track-events`,
        method: "PUT",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * @description GET /api/runtime/workflows/{workflow_id}/instances/{instance_id}/step-events Retrieves debug step events from runtara-environment. The workflow must be compiled with track_events enabled for events to be recorded.
     *
     * @tags workflow-controller
     * @name GetStepEvents
     * @summary Handler to get step events for a workflow execution
     * @request GET:/api/runtime/workflows/{workflowId}/instances/{instanceId}/step-events
     */
    getStepEvents: (
      workflowId: string,
      instanceId: string,
      query?: {
        /** Filter by event type (e.g., "custom", "started", "completed") */
        eventType?: string | null;
        /** Filter by subtype (e.g., "step_debug_start", "step_debug_end", "workflow_log") */
        subtype?: string | null;
        /**
         * Limit number of results (default: 100, max: 1000)
         * @format int32
         * @min 0
         */
        limit?: number | null;
        /**
         * Pagination offset
         * @format int32
         * @min 0
         */
        offset?: number | null;
        /**
         * Filter events created after this timestamp (ISO 8601 format)
         * @format date-time
         */
        createdAfter?: string | null;
        /**
         * Filter events created before this timestamp (ISO 8601 format)
         * @format date-time
         */
        createdBefore?: string | null;
        /** Full-text search in event payload JSON */
        payloadContains?: string | null;
        /** Filter events by scope ID (for hierarchical step events in Split/While/EmbedWorkflow) */
        scopeId?: string | null;
        /** Filter events by parent scope ID (use "null" for root-level events) */
        parentScopeId?: string | null;
        /** When true, only return events from root scopes (no parent scope) */
        rootScopesOnly?: boolean | null;
        /** Sort order for results: "asc" (oldest first) or "desc" (newest first, default) */
        sortOrder?: string | null;
      },
      params: RequestParams = {},
    ) =>
      this.request<StepEventsResponse, any>({
        path: `/api/runtime/workflows/${workflowId}/instances/${instanceId}/step-events`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description GET /api/runtime/workflows/{workflow_id}/instances/{instance_id}/steps Returns unified step records with paired start/end events. Each step appears once with its complete lifecycle information (inputs, outputs, duration, status).
     *
     * @tags workflow-controller
     * @name GetStepSummaries
     * @summary Handler to get step summaries for a workflow execution
     * @request GET:/api/runtime/workflows/{workflowId}/instances/{instanceId}/steps
     */
    getStepSummaries: (
      workflowId: string,
      instanceId: string,
      query?: {
        /**
         * Limit number of results (default: 100, max: 1000)
         * @format int32
         * @min 0
         */
        limit?: number | null;
        /**
         * Pagination offset
         * @format int32
         * @min 0
         */
        offset?: number | null;
        /** Sort order: "asc" (oldest first) or "desc" (newest first, default) */
        sortOrder?: string | null;
        /** Filter by status: "running", "completed", or "failed" */
        status?: string | null;
        /** Filter by step type (e.g., "Http", "Transform", "Agent") */
        stepType?: string | null;
        /** Filter by scope ID (for hierarchical steps in Split/While/EmbedWorkflow) */
        scopeId?: string | null;
        /** Filter by parent scope ID */
        parentScopeId?: string | null;
        /** When true, only return steps from root scopes (no parent) */
        rootScopesOnly?: boolean | null;
      },
      params: RequestParams = {},
    ) =>
      this.request<StepSummariesResponse, any>({
        path: `/api/runtime/workflows/${workflowId}/instances/${instanceId}/steps`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ListInstancesHandler
     * @summary List all workflow instances for a given tenant and workflow
     * @request GET:/api/runtime/workflows/{workflow_id}/instances
     */
    listInstancesHandler: (
      workflowId: string,
      query?: {
        /**
         * Page number (default: 0)
         * @format int32
         */
        page?: number;
        /**
         * Page size (default: 10)
         * @format int32
         */
        size?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<PageWorkflowInstanceHistoryDto, ErrorResponse>({
        path: `/api/runtime/workflows/${workflowId}/instances`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name GetInstanceHandler
     * @summary Get a workflow instance by workflow_id and instance_id with all available data
     * @request GET:/api/runtime/workflows/{workflow_id}/instances/{instance_id}
     */
    getInstanceHandler: (
      workflowId: string,
      instanceId: string,
      params: RequestParams = {},
    ) =>
      this.request<WorkflowInstanceDto, ErrorResponse>({
        path: `/api/runtime/workflows/${workflowId}/instances/${instanceId}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags workflow-controller
     * @name ListInstanceCheckpointsHandler
     * @summary List checkpoints for a workflow instance via runtara management SDK
     * @request GET:/api/runtime/workflows/{workflow_id}/instances/{instance_id}/checkpoints
     */
    listInstanceCheckpointsHandler: (
      workflowId: string,
      instanceId: string,
      query?: {
        /**
         * Page number (default: 0)
         * @format int32
         */
        page?: number;
        /**
         * Page size (default: 20, max: 100)
         * @format int32
         */
        size?: number;
      },
      params: RequestParams = {},
    ) =>
      this.request<ListCheckpointsResponse, ErrorResponse>({
        path: `/api/runtime/workflows/${workflowId}/instances/${instanceId}/checkpoints`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * @description Updates which version is marked as "current" for execution. Note: Requires database migration to add current_version column.
     *
     * @tags workflow-controller
     * @name SetCurrentVersionHandler
     * @summary Set the current version for a workflow
     * @request POST:/api/runtime/workflows/{workflow_id}/versions/{version_number}/set-current
     */
    setCurrentVersionHandler: (
      workflowId: string,
      versionNumber: number,
      params: RequestParams = {},
    ) =>
      this.request<void, ErrorResponse>({
        path: `/api/runtime/workflows/${workflowId}/versions/${versionNumber}/set-current`,
        method: "POST",
        ...params,
      }),
  };
}
