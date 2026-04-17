import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import {
  useConnections,
  useConnectionById,
  useConnectionTypes,
  useCreateConnection,
  useUpdateConnection,
  useDeleteConnection,
} from './useConnections';

// Mock the queries module
vi.mock('../queries', () => ({
  getConnections: vi.fn(),
  getConnectionById: vi.fn(),
  getConnectionTypes: vi.fn(),
  createConnection: vi.fn(),
  updateConnection: vi.fn(),
  removeConnection: vi.fn(),
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
  getConnections,
  getConnectionById,
  getConnectionTypes,
  createConnection,
  updateConnection,
  removeConnection,
} from '../queries';

const mockGetConnections = vi.mocked(getConnections);
const mockGetConnectionById = vi.mocked(getConnectionById);
const mockGetConnectionTypes = vi.mocked(getConnectionTypes);
const mockCreateConnection = vi.mocked(createConnection);
const mockUpdateConnection = vi.mocked(updateConnection);
const mockRemoveConnection = vi.mocked(removeConnection);

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

// Helper to create mock connection data
function createMockConnection(overrides = {}) {
  return {
    id: '1',
    title: 'Connection 1',
    status: 'active',
    tenantId: 'tenant-1',
    createdAt: '2024-01-01T00:00:00Z',
    updatedAt: '2024-01-01T00:00:00Z',
    connectionType: null,
    ...overrides,
  };
}

// Helper to create mock connection type data
function createMockConnectionType(overrides = {}) {
  return {
    integrationId: 'type-1',
    displayName: 'Type 1',
    fields: [],
    ...overrides,
  };
}

describe('useConnections', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  describe('useConnections', () => {
    it('fetches all connections', async () => {
      const mockConnections = [
        createMockConnection({ id: '1', title: 'Connection 1' }),
        createMockConnection({ id: '2', title: 'Connection 2' }),
      ];
      mockGetConnections.mockResolvedValue(mockConnections as any);

      const { result } = renderHook(() => useConnections(), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(result.current.data).toEqual(mockConnections);
      expect(mockGetConnections).toHaveBeenCalledWith('test-token');
    });

    it('handles error when fetching connections', async () => {
      const error = new Error('Failed to fetch');
      mockGetConnections.mockRejectedValue(error);

      const { result } = renderHook(() => useConnections(), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isError).toBe(true);
      });

      expect(result.current.error).toBe(error);
    });
  });

  describe('useConnectionById', () => {
    it('fetches a single connection by ID', async () => {
      const mockConnection = createMockConnection({
        id: '123',
        title: 'Test Connection',
      });
      mockGetConnectionById.mockResolvedValue(mockConnection as any);

      const { result } = renderHook(() => useConnectionById('123'), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(result.current.data).toEqual(mockConnection);
    });

    it('does not fetch when id is undefined', async () => {
      const { result } = renderHook(() => useConnectionById(undefined), {
        wrapper: createWrapper(),
      });

      // Wait a bit to ensure query doesn't run
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockGetConnectionById).not.toHaveBeenCalled();
      expect(result.current.isPending).toBe(true);
      expect(result.current.fetchStatus).toBe('idle');
    });
  });

  describe('useConnectionTypes', () => {
    it('fetches all connection types', async () => {
      const mockTypes = [
        createMockConnectionType({
          integrationId: 'type-1',
          displayName: 'Type 1',
        }),
        createMockConnectionType({
          integrationId: 'type-2',
          displayName: 'Type 2',
        }),
      ];
      mockGetConnectionTypes.mockResolvedValue(mockTypes as any);

      const { result } = renderHook(() => useConnectionTypes(), {
        wrapper: createWrapper(),
      });

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(result.current.data).toEqual(mockTypes);
    });
  });

  describe('useCreateConnection', () => {
    it('creates a new connection', async () => {
      mockCreateConnection.mockResolvedValue('new-connection-id');

      const { result } = renderHook(() => useCreateConnection(), {
        wrapper: createWrapper(),
      });

      const newConnection = {
        title: 'New Connection',
        integrationId: 'type-1',
      };

      result.current.mutate(newConnection as any);

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockCreateConnection).toHaveBeenCalledWith(
        'test-token',
        newConnection
      );
    });
  });

  describe('useUpdateConnection', () => {
    it('updates an existing connection', async () => {
      mockUpdateConnection.mockResolvedValue(undefined);

      const { result } = renderHook(() => useUpdateConnection(), {
        wrapper: createWrapper(),
      });

      const updateData = {
        id: '123',
        title: 'Updated Connection',
        parameters: { key: 'value' },
      };

      result.current.mutate(updateData);

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockUpdateConnection).toHaveBeenCalledWith(
        'test-token',
        updateData
      );
    });
  });

  describe('useDeleteConnection', () => {
    it('deletes a connection', async () => {
      mockRemoveConnection.mockResolvedValue(undefined);

      const { result } = renderHook(() => useDeleteConnection(), {
        wrapper: createWrapper(),
      });

      result.current.mutate('123');

      await waitFor(() => {
        expect(result.current.isSuccess).toBe(true);
      });

      expect(mockRemoveConnection).toHaveBeenCalledWith('test-token', '123');
    });
  });
});
