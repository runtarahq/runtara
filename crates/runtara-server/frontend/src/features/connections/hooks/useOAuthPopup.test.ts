import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

import { OAuthPopupClosedError, useOAuthPopup } from './useOAuthPopup';

type FakePopup = { closed: boolean; close: ReturnType<typeof vi.fn> };

function mockPopup(): FakePopup {
  return { closed: false, close: vi.fn() };
}

describe('useOAuthPopup', () => {
  let openSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    openSpy?.mockRestore();
  });

  it('rejects with OAuthPopupClosedError shortly after the popup is closed', async () => {
    const popup = mockPopup();
    openSpy = vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);

    const { result } = renderHook(() => useOAuthPopup());
    const promise = result.current.openOAuthPopup('https://provider.example/auth');
    const assertion = expect(promise).rejects.toBeInstanceOf(
      OAuthPopupClosedError
    );

    // User closes the consent window.
    popup.closed = true;
    // The poll interval detects the close, then the grace window elapses.
    await vi.advanceTimersByTimeAsync(500);
    await vi.advanceTimersByTimeAsync(1500);

    await assertion;
  });

  it('resolves on a success message even if the popup closes right after', async () => {
    const popup = mockPopup();
    openSpy = vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);

    const { result } = renderHook(() => useOAuthPopup());
    const promise = result.current.openOAuthPopup('https://provider.example/auth');

    // Completion posts back and the popup closes at about the same time.
    popup.closed = true;
    window.dispatchEvent(
      new MessageEvent('message', {
        origin: window.location.origin,
        data: { type: 'oauth-complete', success: true, connectionId: 'conn-1' },
      })
    );

    await expect(promise).resolves.toEqual({
      connectionId: 'conn-1',
      success: true,
    });
    // The grace window must not turn a success into a closed-rejection.
    await vi.advanceTimersByTimeAsync(2000);
  });

  it('rejects immediately when the popup is blocked', async () => {
    openSpy = vi.spyOn(window, 'open').mockReturnValue(null);
    const { result } = renderHook(() => useOAuthPopup());
    await expect(
      result.current.openOAuthPopup('https://provider.example/auth')
    ).rejects.toThrow(/allow popups/i);
  });
});
