import type { StructuredError } from '@/shared/types/structured-error';

/**
 * User-friendly error messages mapped to error codes.
 * These messages provide context and actionable guidance for users.
 *
 * Used for i18n support and consistent error messaging across the application.
 */
export const ERROR_MESSAGES: Record<string, string> = {
  // OpenAI Agent Errors
  OPENAI_MISSING_CONNECTION:
    'OpenAI connection not configured. Please add an OpenAI connection.',
  OPENAI_INVALID_API_KEY:
    'Invalid OpenAI API key. Please check your connection settings.',
  OPENAI_UNAUTHORIZED:
    'OpenAI authentication failed. Please verify your API key.',
  OPENAI_RATE_LIMITED:
    'OpenAI rate limit reached. Please wait before retrying.',
  OPENAI_SERVER_ERROR:
    'OpenAI service is temporarily unavailable. Please try again later.',
  OPENAI_CLIENT_ERROR:
    'OpenAI request failed. Please check your configuration.',
  OPENAI_INVALID_RESPONSE: 'OpenAI returned an invalid response format.',
  OPENAI_RUNTIME_ERROR:
    'OpenAI agent runtime error. No async runtime available.',

  // Shopify Agent Errors
  SHOPIFY_MISSING_CONNECTION:
    'Shopify connection not configured. Please add a Shopify connection.',
  SHOPIFY_INVALID_CONFIG:
    'Invalid Shopify configuration. Please check shop URL and API key.',
  SHOPIFY_UNAUTHORIZED:
    'Shopify authentication failed. Please verify your credentials.',
  SHOPIFY_RATE_LIMITED:
    'Shopify rate limit reached. Please wait before retrying.',
  SHOPIFY_SERVER_ERROR:
    'Shopify service is temporarily unavailable. Please try again later.',
  SHOPIFY_GRAPHQL_ERROR:
    'Shopify GraphQL request failed. Check the error details.',
  SHOPIFY_VALIDATION_ERROR:
    'Shopify rejected the request due to validation errors.',
  SHOPIFY_NOT_FOUND: 'The requested Shopify resource was not found.',
  SHOPIFY_INVALID_RESPONSE: 'Shopify returned an invalid response format.',
  SHOPIFY_RUNTIME_ERROR:
    'Shopify agent runtime error. No async runtime available.',

  // AWS Bedrock Agent Errors
  BEDROCK_MISSING_CONNECTION:
    'Bedrock connection not configured. Please add an AWS Bedrock connection.',
  BEDROCK_INVALID_CREDENTIALS:
    'Invalid AWS credentials. Please check your configuration.',
  BEDROCK_UNSUPPORTED_MODEL:
    'The selected AI model is not supported by AWS Bedrock.',
  BEDROCK_MISSING_INPUT: 'Required input is missing for Bedrock request.',
  BEDROCK_UNSUPPORTED_INPUT:
    'The input type is not supported for the selected model.',
  BEDROCK_UNAUTHORIZED:
    'AWS Bedrock authentication failed. Please verify your credentials.',
  BEDROCK_RATE_LIMITED:
    'AWS Bedrock is throttling requests. Please wait before retrying.',
  BEDROCK_SERVER_ERROR:
    'AWS Bedrock service is temporarily unavailable. Please try again later.',
  BEDROCK_CLIENT_ERROR:
    'AWS Bedrock request failed. Please check your configuration.',
  BEDROCK_INVALID_RESPONSE: 'AWS Bedrock returned an invalid response format.',
  BEDROCK_RUNTIME_ERROR:
    'Bedrock agent runtime error. No async runtime available.',

  // HDM Facade Errors
  HDM_LLM_MISSING_CONNECTION:
    'LLM connection not configured. Please add a connection.',
  HDM_LLM_UNSUPPORTED_PROVIDER: 'The selected LLM provider is not supported.',
  HDM_COMMERCE_MISSING_CONNECTION:
    'Commerce platform connection not configured. Please add a connection.',
  HDM_COMMERCE_UNSUPPORTED_PLATFORM:
    'The selected commerce platform is not supported.',

  // Object Model Agent Errors
  OBJECT_MODEL_STORE_UNAVAILABLE:
    'Object store is not initialized. Please contact support.',
  OBJECT_MODEL_RUNTIME_ERROR:
    'Object model agent runtime error. No async runtime available.',
};

/**
 * Get a user-friendly localized message for a structured error.
 * Falls back to the error's own message if no mapping exists.
 *
 * @param error - Structured error object
 * @returns User-friendly error message
 *
 * @example
 * ```typescript
 * const message = getLocalizedMessage(error);
 * toast.error(message);
 * ```
 */
export function getLocalizedMessage(error: StructuredError): string {
  return ERROR_MESSAGES[error.code] || error.message;
}

/**
 * Get actionable guidance for common error scenarios.
 * Provides specific steps users can take to resolve the error.
 *
 * @param errorCode - Error code
 * @returns Actionable guidance or null if no specific guidance available
 */
export function getErrorGuidance(errorCode: string): string | null {
  const guidance: Record<string, string> = {
    OPENAI_RATE_LIMITED:
      'Wait a few minutes and try again. Consider upgrading your OpenAI plan for higher limits.',
    SHOPIFY_RATE_LIMITED:
      'Wait a few minutes and try again. Shopify rate limits reset every second.',
    BEDROCK_RATE_LIMITED:
      'Wait a few minutes and try again. Consider requesting a quota increase from AWS.',
    OPENAI_INVALID_API_KEY:
      'Go to Connections and verify your OpenAI API key is correct.',
    SHOPIFY_INVALID_CONFIG:
      'Go to Connections and verify your Shopify shop URL and API key.',
    BEDROCK_INVALID_CREDENTIALS:
      'Go to Connections and verify your AWS credentials.',
    SHOPIFY_VALIDATION_ERROR:
      'Check the validation errors in the details below and fix the data.',
    BEDROCK_UNSUPPORTED_MODEL:
      'Select a different AI model that is supported by AWS Bedrock.',
  };

  return guidance[errorCode] || null;
}
