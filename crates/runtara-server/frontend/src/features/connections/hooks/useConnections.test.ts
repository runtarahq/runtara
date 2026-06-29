import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import { useConnections } from './useConnections';

// Mock the queries module
vi.mock('../queries', () => ({
  getConnections: vi.fn(),
}));

// Mock react-oidc-context
vi.mock('react-oidc-context', () => ({
  useAuth: vi.fn(() => ({
    user: { access_token: 'test-token' },
  })),
}));

import { getConnections } from '../queries';

const mockGetConnections = vi.mocked(getConnections);

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
});
