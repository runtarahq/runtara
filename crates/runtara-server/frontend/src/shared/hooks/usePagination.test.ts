import { describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { usePagination } from './usePagination.ts';

describe('usePagination', () => {
  it('initializes with correct default values', () => {
    const { result } = renderHook(() => usePagination());

    expect(result.current.pagination.pageIndex).toBe(0);
    expect(result.current.pagination.pageSize).toBe(10);
  });

  it('updates pagination state correctly', () => {
    const { result } = renderHook(() => usePagination());

    act(() => {
      result.current.setPagination({
        pageIndex: 2,
        pageSize: 20,
      });
    });

    expect(result.current.pagination.pageIndex).toBe(2);
    expect(result.current.pagination.pageSize).toBe(20);
  });

  it('updates pageIndex only', () => {
    const { result } = renderHook(() => usePagination());

    act(() => {
      result.current.setPagination({
        ...result.current.pagination,
        pageIndex: 3,
      });
    });

    expect(result.current.pagination.pageIndex).toBe(3);
    expect(result.current.pagination.pageSize).toBe(10); // Should remain unchanged
  });

  it('updates pageSize only', () => {
    const { result } = renderHook(() => usePagination());

    act(() => {
      result.current.setPagination({
        ...result.current.pagination,
        pageSize: 50,
      });
    });

    expect(result.current.pagination.pageIndex).toBe(0); // Should remain unchanged
    expect(result.current.pagination.pageSize).toBe(50);
  });
});
