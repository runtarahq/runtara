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

/** Status of an onboarding flow or step */
export enum OnboardingStatus {
  NOT_STARTED = "NOT_STARTED",
  IN_PROGRESS = "IN_PROGRESS",
  SKIPPED = "SKIPPED",
  FINISHED = "FINISHED",
}

/** Connection type for integration entities */
export enum ConnectionType {
  API_KEY = "API_KEY",
  OAUTH2 = "OAUTH2",
  USERNAME_PASSWORD = "USERNAME_PASSWORD",
  SSH_KEY = "SSH_KEY",
}

/** API key record (key_hash is never exposed) */
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

export interface BillingPortalResponse {
  /** URL to the Stripe billing portal session */
  url: string;
}

/** Request to create a new API key */
export interface CreateApiKeyRequest {
  /** Optional expiration time */
  expires_at?: string | null;
  /** Human-readable name for the key */
  name: string;
}

/** Response when creating an API key (includes plaintext key ONCE) */
export type CreateApiKeyResponse = ApiKey & {
  /** The plaintext API key — shown only once, store it securely */
  key: string;
};

export interface CreateBillingPortalRequest {
  /** URL to redirect to after the customer finishes managing billing */
  return_url: string;
}

export interface ErrorResponse {
  error: string;
  message: string;
}

/** Response for GET /api/management/onboarding/{flow_id} */
export interface GetOnboardingFlowResponse {
  /**
   * Creation timestamp
   * @format date-time
   */
  created_at: string;
  /**
   * Unique identifier for the onboarding flow
   * @format uuid
   */
  id: string;
  /** Human-readable name of the onboarding flow */
  name: string;
  /** Current status of the overall flow */
  status: OnboardingStatus;
  /**
   * Step-level status tracking
   * Format: {"step_uuid": "status", ...}
   */
  steps: Value;
  /**
   * Last update timestamp
   * @format date-time
   */
  updated_at: string;
}

/** Integration entity model */
export interface IntegrationEntity {
  /**
   * Category of the integration
   * @example "E-commerce"
   */
  category?: string | null;
  /** Type of connection used by this integration */
  connection_type?: null | ConnectionType;
  /**
   * Timestamp when the entity was created
   * @example "2025-01-15T10:30:00Z"
   */
  created_at: string;
  /**
   * Description of the integration
   * @example "E-commerce platform for online stores"
   */
  description?: string | null;
  /** Additional details in JSON format */
  details?: object | null;
  /**
   * Whether the integration is enabled
   * @example true
   */
  enabled: boolean;
  /**
   * URL to the integration's icon
   * @example "https://example.com/icons/shopify.png"
   */
  icon_url?: string | null;
  /**
   * Unique identifier for the integration entity
   * @example "shopify"
   */
  id: string;
  /**
   * Name of the integration
   * @example "Shopify"
   */
  name: string;
  /** Supported operators in JSON format */
  supported_operators?: object | null;
  /**
   * Timestamp when the entity was last updated
   * @example "2025-01-15T10:30:00Z"
   */
  updated_at: string;
}

/** Request body for PUT /api/management/onboarding/{flow_id} */
export interface UpdateOnboardingFlowRequest {
  /** Optional: Name of the onboarding flow (required for initial creation) */
  name?: string | null;
  /** Optional: Update the overall flow status */
  status?: null | OnboardingStatus;
  /**
   * Optional: Step ID to update
   * @format uuid
   */
  step_id?: string | null;
  /** Optional: Status for the specified step (required if step_id is provided) */
  step_status?: null | OnboardingStatus;
}

/** Response for PUT /api/management/onboarding/{flow_id} */
export interface UpdateOnboardingFlowResponse {
  /**
   * Unique identifier for the onboarding flow
   * @format uuid
   */
  id: string;
  /** Human-readable name of the onboarding flow */
  name: string;
  /** Current status of the overall flow */
  status: OnboardingStatus;
  /** Step-level status tracking */
  steps: Value;
  /**
   * Last update timestamp
   * @format date-time
   */
  updated_at: string;
}

export type Value = any;

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
 * @title Runtara Management API
 * @version 0.1.0
 * @license MIT
 *
 * REST API for managing integration entities in the Runtara platform
 */
export class Api<
  SecurityDataType extends unknown,
> extends HttpClient<SecurityDataType> {
  api = {
    /**
 * No description
 *
 * @tags API Keys
 * @name ListApiKeys
 * @summary List all API keys for the authenticated tenant.
Key hashes are never exposed.
 * @request GET:/api/management/api-keys
 * @secure
 */
    listApiKeys: (params: RequestParams = {}) =>
      this.request<ApiKey[], ErrorResponse>({
        path: `/api/management/api-keys`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
 * No description
 *
 * @tags API Keys
 * @name CreateApiKey
 * @summary Create a new API key for the authenticated tenant.
The plaintext key is returned ONCE in the response — store it securely.
 * @request POST:/api/management/api-keys
 * @secure
 */
    createApiKey: (data: CreateApiKeyRequest, params: RequestParams = {}) =>
      this.request<CreateApiKeyResponse, ErrorResponse>({
        path: `/api/management/api-keys`,
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
     * @tags API Keys
     * @name RevokeApiKey
     * @summary Revoke an API key. The key can no longer be used for authentication.
     * @request DELETE:/api/management/api-keys/{id}
     * @secure
     */
    revokeApiKey: (id: string, params: RequestParams = {}) =>
      this.request<void, ErrorResponse>({
        path: `/api/management/api-keys/${id}`,
        method: "DELETE",
        secure: true,
        ...params,
      }),

    /**
     * No description
     *
     * @tags Billing
     * @name CreateBillingPortalSession
     * @summary Create a Stripe billing portal session
     * @request POST:/api/management/billing-dashboard
     */
    createBillingPortalSession: (
      data: CreateBillingPortalRequest,
      params: RequestParams = {},
    ) =>
      this.request<BillingPortalResponse, ErrorResponse>({
        path: `/api/management/billing-dashboard`,
        method: "POST",
        body: data,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Integration Entities
     * @name ListIntegrationEntities
     * @summary List all integration entities
     * @request GET:/api/management/integrations
     */
    listIntegrationEntities: (params: RequestParams = {}) =>
      this.request<IntegrationEntity[], ErrorResponse>({
        path: `/api/management/integrations`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Integration Entities
     * @name ListIntegrationEntitiesByOperator
     * @summary List integration entities by supported operator
     * @request GET:/api/management/integrations/by-operator
     */
    listIntegrationEntitiesByOperator: (
      query: {
        /**
         * The operator to filter by (e.g., "RemoteSFTPAgent", "RemoteShopifyAgent")
         * @example "RemoteSFTPAgent"
         */
        operator: string;
      },
      params: RequestParams = {},
    ) =>
      this.request<IntegrationEntity[], ErrorResponse>({
        path: `/api/management/integrations/by-operator`,
        method: "GET",
        query: query,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Integration Entities
     * @name GetIntegrationEntity
     * @summary Get a single integration entity by ID
     * @request GET:/api/management/integrations/{id}
     */
    getIntegrationEntity: (id: string, params: RequestParams = {}) =>
      this.request<IntegrationEntity, ErrorResponse>({
        path: `/api/management/integrations/${id}`,
        method: "GET",
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Onboarding
     * @name GetOnboardingFlow
     * @summary Get the current state of an onboarding flow
     * @request GET:/api/management/onboarding/{flow_id}
     * @secure
     */
    getOnboardingFlow: (flowId: string, params: RequestParams = {}) =>
      this.request<GetOnboardingFlowResponse, ErrorResponse>({
        path: `/api/management/onboarding/${flowId}`,
        method: "GET",
        secure: true,
        format: "json",
        ...params,
      }),

    /**
     * No description
     *
     * @tags Onboarding
     * @name UpdateOnboardingFlow
     * @summary Update the state of an onboarding flow (or create if doesn't exist)
     * @request PUT:/api/management/onboarding/{flow_id}
     * @secure
     */
    updateOnboardingFlow: (
      flowId: string,
      data: UpdateOnboardingFlowRequest,
      params: RequestParams = {},
    ) =>
      this.request<UpdateOnboardingFlowResponse, ErrorResponse>({
        path: `/api/management/onboarding/${flowId}`,
        method: "PUT",
        body: data,
        secure: true,
        type: ContentType.Json,
        format: "json",
        ...params,
      }),
  };
}
