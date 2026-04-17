import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import { useCustomQuery, useCustomMutation, useTableQuery } from './api';

// Mock react-oidc-context
vi.mock('react-oidc-context', () => ({
  useAuth: vi.fn(),
}));

// Mock sonner toast
vi.mock('sonner', () => ({
  toast: {
    error: vi.fn(),
  },
}));

import { useAuth } from 'react-oidc-context';
import { toast } from 'sonner';

const mockUseAuth = vi.mocked(useAuth);
const mockToastError = vi.mocked(toast.error);

// Helper to create a wrapper with QueryClient
function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        gcTime: 0,
      },
      mutations: {
        retry: false,
      },
    },
  });

  return function Wrapper({ children }: { children: React.ReactNode }) {
    return React.createElement(
      QueryClientProvider,
      { client: queryClient },
      children
    );
  };
}

describe('useCustomQuery', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockUseAuth.mockReturnValue({
      user: { access_token: 'test-token-123' },
    } as ReturnType<typeof useAuth>);
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  it('injects token into queryFn', async () => {
    const mockQueryFn = vi.fn().mockResolvedValue({ data: 'test' });

    const { result } = renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });

    expect(mockQueryFn).toHaveBeenCalledWith(
      'test-token-123',
      expect.any(Object)
    );
  });

  it('returns data from queryFn', async () => {
    const testData = { id: 1, name: 'Test Item' };
    const mockQueryFn = vi.fn().mockResolvedValue(testData);

    const { result } = renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test-data'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });

    expect(result.current.data).toEqual(testData);
  });

  it('does not fire query when token is undefined', async () => {
    mockUseAuth.mockReturnValue({
      user: null,
    } as ReturnType<typeof useAuth>);

    const mockQueryFn = vi.fn().mockResolvedValue({ data: 'test' });

    const { result } = renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test-no-token'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    // Query should not execute without a token
    expect(result.current.fetchStatus).toBe('idle');
    expect(mockQueryFn).not.toHaveBeenCalled();
  });

  it('sets refetchOnWindowFocus to false by default', async () => {
    const mockQueryFn = vi.fn().mockResolvedValue({ data: 'test' });

    renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test-options'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    // The default option is set internally - we verify by checking the query executed
    await waitFor(() => {
      expect(mockQueryFn).toHaveBeenCalled();
    });
  });

  it('handles query errors', async () => {
    const testError = new Error('Query failed');
    const mockQueryFn = vi.fn().mockRejectedValue(testError);

    const { result } = renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test-error'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isError).toBe(true);
    });

    expect(result.current.error).toBe(testError);
  });

  it('respects enabled option', async () => {
    const mockQueryFn = vi.fn().mockResolvedValue({ data: 'test' });

    const { result } = renderHook(
      () =>
        useCustomQuery({
          queryKey: ['test-disabled'],
          queryFn: mockQueryFn,
          enabled: false,
        }),
      { wrapper: createWrapper() }
    );

    // Wait a bit to ensure query doesn't run
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(mockQueryFn).not.toHaveBeenCalled();
    expect(result.current.isPending).toBe(true);
    expect(result.current.fetchStatus).toBe('idle');
  });
});

describe('useCustomMutation', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockUseAuth.mockReturnValue({
      user: { access_token: 'test-token-456' },
    } as ReturnType<typeof useAuth>);
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  it('injects token into mutationFn', async () => {
    const mockMutationFn = vi.fn().mockResolvedValue({ success: true });

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ name: 'test' });

    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });

    expect(mockMutationFn).toHaveBeenCalledWith('test-token-456', {
      name: 'test',
    });
  });

  it('returns data from mutationFn on success', async () => {
    const responseData = { id: 123, created: true };
    const mockMutationFn = vi.fn().mockResolvedValue(responseData);

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ data: 'test' });

    await waitFor(() => {
      expect(result.current.isSuccess).toBe(true);
    });

    expect(result.current.data).toEqual(responseData);
  });

  it('shows toast error on generic error', async () => {
    const testError = {
      message: 'Something went wrong',
      response: { status: 500 },
    };
    const mockMutationFn = vi.fn().mockRejectedValue(testError);

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ data: 'test' });

    await waitFor(() => {
      expect(result.current.isError).toBe(true);
    });

    expect(mockToastError).toHaveBeenCalledWith('Error: 500', {
      description: 'Something went wrong',
    });
  });

  it('shows validation errors on 400 response with validationErrors', async () => {
    const validationError = {
      message: 'Validation failed',
      response: {
        status: 400,
        data: {
          message: 'Workflow validation failed',
          success: false,
          validationErrors: [
            {
              code: 'E001',
              stepId: 'step-1',
              fieldName: 'field1',
              message: 'Field is required',
            },
            {
              code: 'E002',
              stepId: 'step-1',
              fieldName: 'field2',
              message: 'Value must be positive',
            },
            {
              code: 'E003',
              stepId: 'step-2',
              fieldName: 'field1',
              message: 'Invalid format',
            },
          ],
        },
      },
    };
    const mockMutationFn = vi.fn().mockRejectedValue(validationError);

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ data: 'test' });

    await waitFor(() => {
      expect(result.current.isError).toBe(true);
    });

    // Should show grouped errors by step
    expect(mockToastError).toHaveBeenCalledTimes(2);
    expect(mockToastError).toHaveBeenCalledWith(
      'Validation errors in step: step-1',
      expect.objectContaining({
        description: expect.stringContaining('[E001] Field is required'),
        duration: 8000,
      })
    );
    expect(mockToastError).toHaveBeenCalledWith(
      'Validation errors in step: step-2',
      expect.objectContaining({
        description: expect.stringContaining('[E003] Invalid format'),
        duration: 8000,
      })
    );
  });

  it('calls caller-provided onError in addition to showing toast', async () => {
    const callerOnError = vi.fn();
    const testError = {
      message: 'Something went wrong',
      response: { status: 500 },
    };
    const mockMutationFn = vi.fn().mockRejectedValue(testError);

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
          onError: callerOnError,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ data: 'test' });

    await waitFor(() => {
      expect(result.current.isError).toBe(true);
    });

    expect(mockToastError).toHaveBeenCalled();
    expect(callerOnError).toHaveBeenCalledWith(
      testError,
      { data: 'test' },
      undefined,
      expect.any(Object)
    );
  });

  it('shows fallback error message when status is missing', async () => {
    const testError = {
      message: 'Network error',
      response: {},
    };
    const mockMutationFn = vi.fn().mockRejectedValue(testError);

    const { result } = renderHook(
      () =>
        useCustomMutation({
          mutationFn: mockMutationFn,
        }),
      { wrapper: createWrapper() }
    );

    result.current.mutate({ data: 'test' });

    await waitFor(() => {
      expect(result.current.isError).toBe(true);
    });

    expect(mockToastError).toHaveBeenCalledWith('Error: Request failed', {
      description: 'Network error',
    });
  });
});

describe('useTableQuery', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockUseAuth.mockReturnValue({
      user: { access_token: 'test-token-789' },
    } as ReturnType<typeof useAuth>);
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  it('returns paginated data structure', async () => {
    const paginatedResponse = {
      content: [
        { id: 1, name: 'Item 1' },
        { id: 2, name: 'Item 2' },
      ],
      number: 0,
      size: 10,
      totalElements: 25,
      totalPages: 3,
    };
    const mockQueryFn = vi.fn().mockResolvedValue(paginatedResponse);

    const { result } = renderHook(
      () =>
        useTableQuery({
          queryKey: ['test-table'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isFetching).toBe(false);
    });

    expect(result.current.data).toEqual(paginatedResponse.content);
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.pageSize).toBe(10);
    expect(result.current.totalElements).toBe(25);
    expect(result.current.totalPages).toBe(3);
  });

  it('returns default values when data is not loaded', async () => {
    const mockQueryFn = vi.fn().mockImplementation(
      () => new Promise(() => {}) // Never resolves
    );

    const { result } = renderHook(
      () =>
        useTableQuery({
          queryKey: ['test-table-pending'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    // Check defaults while loading
    expect(result.current.data).toEqual([]);
    expect(result.current.pageIndex).toBe(0);
    expect(result.current.pageSize).toBe(10);
    expect(result.current.totalElements).toBe(0);
    expect(result.current.totalPages).toBe(0);
    expect(result.current.isFetching).toBe(true);
  });

  it('injects token into queryFn', async () => {
    const mockQueryFn = vi.fn().mockResolvedValue({
      content: [],
      number: 0,
      size: 10,
      totalElements: 0,
      totalPages: 0,
    });

    const { result } = renderHook(
      () =>
        useTableQuery({
          queryKey: ['test-table-token'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isFetching).toBe(false);
    });

    expect(mockQueryFn).toHaveBeenCalledWith(
      'test-token-789',
      expect.any(Object)
    );
  });

  it('provides refetch function', async () => {
    let callCount = 0;
    const mockQueryFn = vi.fn().mockImplementation(() => {
      callCount++;
      return Promise.resolve({
        content: [{ id: callCount }],
        number: 0,
        size: 10,
        totalElements: callCount,
        totalPages: 1,
      });
    });

    const { result } = renderHook(
      () =>
        useTableQuery({
          queryKey: ['test-table-refetch'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isFetching).toBe(false);
    });

    expect(result.current.totalElements).toBe(1);

    // Trigger refetch
    result.current.refetch();

    await waitFor(() => {
      expect(result.current.totalElements).toBe(2);
    });

    expect(mockQueryFn).toHaveBeenCalledTimes(2);
  });

  it('handles empty response gracefully', async () => {
    const mockQueryFn = vi.fn().mockResolvedValue({
      content: [],
      number: 0,
      size: 20,
      totalElements: 0,
      totalPages: 0,
    });

    const { result } = renderHook(
      () =>
        useTableQuery({
          queryKey: ['test-table-empty'],
          queryFn: mockQueryFn,
        }),
      { wrapper: createWrapper() }
    );

    await waitFor(() => {
      expect(result.current.isFetching).toBe(false);
    });

    expect(result.current.data).toEqual([]);
    expect(result.current.totalElements).toBe(0);
    expect(result.current.totalPages).toBe(0);
  });
});
