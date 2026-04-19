import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import {
  useTriggers,
  useTriggerById,
  useCreateTrigger,
  useUpdateTrigger,
  useDeleteTrigger,
} from './useTriggers';
import { TriggerType } from '../types';

// Mock the queries module
vi.mock('../queries', () => ({
  getInvocationTriggers: vi.fn(),
  getInvocationTriggerById: vi.fn(),
  createInvocationTrigger: vi.fn(),
  updateInvocationTrigger: vi.fn(),
  removeInvocationTrigger: vi.fn(),
}));

// Mock react-oidc-context
vi.mock('react-oidc-context', () => ({
  useAuth: vi.fn(() => ({
    user: { access_token: 'test-token' },
  })),
}));

// Mock sonner toast (used by useCustomMutation)
vi.mock('sonner', () => ({
  toast: {
    error: vi.fn(),
  },
}));

import {
  getInvocationTriggers,
  getInvocationTriggerById,
  createInvocationTrigger,
  updateInvocationTrigger,
  removeInvocationTrigger,
} from '../queries';

const mockGetInvocationTriggers = vi.mocked(getInvocationTriggers);
const mockGetInvocationTriggerById = vi.mocked(getInvocationTriggerById);
const mockCreateInvocationTrigger = vi.mocked(createInvocationTrigger);
const mockUpdateInvocationTrigger = vi.mocked(updateInvocationTrigger);
const mockRemoveInvocationTrigger = vi.mocked(removeInvocationTrigger);

// Helper to create wrapper with QueryClient
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

// Helper to create mock trigger data
function createMockTrigger(overrides = {}) {
  return {
    id: 'trigger-1',
    workflowId: 'scen-1',
    workflowName: 'Workflow 1',
    triggerType: 'CRON' as TriggerType,
    configuration: { expression: '0 0 * * *' },
    active: true,
    singleInstance: false,
    createdAt: '2024-01-01T00:00:00Z',
    updatedAt: '2024-01-01T00:00:00Z',
    ...overrides,
  };
}

describe('useTriggers', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  describe('useTriggers', () => {
    it('fetches all triggers', async () => {
      const mockTriggers = [
        createMockTrigger({ id: 'trigger-1', workflowName: 'Workflow 1' }),
        createMockTrigger({
          id: 'trigger-2',
          workflowId: 'scen-2',
          workflowName: 'Workflow 2',
          triggerType: 'APPLICATION' as TriggerType,
          active: false,
        }),
      ];
      mockGetInvocationTriggers.mockResolvedValue(mockTriggers as any);

      const { result } = renderHook(() => useTriggers(), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(result.current.data).toEqual(mockTriggers);
      expect(mockGetInvocationTriggers).toHaveBeenCalledWith('test-token');
    });

    it('handles error when fetching triggers', async () => {
      const error = new Error('Failed to fetch triggers');
      mockGetInvocationTriggers.mockRejectedValue(error);

      const { result } = renderHook(() => useTriggers(), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isError).toBe(true);
      });

      expect(result.current.error).toBe(error);
    });
  });

  describe('useTriggerById', () => {
    it('fetches a single trigger by ID', async () => {
      const mockTrigger = createMockTrigger({
        id: 'trigger-123',
        workflowName: 'Test Workflow',
      });
      mockGetInvocationTriggerById.mockResolvedValue(mockTrigger as any);

      const { result } = renderHook(() => useTriggerById('trigger-123'), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(result.current.data).toEqual(mockTrigger);
    });

    it('does not fetch when id is undefined', async () => {
      const { result } = renderHook(() => useTriggerById(undefined), {
        wrapper: createWrapper(),
      });

      // Wait a bit to ensure query doesn't run
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockGetInvocationTriggerById).not.toHaveBeenCalled();
      expect(result.current.isPending).toBe(true);
      expect(result.current.fetchStatus).toBe('idle');
    });
  });

  describe('useCreateTrigger', () => {
    it('creates a new trigger', async () => {
      mockCreateInvocationTrigger.mockResolvedValue(undefined);

      const { result } = renderHook(() => useCreateTrigger(), {
        wrapper: createWrapper(),
      });

      const newTrigger = {
        workflowId: 'scen-1',
        triggerType: 'CRON' as TriggerType,
        configuration: { expression: '0 0 * * *' },
        active: true,
      };

      result.current.mutate(newTrigger);

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockCreateInvocationTrigger).toHaveBeenCalledWith(
        'test-token',
        newTrigger
      );
    });
  });

  describe('useUpdateTrigger', () => {
    it('updates an existing trigger', async () => {
      mockUpdateInvocationTrigger.mockResolvedValue(undefined);

      const { result } = renderHook(() => useUpdateTrigger(), {
        wrapper: createWrapper(),
      });

      const updateData = {
        id: 'trigger-123',
        active: false,
        configuration: { expression: '0 12 * * *' },
      };

      result.current.mutate(updateData);

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockUpdateInvocationTrigger).toHaveBeenCalledWith(
        'test-token',
        updateData
      );
    });
  });

  describe('useDeleteTrigger', () => {
    it('deletes a trigger', async () => {
      mockRemoveInvocationTrigger.mockResolvedValue(undefined);

      const { result } = renderHook(() => useDeleteTrigger(), {
        wrapper: createWrapper(),
      });

      result.current.mutate('trigger-123');

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockRemoveInvocationTrigger).toHaveBeenCalledWith(
        'test-token',
        'trigger-123'
      );
    });
  });
});
