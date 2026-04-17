import {
  keepPreviousData,
  useMutation,
  useQuery,
  UseQueryOptions,
  UseMutationOptions,
  QueryKey,
  QueryFunctionContext,
} from '@tanstack/react-query';
import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';
import { isOidcAuth } from '@/shared/config/runtimeConfig';

// Shared interfaces
interface PaginatedResponse<T = unknown> {
  content: T[];
  number: number;
  size: number;
  totalElements: number;
  totalPages: number;
}

// Token type uses 'any' because existing queryFn/mutationFn callbacks expect 'string'
// but hooks provide 'string | undefined'. This maintains backwards compatibility.
type QueryFnType<TData> = (token: any, context?: any) => Promise<TData>;
type MutationFnType<TData, TVariables> = (
  token: any,
  data: TVariables
) => Promise<TData>;
type TableQueryFnType<T> = (
  token: any,
  context?: any
) => Promise<PaginatedResponse<T>>;

interface CustomQueryOptions<TData = unknown>
  extends Omit<
    UseQueryOptions<TData, Error, TData, QueryKey>,
    'queryKey' | 'queryFn'
  > {
  queryKey: QueryKey;
  queryFn: QueryFnType<TData>;
}

export function useCustomQuery<TData = unknown>({
  queryKey,
  queryFn,
  enabled,
  ...options
}: CustomQueryOptions<TData>) {
  const auth = useAuth();
  const token = auth.user?.access_token;

  const customQueryFn = async (context: QueryFunctionContext<QueryKey>) => {
    return queryFn(token, context);
  };

  const customOptions = {
    refetchOnWindowFocus: false,
    placeholderData: keepPreviousData,
    ...options,
  };

  return useQuery<TData, Error, TData, QueryKey>({
    queryKey,
    queryFn: customQueryFn,
    enabled: (!!token || !isOidcAuth) && (enabled ?? true),
    ...customOptions,
  });
}

/**
 * Validation error with step context for highlighting issues in the workflow editor.
 * Matches the ValidationErrorDto from the backend API.
 */
export interface ValidationError {
  /** Error code (e.g., "E023") */
  code: string;
  /** Field name with the error (if applicable) */
  fieldName?: string | null;
  /** Human-readable error message */
  message: string;
  /** Step ID where the error occurred (if applicable) */
  stepId?: string | null;
  /** Additional step IDs involved (for errors spanning multiple steps) */
  relatedStepIds?: string[] | null;
}

interface ApiError extends Error {
  /** Axios error code (e.g., 'ERR_NETWORK', 'ERR_BAD_REQUEST') */
  code?: string;
  /** HTTP status (set directly by axios v1+ even when response parsing fails) */
  status?: number;
  response?: {
    status?: number;
    data?: {
      message?: string;
      success?: boolean;
      /** New format: WorkflowValidationErrorResponse */
      validationErrors?: ValidationError[];
    };
  };
}

interface CustomMutationOptions<TData = unknown, TVariables = unknown>
  extends Omit<UseMutationOptions<TData, ApiError, TVariables>, 'mutationFn'> {
  mutationFn: MutationFnType<TData, TVariables>;
  /**
   * When true, suppresses toast notifications for validation errors (400 status with validationErrors).
   * Use this when you want to handle validation errors in a custom way (e.g., validation panel).
   * Other errors will still show toasts.
   */
  suppressValidationToasts?: boolean;
}

export function useCustomMutation<TData = unknown, TVariables = unknown>({
  mutationFn,
  suppressValidationToasts = false,
  ...options
}: CustomMutationOptions<TData, TVariables>) {
  const auth = useAuth();

  const customMutationFn = async (data: TVariables) => {
    const token = auth.user?.access_token;
    return mutationFn(token, data);
  };

  const handleError = (error: ApiError) => {
    const validationErrors = error.response?.data?.validationErrors;

    // Check if it's a workflow validation error with step context
    if (
      error.response?.status === 400 &&
      validationErrors &&
      validationErrors.length > 0
    ) {
      // If suppressValidationToasts is true, skip toast display
      // Caller is responsible for handling validation errors (e.g., via validation panel)
      if (suppressValidationToasts) {
        return;
      }

      // Group validation errors by step
      const errorsByStep: Record<
        string,
        { stepId: string; errors: ValidationError[] }
      > = {};
      const globalErrors: ValidationError[] = [];

      validationErrors.forEach((validationError) => {
        if (validationError.stepId) {
          if (!errorsByStep[validationError.stepId]) {
            errorsByStep[validationError.stepId] = {
              stepId: validationError.stepId,
              errors: [],
            };
          }
          errorsByStep[validationError.stepId].errors.push(validationError);
        } else {
          globalErrors.push(validationError);
        }
      });

      // Display validation errors for each step
      Object.values(errorsByStep).forEach((step) => {
        const errorsList = step.errors
          .map((err) => `• [${err.code}] ${err.message}`)
          .join('\n');

        toast.error(`Validation errors in step: ${step.stepId}`, {
          description: errorsList,
          duration: 8000,
        });
      });

      // Display global errors (not tied to a specific step)
      if (globalErrors.length > 0) {
        const errorsList = globalErrors
          .map((err) => `• [${err.code}] ${err.message}`)
          .join('\n');

        toast.error('Workflow validation errors', {
          description: errorsList,
          duration: 8000,
        });
      }
    } else if (error.response?.status === 413 || error.status === 413) {
      // Handle 413 Payload Too Large specifically
      toast.error('File too large', {
        description:
          'The uploaded file exceeds the maximum allowed size. Please try with a smaller file.',
        duration: 8000,
      });
    } else if (error.code === 'ERR_NETWORK') {
      // Network errors (no response received) — often caused by the server
      // rejecting a large upload and closing the connection before sending
      // a proper HTTP response.
      toast.error('Network error', {
        description:
          'The request failed due to a network error. If you were uploading a file, it may exceed the maximum allowed size.',
        duration: 8000,
      });
    } else {
      // Default error handling for other errors
      // Use the backend message if available, otherwise fall back to the error message
      const backendMessage = error.response?.data?.message;
      const errorMessage = backendMessage || error.message;

      toast.error(`Error: ${error.response?.status || 'Request failed'}`, {
        description: errorMessage,
      });
    }
  };

  return useMutation<TData, ApiError, TVariables>({
    mutationFn: customMutationFn,
    ...options,
    onError: (error, variables, onMutateResult, context) => {
      handleError(error);
      options.onError?.(error, variables, onMutateResult, context);
    },
  });
}

interface TableQueryOptions<T = unknown>
  extends Omit<
    UseQueryOptions<
      PaginatedResponse<T>,
      Error,
      PaginatedResponse<T>,
      QueryKey
    >,
    'queryKey' | 'queryFn'
  > {
  queryKey: QueryKey;
  queryFn: TableQueryFnType<T>;
}

export function useTableQuery<T = unknown>({
  queryKey,
  queryFn,
  enabled,
  ...options
}: TableQueryOptions<T>) {
  const auth = useAuth();
  const token = auth.user?.access_token;

  const customQueryFn = async (context: QueryFunctionContext<QueryKey>) => {
    return queryFn(token, context);
  };

  const customOptions = {
    refetchOnWindowFocus: false,
    placeholderData: keepPreviousData,
    ...options,
  };

  const { data, isFetching, refetch, isLoading } = useQuery({
    queryKey,
    queryFn: customQueryFn,
    enabled: (!!token || !isOidcAuth) && (enabled ?? true),
    ...customOptions,
  });

  // Provide defaults when data is not yet loaded
  const {
    content = [],
    number: pageIndex = 0,
    size: pageSize = 10,
    totalElements = 0,
    totalPages = 0,
  } = data || {};

  return {
    data: content,
    pageIndex,
    pageSize,
    totalElements,
    totalPages,
    isFetching: isFetching || isLoading,
    refetch,
  };
}
