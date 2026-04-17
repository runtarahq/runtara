import { describe, it, expect } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useDialogState } from './useDialogState';

describe('useDialogState', () => {
  it('initializes with closed state by default', () => {
    const { result } = renderHook(() => useDialogState());
    expect(result.current.isOpen).toBe(false);
  });

  it('initializes with open state when true is passed', () => {
    const { result } = renderHook(() => useDialogState(true));
    expect(result.current.isOpen).toBe(true);
  });

  it('opens dialog with open()', () => {
    const { result } = renderHook(() => useDialogState());

    act(() => {
      result.current.open();
    });

    expect(result.current.isOpen).toBe(true);
  });

  it('closes dialog with close()', () => {
    const { result } = renderHook(() => useDialogState(true));

    act(() => {
      result.current.close();
    });

    expect(result.current.isOpen).toBe(false);
  });

  it('toggles dialog state with toggle()', () => {
    const { result } = renderHook(() => useDialogState());

    // Toggle from closed to open
    act(() => {
      result.current.toggle();
    });
    expect(result.current.isOpen).toBe(true);

    // Toggle from open to closed
    act(() => {
      result.current.toggle();
    });
    expect(result.current.isOpen).toBe(false);
  });

  it('sets specific state with setIsOpen()', () => {
    const { result } = renderHook(() => useDialogState());

    act(() => {
      result.current.setIsOpen(true);
    });
    expect(result.current.isOpen).toBe(true);

    act(() => {
      result.current.setIsOpen(false);
    });
    expect(result.current.isOpen).toBe(false);
  });

  it('maintains stable function references across renders', () => {
    const { result, rerender } = renderHook(() => useDialogState());

    const { open: open1, close: close1, toggle: toggle1 } = result.current;

    rerender();

    expect(result.current.open).toBe(open1);
    expect(result.current.close).toBe(close1);
    expect(result.current.toggle).toBe(toggle1);
  });

  it('open() is idempotent', () => {
    const { result } = renderHook(() => useDialogState());

    act(() => {
      result.current.open();
      result.current.open();
      result.current.open();
    });

    expect(result.current.isOpen).toBe(true);
  });

  it('close() is idempotent', () => {
    const { result } = renderHook(() => useDialogState(true));

    act(() => {
      result.current.close();
      result.current.close();
      result.current.close();
    });

    expect(result.current.isOpen).toBe(false);
  });
});
