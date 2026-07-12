import {
  analyzeFormJson,
  ensureRustValidationInitialized,
  validateFormDefinitionJson,
} from '@/shared/lib/rust-validation-wasm';

import type { FormAnalysisResult, FormDefinition } from './types';

function normalizeResponse(value: unknown): FormAnalysisResult {
  const response =
    value && typeof value === 'object'
      ? (value as Partial<FormAnalysisResult>)
      : {};
  const success = response.success === true;
  const valid = success && response.valid === true;
  return {
    success,
    valid,
    status: valid ? 'valid' : 'invalid',
    fields:
      response.fields && typeof response.fields === 'object'
        ? response.fields
        : {},
    issues: Array.isArray(response.issues) ? response.issues : [],
    message:
      typeof response.message === 'string'
        ? response.message
        : 'Form validation completed',
    wasmAvailable: true,
  };
}

function unavailable(error: unknown): FormAnalysisResult {
  return {
    success: false,
    valid: false,
    status: 'unavailable',
    fields: {},
    issues: [],
    message: 'Rust form validation unavailable; the form cannot be submitted',
    wasmAvailable: false,
    unavailableReason: error instanceof Error ? error.message : String(error),
  };
}

export async function validateFormDefinitionWithRust(
  definition: FormDefinition
): Promise<FormAnalysisResult> {
  try {
    await ensureRustValidationInitialized();
    return normalizeResponse(
      JSON.parse(validateFormDefinitionJson(JSON.stringify(definition)))
    );
  } catch (error) {
    console.warn('Rust form-definition validation WASM unavailable', error);
    return unavailable(error);
  }
}

export async function analyzeFormWithRust(
  definition: FormDefinition,
  data: Record<string, unknown>
): Promise<FormAnalysisResult> {
  try {
    await ensureRustValidationInitialized();
    return normalizeResponse(
      JSON.parse(
        analyzeFormJson(JSON.stringify(definition), JSON.stringify(data))
      )
    );
  } catch (error) {
    console.warn('Rust form analysis WASM unavailable', error);
    return unavailable(error);
  }
}
