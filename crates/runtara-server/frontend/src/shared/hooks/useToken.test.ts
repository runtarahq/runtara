import { describe, expect, it, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useToken } from './useToken';

// Mock react-oidc-context
vi.mock('react-oidc-context', () => ({
  useAuth: vi.fn(),
}));

import { useAuth } from 'react-oidc-context';

const mockUseAuth = vi.mocked(useAuth);

describe('useToken', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('returns access token when user is authenticated', () => {
    const mockToken = 'test-access-token-12345';
    mockUseAuth.mockReturnValue({
      user: {
        access_token: mockToken,
      },
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe(mockToken);
  });

  it('returns empty string when user is null', () => {
    mockUseAuth.mockReturnValue({
      user: null,
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe('');
  });

  it('returns empty string when user is undefined', () => {
    mockUseAuth.mockReturnValue({
      user: undefined,
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe('');
  });

  it('returns empty string when access_token is undefined', () => {
    mockUseAuth.mockReturnValue({
      user: {
        access_token: undefined,
      },
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe('');
  });

  it('returns empty string when access_token is null', () => {
    mockUseAuth.mockReturnValue({
      user: {
        access_token: null,
      },
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe('');
  });

  it('returns empty string when access_token is empty string', () => {
    mockUseAuth.mockReturnValue({
      user: {
        access_token: '',
      },
    } as any);

    const { result } = renderHook(() => useToken());

    expect(result.current).toBe('');
  });

  it('updates when auth context changes', () => {
    const initialToken = 'initial-token';
    const newToken = 'new-token';

    mockUseAuth.mockReturnValue({
      user: {
        access_token: initialToken,
      },
    } as any);

    const { result, rerender } = renderHook(() => useToken());

    expect(result.current).toBe(initialToken);

    // Simulate auth context change
    mockUseAuth.mockReturnValue({
      user: {
        access_token: newToken,
      },
    } as any);

    rerender();

    expect(result.current).toBe(newToken);
  });
});
